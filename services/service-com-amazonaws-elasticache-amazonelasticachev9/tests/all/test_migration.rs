// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestMigration`, and for the sentence the operation that STARTS a
//! migration refuses a group whose status cannot take one with.
//!
//! The rules under test:
//!
//! * "Async API to test connection between source and target replication group" — the call
//!   decides whether there is a migration to test at all: the group has to be one this
//!   account holds, provisioned and idle, with no data sync of its own running, and the
//!   list has to name endpoints a migration could read from ("List should have only one
//!   element"). A call that passes those reports the group it tested and changes NOTHING
//!   about it, so a `StartMigration` right after one is accepted;
//! * a refusal is the modelled fault of the thing that is wrong — the group's own not-found
//!   fault (404), its state fault, its already-under-migration fault, and the parameter
//!   refusal for a value no group could carry or a list no test could read — and each states
//!   its rule in ElastiCache's own words.
//!
//! The tests are tables: the rows span how the group is partitioned, which way a request can
//! be wrong, and which status the group is in, so no single recorded request is what makes
//! them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticache_amazonelasticachev9::{service, state};

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// A span far longer than any transition this service publishes, for a group a row holds in
/// a non-terminal status on purpose — so what the row asserts does not depend on how fast
/// the host runs.
const NEVER_SETTLES: Duration = Duration::from_secs(60 * 60);

/// The sentences this service refuses a migration with, written out here rather than read
/// off the constants the handlers carry, so the suite pins the WORDING: the statuses a test
/// of a migration needs, the statuses a start needs, and the one-element rule
/// `CustomerNodeEndpointList` states.
const TEST_STATE_SENTENCE: &str = "Replication group must be in available status to test migration.";
const START_STATE_SENTENCE: &str =
    "Replication group must be in available or migrating status to start migration.";
const ONE_SOURCE_SENTENCE: &str = "List should have only one element.";

/// Hold the stored replication group `id` in `status`, with nothing the clock owes it due
/// inside this test.
fn hold_group_status(emulator: &Emulator, id: &str, status: &str) {
    emulator.with_state(|store| {
        let group = store
            .replication_groups
            .get_mut(id)
            .unwrap_or_else(|| panic!("case {id:?}: the group just created must be stored"));
        group.status = status.to_string();
        group.pending_status = Some(state::PendingTransition {
            due: SystemTime::now() + NEVER_SETTLES,
            status: state::STATUS_AVAILABLE.to_string(),
        });
    });
}

/// The `CustomerNodeEndpoint` form pairs for one list of source endpoints: awsQuery indexes
/// a wrapped list's elements from 1 beneath the member segment, which for
/// `CustomerNodeEndpointList` is the Smithy default `member` (the list declares no
/// `xmlName`).
pub(crate) fn endpoint_params(endpoints: &[(&str, i32)]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (index, (address, port)) in endpoints.iter().enumerate() {
        let position = index + 1;
        pairs.push((
            format!("CustomerNodeEndpointList.member.{position}.Address"),
            (*address).to_string(),
        ));
        pairs.push((
            format!("CustomerNodeEndpointList.member.{position}.Port"),
            port.to_string(),
        ));
    }
    pairs
}

/// Wait until the replication group `id` reports the status the clock owes it after a
/// create or a change: `available`.
pub(crate) async fn await_available_group(emulator: &Emulator, case: &str, id: &str) -> Reply {
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the replication group {id} settling at available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await
}

/// Create one replication group with `shards` shards and let the clock settle it at
/// `available`.
pub(crate) async fn create_settled_group(emulator: &Emulator, case: &str, id: &str, shards: i32) {
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "a migration test target"),
                ("Engine", state::DEFAULT_ENGINE),
                ("CacheNodeType", "cache.m7g.large"),
                ("NumNodeGroups", &shards.to_string()),
                ("ReplicasPerNodeGroup", "1"),
                // A migration is only ever carried into a group that can
                // promote a replica in its primary's place, so every group a
                // migration row names has automatic failover on.
                ("AutomaticFailoverEnabled", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    await_available_group(emulator, case, id).await;
}

/// `TestMigration` of `endpoints` into the replication group `id`.
pub(crate) async fn test_migration(
    emulator: &Emulator,
    id: &str,
    endpoints: &[(&str, i32)],
) -> Reply {
    let mut pairs = params(&[("ReplicationGroupId", id)]);
    pairs.extend(endpoint_params(endpoints));
    emulator.call("TestMigration", &pairs).await
}

/// `StartMigration` of one source endpoint into the replication group `id`.
async fn start_migration(emulator: &Emulator, id: &str) -> Reply {
    let mut pairs = params(&[("ReplicationGroupId", id)]);
    pairs.extend(endpoint_params(&[("10.0.0.9", 6379)]));
    emulator.call("StartMigration", &pairs).await
}

/// `DescribeReplicationGroups` for one group by id.
pub(crate) async fn describe_group(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// A test of a group that CAN take a migration reports that group and leaves it exactly as
/// it was, so the `StartMigration` the test precedes is accepted.
#[tokio::test]
async fn a_test_reports_the_group_and_changes_nothing_about_it() {
    // The axis: how the group's keyspace is partitioned, and which endpoint the test names —
    // the address and the port a source is reached at, at the published port bounds
    // (`CustomerNodeEndpoint$Port` is a TCP port, so 1 and 65535 are the edges).
    let rows: &[(&str, &str, i32, (&str, i32))] = &[
        (
            "a cluster mode disabled group tested against one source",
            "tested-unsharded",
            1,
            ("10.0.0.21", 6379),
        ),
        (
            "a cluster mode enabled group tested against a source on the lowest port",
            "tested-sharded",
            3,
            ("source.example.internal", 1),
        ),
        (
            "a group tested against a source on the highest port",
            "tested-high-port",
            2,
            ("10.0.0.22", 65_535),
        ),
    ];

    for (case, id, shards, endpoint) in rows {
        let emulator = Emulator::new();
        create_settled_group(&emulator, case, id, *shards).await;

        let tested = test_migration(&emulator, id, &[*endpoint]).await;
        assert_eq!(
            tested.status, 200,
            "case {case:?}: expected the test to be accepted, got {} {}",
            tested.status, tested.body
        );
        assert_eq!(
            tested.text("ReplicationGroupId"),
            Some(*id),
            "case {case:?}: expected the group it tested to be reported, got {}",
            tested.body
        );
        assert_eq!(
            tested.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: a test starts no work on the group, so it stays available, got {}",
            tested.body
        );
        assert_eq!(
            tested.all("NodeGroups/NodeGroup/NodeGroupId").len(),
            usize::try_from(*shards).unwrap_or(0),
            "case {case:?}: expected the group's own shards to be reported, got {}",
            tested.body
        );

        // The group a later read reports is the one the test reported: a test writes
        // nothing, so the migration a client starts next is accepted.
        let described = describe_group(&emulator, id).await;
        assert_eq!(
            described.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: expected a later read to report the group available, got {}",
            described.body
        );
        let started = start_migration(&emulator, id).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected a migration to be startable after the test, got {} {}",
            started.status, started.body
        );
    }
}

/// One row of the refusal table: the case, the group the request names, the endpoints it
/// carries, and the status, wire code and message it must be refused with.
type TestRefusal = (
    &'static str,
    &'static str,
    &'static [(&'static str, i32)],
    u16,
    &'static str,
    String,
);

/// A test of a group that cannot take a migration is refused with the modelled fault of
/// whatever is wrong, and the group is left as it was.
#[tokio::test]
async fn a_test_that_has_no_migration_to_test_is_refused_with_its_own_fault() {
    let emulator = Emulator::new();
    create_settled_group(&emulator, "the refusal table's group", "test-refusals", 1).await;
    // A second group, left carrying an open data sync while reporting `available`, so the
    // already-under-migration row has one to collide with. A start of its own leaves the
    // group applying the migration, and the sync is closed when that work settles, so the
    // row seeds the record on the store the handlers read and then asserts over the wire
    // that the refusal is computed FROM it.
    create_settled_group(&emulator, "the migrating group", "test-migrating", 1).await;
    emulator.with_state(|store| {
        let group = store
            .replication_groups
            .get_mut("test-migrating")
            .expect("the group just created must be stored");
        let mut migration = state::MigrationRecord::prepared(vec![state::SourceEndpointRecord {
            address: "10.0.0.9".to_string(),
            port: 6379,
        }]);
        migration.status = state::MIGRATION_STATUS_SYNCING.to_string();
        group.migration = Some(migration);
    });
    // A third group held in the status a just-created one has, for the state row.
    create_settled_group(&emulator, "the group the state row names", "test-creating", 1).await;
    hold_group_status(&emulator, "test-creating", state::STATUS_CREATING);

    // Each row: the case, the group the request names, the endpoints it carries, the status,
    // the wire code and the message the refusal must state.
    let rows: &[TestRefusal] = &[
        (
            "a group this account does not hold",
            "never-created-group",
            &[("10.0.0.1", 6379)],
            404,
            "ReplicationGroupNotFoundFault",
            service::replication_group_not_found::quoted_sentence("never-created-group"),
        ),
        (
            "a group still coming up",
            "test-creating",
            &[("10.0.0.1", 6379)],
            400,
            "InvalidReplicationGroupState",
            TEST_STATE_SENTENCE.to_string(),
        ),
        (
            "a group that already carries an open migration",
            "test-migrating",
            &[("10.0.0.1", 6379)],
            400,
            "ReplicationGroupAlreadyUnderMigrationFault",
            "Replication group test-migrating is already under migration.".to_string(),
        ),
        (
            "two sources for one test",
            "test-refusals",
            &[("10.0.0.1", 6379), ("10.0.0.2", 6379)],
            400,
            "InvalidParameterValue",
            ONE_SOURCE_SENTENCE.to_string(),
        ),
        (
            "a source whose port is past the widest a TCP endpoint can name",
            "test-refusals",
            &[("10.0.0.1", 65_536)],
            400,
            "InvalidParameterValue",
            format!(
                "Invalid value 65536 for Port: must be between 1 and {}.",
                state::MAX_PORT
            ),
        ),
        (
            "a source with no address",
            "test-refusals",
            &[("", 6379)],
            400,
            "InvalidParameterValue",
            "The parameter CustomerNodeEndpoint.Address must be provided and must not be blank."
                .to_string(),
        ),
    ];

    for (case, id, endpoints, status, code, message) in rows {
        let refused = test_migration(&emulator, id, endpoints).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            message.as_str(),
            "case {case:?}: expected the message {message:?}, got {:?}",
            refused.error_message()
        );
    }

    // Nothing the table did reached the group it kept naming: it is still available and
    // still carries no migration, so a migration can be started on it.
    let described = describe_group(&emulator, "test-refusals").await;
    assert_eq!(
        described.text("Status"),
        Some(state::STATUS_AVAILABLE),
        "a refused test must change nothing, got {}",
        described.body
    );
    let started = start_migration(&emulator, "test-refusals").await;
    assert_eq!(
        started.status, 200,
        "a refused test must leave the group able to start a migration, got {} {}",
        started.status, started.body
    );
}

/// An identifier no group of this service could carry is refused as the bad parameter it is,
/// before the group is resolved — so the answer reports the rule rather than a group that
/// does not exist.
#[tokio::test]
async fn an_identifier_no_group_could_carry_is_refused_before_the_lookup() {
    let emulator = Emulator::new();
    // The axis: each way a value can fall outside the identifier rule this service states,
    // plus the length bound (`CacheClusterId`: "A name must contain from 1 to 50
    // alphanumeric characters or hyphens").
    let too_long = "a".repeat(state::CLUSTER_ID_MAX_LEN + 1);
    let rows: &[(&str, &str)] = &[
        ("one character past the bound", too_long.as_str()),
        ("a value starting with a digit", "9group"),
        ("a value ending with a hyphen", "group-"),
        ("a value with two consecutive hyphens", "gro--up"),
        ("a value carrying a character outside the set", "group_one"),
    ];

    for (case, id) in rows {
        let refused = test_migration(&emulator, id, &[("10.0.0.1", 6379)]).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidParameterValue"),
            "case {case:?}: expected 400 InvalidParameterValue, got {} {}",
            refused.status,
            refused.body
        );
        let expected = state::identifier_refusal("Cluster ID", id, state::CLUSTER_ID_MAX_LEN)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the row must be outside the identifier rule")
            });
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected the identifier refusal, got {:?}",
            refused.error_message()
        );
    }
}

/// A `StartMigration` of a group whose status cannot take one is refused with the sentence
/// that states the statuses THAT operation accepts.
#[tokio::test]
async fn starting_a_migration_on_a_group_that_is_not_available_states_its_own_statuses() {
    let emulator = Emulator::new();

    // The axis: HOW the group came to be in a status that cannot take a migration — one
    // still being created, and one busy with a change of its own.
    create_settled_group(&emulator, "the group still coming up", "start-creating", 1).await;
    hold_group_status(&emulator, "start-creating", state::STATUS_CREATING);

    create_settled_group(&emulator, "the group a migration leaves modifying", "start-busy", 1)
        .await;
    let started = start_migration(&emulator, "start-busy").await;
    assert_eq!(
        started.status, 200,
        "the first migration must start, got {} {}",
        started.status, started.body
    );
    hold_group_status(&emulator, "start-busy", state::STATUS_MODIFYING);

    for (case, id) in [
        ("a group still being created", "start-creating"),
        ("a group busy with a change of its own", "start-busy"),
    ] {
        let refused = start_migration(&emulator, id).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidReplicationGroupState"),
            "case {case:?}: expected 400 InvalidReplicationGroupState, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            START_STATE_SENTENCE,
            "case {case:?}: expected the start-migration state sentence, got {:?}",
            refused.error_message()
        );
    }
}
