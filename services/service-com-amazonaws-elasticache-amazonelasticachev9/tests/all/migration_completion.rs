// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CompleteMigration`.
//!
//! The rules under test:
//!
//! * "Completes the migration of data" — the cutover it performs closes the ONE open
//!   migration recorded on the target the request names, so the target a later read
//!   reports carries no migration and a second completion is refused;
//! * a cutover exists only inside a migration that is MIGRATING: a target carrying none,
//!   or one whose data sync is not in flight because the work that establishes it is still
//!   under way, is refused with the modelled parameter refusal that states that rule — and
//!   `Force`, which bypasses the data-in-sync check, is not a way around it;
//! * a migration has ONE target, so a request naming both or neither is refused, and a
//!   target this account does not hold is that target's own not-found fault.
//!
//! The tests are tables: the rows span which target the request names, whether `Force` is
//! sent, which phase an open migration is in, and each way a request can be wrong, so no
//! single recorded request is what makes them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts. The one exception is the serverless rows: the migration a completion closes is
//! opened by `PrepareMigration`, which is still a scaffolded 501 with its own gap issue,
//! so those rows seed the migration through [`crate::harness::Emulator::with_state`] — the
//! same store the handlers read — and then assert over the wire that the answer is
//! computed FROM it.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, PAST_TRANSITION, REGION};

/// One row of the target table: the case, the form pairs the request carries, and the
/// status plus wire code it must be refused with.
type TargetRefusal<'a> = (&'static str, Vec<(&'static str, &'a str)>, u16, &'static str);

/// Create one replication group and let the clock settle it at `available`.
async fn create_settled_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a migration target"),
        ("Engine", state::DEFAULT_ENGINE),
        ("CacheNodeType", "cache.m7g.large"),
        // A migration is only ever carried into a group that can promote a
        // replica in its primary's place, so every group here has automatic
        // failover on — which takes a second cache cluster in the group.
        ("NumCacheClusters", "2"),
        ("AutomaticFailoverEnabled", "true"),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// Create one serverless cache and let the clock settle it.
async fn create_settled_cache(emulator: &Emulator, case: &str, name: &str) {
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", name), ("Engine", "valkey")]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// `CompleteMigration` with the pairs a row sends.
async fn complete(emulator: &Emulator, extra: &[(&str, &str)]) -> Reply {
    emulator.call("CompleteMigration", &params(extra)).await
}

/// Ask for the cutover until it is SERVED, and answer with that reply.
///
/// Establishing the data sync a start began is work the service carries out
/// after the call ([`state::MIGRATION_SYNC_ESTABLISHMENT`]), and the refusal
/// until then says to retry later — which is what this does, looking again
/// until the cutover goes through rather than waiting a fixed time.
async fn poll_completed(emulator: &Emulator, case: &str, id: &str) -> Reply {
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the data sync into {id} being established so the cutover is served"),
        "CompleteMigration",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.status == 200,
    )
    .await
}

/// `StartMigration` of one source endpoint into the replication group `id`.
async fn start_migration(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "StartMigration",
            &params(&[
                ("ReplicationGroupId", id),
                ("CustomerNodeEndpointList.member.1.Address", "10.0.0.9"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ]),
        )
        .await
}

/// `DescribeReplicationGroups` for one group by id.
async fn describe_group(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// `DescribeServerlessCaches` for one cache by name.
async fn describe_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

/// Open a migration on the stored serverless cache `name`, in `status`.
///
/// `PrepareMigration` is still a scaffolded 501, so the record is seeded into the store the
/// handlers read rather than through the wire.
fn open_cache_migration(emulator: &Emulator, name: &str, status: &str) {
    emulator.with_state(|store| {
        let cache = store
            .serverless_caches
            .get_mut(name)
            .unwrap_or_else(|| panic!("case {name:?}: the cache just created must be stored"));
        let mut migration = state::MigrationRecord::prepared(vec![state::SourceEndpointRecord {
            address: format!("{name}-source.example.internal"),
            port: 6379,
        }]);
        migration.status = status.to_string();
        cache.migration = Some(migration);
    });
}

/// A completion of a replication group that is not migrating is refused with the rule,
/// whatever `Force` says and however the group got there.
#[tokio::test]
async fn a_group_that_is_not_migrating_cannot_be_completed() {
    let emulator = Emulator::new();
    create_settled_group(&emulator, "the idle group", "idle-group", &[]).await;

    // Each row: the case, the pairs the request carries beside the group id.
    let rows: [(&str, &[(&str, &str)]); 3] = [
        ("no Force member at all", &[]),
        ("Force false", &[("Force", "false")]),
        ("Force true, which bypasses no rule here", &[("Force", "true")]),
    ];

    for (case, extra) in rows {
        let mut pairs = vec![("ReplicationGroupId", "idle-group")];
        pairs.extend(extra.iter().copied());
        let refused = complete(&emulator, &pairs).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected the modelled parameter refusal, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            state::MIGRATION_NOT_MIGRATING_MESSAGE,
            "case {case:?}: expected the migrating-state sentence, got {:?}",
            refused.error_message()
        );
        // The group is left exactly as it was: a refusal is not a change.
        let described = describe_group(&emulator, "idle-group").await;
        assert_eq!(
            described.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: the refused group must still be available, got {:?}",
            described.text("Status")
        );
    }
}

/// The whole migration path over the wire: a group whose data sync the service is still
/// establishing cannot be completed, one whose sync is in flight can, and the completion
/// closes it.
#[tokio::test]
async fn a_migrating_group_is_completed_once_its_data_sync_is_in_flight() {
    let emulator = Emulator::new();
    create_settled_group(&emulator, "the migrated group", "migrated-group", &[]).await;

    let started = start_migration(&emulator, "migrated-group").await;
    assert_eq!(
        started.status, 200,
        "StartMigration must answer 200, got {} {}",
        started.status, started.body
    );
    // The sync that start began is not established yet, so there is nothing to cut over and
    // the refusal says to retry later.
    let too_early = complete(&emulator, &[("ReplicationGroupId", "migrated-group")]).await;
    assert_eq!(
        (too_early.status, too_early.error_code()),
        (400, "InvalidParameterValue"),
        "a cutover before the sync is established must be refused, got {} {}",
        too_early.status,
        too_early.body
    );
    assert_eq!(
        too_early.error_message(),
        state::MIGRATION_NOT_MIGRATING_MESSAGE,
        "expected the migrating-state sentence, got {:?}",
        too_early.error_message()
    );
    // Retried, it is served once the service has established that sync.
    let completed = poll_completed(&emulator, "the migrated group", "migrated-group").await;
    assert_eq!(
        completed.status, 200,
        "a retried completion must be served, got {} {}",
        completed.status, completed.body
    );
    assert_eq!(
        completed.text("ReplicationGroupId"),
        Some("migrated-group"),
        "the answer must report the group it completed, got {}",
        completed.body
    );
    // The cutover is work the service carries out after the call, so the group is
    // transient until the clock owes it `available` again.
    assert_eq!(
        completed.text("Status"),
        Some(state::STATUS_MODIFYING),
        "expected the transient status in the answer, got {:?}",
        completed.text("Status")
    );

    // The migration is CLOSED: a second completion is the not-migrating refusal again,
    // even once the group has settled. The suite waits for THAT condition — the group
    // reporting `available` — rather than for a fixed time.
    let described = poll_until(
        &emulator,
        REGION,
        "the migrated group",
        "the group settling at available after the cutover",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "migrated-group")]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    assert_eq!(
        described.text("Status"),
        Some(state::STATUS_AVAILABLE),
        "expected the group settled once the clock owes it, got {:?}",
        described.text("Status")
    );
    let again = complete(&emulator, &[("ReplicationGroupId", "migrated-group")]).await;
    assert_eq!(
        again.error_code(),
        "InvalidParameterValue",
        "a second completion must be refused, got {}",
        again.body
    );
    assert_eq!(
        again.error_message(),
        state::MIGRATION_NOT_MIGRATING_MESSAGE,
        "expected the migrating-state sentence on the second completion, got {:?}",
        again.error_message()
    );
    // And the group may be migrated again, which is what proves the record was closed
    // rather than left behind.
    let restarted = start_migration(&emulator, "migrated-group").await;
    assert_eq!(
        restarted.status, 200,
        "a group whose migration was completed must take another, got {} {}",
        restarted.status, restarted.body
    );
}

/// A serverless cache's completion follows the same rule, and answers with the cache's own
/// modelled faults: the rows span the phase the open migration is in.
#[tokio::test]
async fn a_serverless_cache_is_completed_only_while_its_migration_is_migrating() {
    // Each row: the case, the migration phase seeded on the cache (`None` for a cache
    // carrying none at all), the status, and the wire code the completion is answered
    // with (`None` when it must succeed).
    let rows: [(&str, Option<&str>, u16, Option<&str>); 5] = [
        (
            "a cache under no migration",
            None,
            400,
            Some("ServerlessCacheNotUnderMigrationFault"),
        ),
        (
            "a migration only prepared",
            Some(state::MIGRATION_STATUS_PREPARED),
            400,
            Some("InvalidParameterValue"),
        ),
        (
            "a migration a stop left open",
            Some(state::MIGRATION_STATUS_STOPPED),
            400,
            Some("InvalidParameterValue"),
        ),
        (
            "a migration whose data sync is in flight",
            Some(state::MIGRATION_STATUS_SYNCING),
            200,
            None,
        ),
        (
            "a migration already cutting over",
            Some(state::MIGRATION_STATUS_CUTOVER),
            200,
            None,
        ),
    ];

    for (case, phase, status, code) in rows {
        let emulator = Emulator::new();
        create_settled_cache(&emulator, case, "migrated-cache").await;
        if let Some(phase) = phase {
            open_cache_migration(&emulator, "migrated-cache", phase);
        }

        let reply = complete(&emulator, &[("ServerlessCacheName", "Migrated-Cache")]).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        match code {
            Some(code) => {
                assert_eq!(
                    reply.error_code(),
                    code,
                    "case {case:?}: expected the wire code {code:?}, got {}",
                    reply.body
                );
                // The migration the cache carried is still open: a refusal changes
                // nothing.
                let described = describe_cache(&emulator, "migrated-cache").await;
                assert_eq!(
                    described.text("Migration/Status").map(str::to_string),
                    phase.map(str::to_string),
                    "case {case:?}: the refused cache must still carry the migration it had, \
                     got {}",
                    described.body
                );
            }
            None => {
                assert_eq!(
                    reply.text("ServerlessCacheName"),
                    Some("migrated-cache"),
                    "case {case:?}: the answer must report the cache it completed, got {}",
                    reply.body
                );
                // The migration is closed, so the cache reports none and a second
                // completion is the cache's not-under-migration fault.
                let described = describe_cache(&emulator, "migrated-cache").await;
                assert_eq!(
                    described.text("Migration/Status"),
                    None,
                    "case {case:?}: the completed cache must carry no migration, got {}",
                    described.body
                );
                let again = complete(&emulator, &[("ServerlessCacheName", "migrated-cache")]).await;
                assert_eq!(
                    again.error_code(),
                    "ServerlessCacheNotUnderMigrationFault",
                    "case {case:?}: a second completion must be refused, got {}",
                    again.body
                );
            }
        }
    }
}

/// A migration has ONE target, and a target this account does not hold is that target's
/// own not-found: the rows span both targets and both ways of naming the wrong number of
/// them.
#[tokio::test]
async fn a_completion_names_exactly_one_target_the_account_holds() {
    let emulator = Emulator::new();
    create_settled_group(&emulator, "a group that exists", "held-group", &[]).await;
    create_settled_cache(&emulator, "a cache that exists", "held-cache").await;

    // An id past the identifier bound this service states ("Replication Group ID cannot be
    // longer than 50 characters"), which names no group it could have stored.
    let too_long = "g".repeat(state::LOOKUP_IDENTIFIER_MAX_LEN + 1);
    let rows: Vec<TargetRefusal> = vec![
        (
            "both targets at once",
            vec![
                ("ReplicationGroupId", "held-group"),
                ("ServerlessCacheName", "held-cache"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        ("no target at all", vec![], 400, "InvalidParameterValue"),
        (
            "a replication group this account does not hold",
            vec![("ReplicationGroupId", "never-created-group")],
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "a serverless cache this account does not hold",
            vec![("ServerlessCacheName", "never-created-cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "an id past the identifier bound",
            vec![("ReplicationGroupId", too_long.as_str())],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, pairs, status, code) in rows {
        let refused = complete(&emulator, &pairs).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the wire code {code:?}, got {}",
            refused.body
        );
    }
}

/// The completion is recorded as an event of the group — the other place a state change
/// shows up.
#[tokio::test]
async fn a_completion_is_recorded_as_an_event() {
    let emulator = Emulator::new();
    create_settled_group(&emulator, "the logged group", "logged-migration", &[]).await;
    let started = start_migration(&emulator, "logged-migration").await;
    assert_eq!(
        started.status, 200,
        "StartMigration must answer 200, got {} {}",
        started.status, started.body
    );
    // The cutover is of the migration that start began, and it is served once the service
    // has established that sync — which is what retrying the call waits for.
    let completed = poll_completed(&emulator, "the logged group", "logged-migration").await;
    assert_eq!(
        completed.status, 200,
        "the completion must answer 200, got {} {}",
        completed.status, completed.body
    );

    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceType", "replication-group"),
                ("SourceIdentifier", "logged-migration"),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "the event read must answer 200, got {} {}",
        events.status, events.body
    );
    let messages = events.all("Event/Message");
    assert!(
        messages
            .iter()
            .any(|message| message.to_ascii_lowercase().contains("migration")
                && message.to_ascii_lowercase().contains("completed")),
        "expected an event recording the completion, got {messages:?} from {}",
        events.body
    );
}
