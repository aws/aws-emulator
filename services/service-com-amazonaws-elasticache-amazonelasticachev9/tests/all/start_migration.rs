// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartMigration`.
//!
//! The rule under test: `StartMigration` starts a data sync into the ONE target
//! the request names — the replication group or the serverless cache — reading
//! and writing the single stored copy of that resource, so it refuses with the
//! target's modelled fault when the store says the sync cannot start (the target
//! is not held, is not in a state that admits the change, already carries an open
//! migration, or carries none at all), and otherwise records the migration on
//! that resource and answers with the resource as the store now holds it.
//!
//! The tests are tables: each row is one input along an axis the rule
//! generalises over (which target the request names, how many shards the group
//! has, how many source endpoints the list carries and at which published port
//! bound, which phase an open migration is in, and which side is asked to cut
//! over), so no single recorded request is what makes them pass.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, with the shared `harness` request builder and XML reader. The
//! one exception is the serverless rows: the migration a `StartMigration` starts
//! is opened by `PrepareMigration`, which is still a scaffolded 501 with its own
//! gap issue, so those rows seed the migration through
//! [`crate::harness::Emulator::with_state`] — the same store the handlers read —
//! and then assert over the wire that the answer is computed FROM it.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};

/// A span far longer than any transition this service publishes, for a record a
/// row holds in a non-terminal status on purpose.
const NEVER_SETTLES: Duration = Duration::from_secs(60 * 60);

/// The `CustomerNodeEndpoint` form pairs for one list of source endpoints.
///
/// awsQuery indexes a wrapped list's elements from 1 beneath the member segment,
/// which is the layout the AWS SDKs send (`CustomerNodeEndpointList` declares no
/// `xmlName` on its member, so the segment is the Smithy default `member`).
fn endpoint_params(endpoints: &[(&str, i32)]) -> Vec<(String, String)> {
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

/// Create one replication group with `shards` shards, over the wire, and let the
/// clock settle it at `available`.
async fn create_replication_group(emulator: &Emulator, id: &str, shards: i32) {
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", &format!("{id} group")),
                ("Engine", state::DEFAULT_ENGINE),
                ("CacheNodeType", "cache.m7g.large"),
                ("NumNodeGroups", &shards.to_string()),
                ("ReplicasPerNodeGroup", "1"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {id:?}: the replication group must be creatable, got {} {}",
        created.status, created.body
    );
    // A created group is `creating` until the clock owes it `available`, and a
    // migration can only start on a settled one.
    std::thread::sleep(PAST_TRANSITION);
}

/// Create one serverless cache over the wire and let the clock settle it at
/// `AVAILABLE`.
async fn create_serverless_cache(emulator: &Emulator, name: &str) {
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", name), ("Engine", "valkey")]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the serverless cache must be creatable, got {} {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// Open a migration on the stored serverless cache `name`, in `status`.
///
/// The operation that opens one (`PrepareMigration`) is still a scaffolded 501,
/// so the record is seeded into the store the handlers read rather than through
/// the wire.
fn open_migration(emulator: &Emulator, name: &str, status: &str) {
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

/// Hold the stored serverless cache `name` in `status`, with nothing the clock
/// owes it due inside this test.
fn hold_cache_status(emulator: &Emulator, name: &str, status: &str) {
    emulator.with_state(|store| {
        let cache = store
            .serverless_caches
            .get_mut(name)
            .unwrap_or_else(|| panic!("case {name:?}: the cache just created must be stored"));
        cache.status = status.to_string();
        cache.pending_status = Some(state::PendingTransition {
            due: SystemTime::now() + NEVER_SETTLES,
            status: state::SERVERLESS_STATUS_AVAILABLE.to_string(),
        });
    });
}

/// Read one replication group back by id, asserting the describe succeeded.
async fn describe_group(emulator: &Emulator, id: &str) -> Reply {
    let described = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {id:?}: DescribeReplicationGroups must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Read one serverless cache back by name, asserting the describe succeeded.
async fn describe_cache(emulator: &Emulator, name: &str) -> Reply {
    let described = emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {name:?}: DescribeServerlessCaches must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Assert one refusal row: the HTTP status and the `awsQuery` error code.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case:?}: expected HTTP {status} {code}, got HTTP {} {:?} — {}",
        reply.status,
        reply.error_code(),
        reply.body
    );
}

// ───────────────── the migration into a replication group ────────────────────

/// One accepted start into a replication group.
#[derive(Debug, Clone, Copy)]
struct GroupCase {
    case: &'static str,
    id: &'static str,
    /// How many shards the group is created with: more than one is a cluster
    /// mode enabled group.
    shards: i32,
    /// The source endpoints the list carries, address and port.
    endpoints: &'static [(&'static str, i32)],
}

/// The axis: how many shards the target has, how many source endpoints the list
/// carries, and the port each one names — including both published bounds of a
/// TCP port.
const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        case: "cluster mode disabled, one source endpoint on the engine's port",
        id: "unsharded-target",
        shards: 1,
        endpoints: &[("one.source.example.internal", 6379)],
    },
    GroupCase {
        case: "cluster mode enabled, two source endpoints",
        id: "sharded-target",
        shards: 2,
        endpoints: &[
            ("first.source.example.internal", 6379),
            ("second.source.example.internal", 6380),
        ],
    },
    GroupCase {
        // The list is not required to have one element per shard, and a port is
        // any port a TCP endpoint can carry: the lowest and the highest are both
        // accepted.
        case: "cluster mode enabled, source endpoints at both port bounds",
        id: "port-bound-target",
        shards: 3,
        endpoints: &[
            ("low.source.example.internal", 1),
            ("high.source.example.internal", 65_535),
        ],
    },
];

#[tokio::test]
async fn a_start_into_a_replication_group_records_the_migration_and_moves_the_group_to_modifying() {
    for case in GROUP_CASES {
        let emulator = Emulator::new();
        create_replication_group(&emulator, case.id, case.shards).await;

        let mut request = params(&[("ReplicationGroupId", case.id)]);
        request.extend(endpoint_params(case.endpoints));
        let started = emulator.call("StartMigration", &request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartMigration must answer 200, got {} {}",
            case.case, started.status, started.body
        );
        assert_eq!(
            started.text("ReplicationGroupId"),
            Some(case.id),
            "case {:?}: the answer must be the group the request named, got {:?}",
            case.case,
            started.text("ReplicationGroupId")
        );
        // Syncing data into the group is work the service carries out after the
        // call answers, so the group reports the transient status.
        assert_eq!(
            started.text("Status"),
            Some(state::STATUS_MODIFYING),
            "case {:?}: the group must answer {:?}, got {:?}",
            case.case,
            state::STATUS_MODIFYING,
            started.text("Status")
        );
        // The answer is not a value the handler made up for this response: the
        // stored group carries the migration, and the read that follows is
        // answered once the sync the clock owed is done — the migration it
        // carries stays open, so a second start is still refused once it has
        // settled.
        let settled = describe_group(&emulator, case.id).await;
        assert_eq!(
            settled.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {:?}: the clock must settle the group at {:?}, got {:?}",
            case.case,
            state::STATUS_AVAILABLE,
            settled.text("Status")
        );
        let again = emulator.call("StartMigration", &request).await;
        assert_refused(
            case.case,
            &again,
            400,
            "ReplicationGroupAlreadyUnderMigrationFault",
        );
        let expected = format!("Replication group {} is already under migration.", case.id);
        assert_eq!(
            again.error_message(),
            expected,
            "case {:?}: the fault must name the group it refused, got {:?}",
            case.case,
            again.error_message()
        );
    }
}

#[tokio::test]
async fn a_start_the_store_cannot_serve_is_refused_with_the_targets_modelled_fault() {
    let emulator = Emulator::new();
    create_replication_group(&emulator, "settled-group", 1).await;
    create_replication_group(&emulator, "migrating-group", 1).await;
    create_replication_group(&emulator, "creating-group", 1).await;
    create_serverless_cache(&emulator, "idle-cache").await;

    // `migrating-group` carries an open migration, and has settled back to
    // `available` since: the row below is refused for the migration it carries
    // and not for a transient status.
    let opened = emulator
        .call(
            "StartMigration",
            &[
                params(&[("ReplicationGroupId", "migrating-group")]),
                endpoint_params(&[("source.example.internal", 6379)]),
            ]
            .concat(),
        )
        .await;
    assert_eq!(
        opened.status, 200,
        "the migration the table needs must start, got {} {}",
        opened.status, opened.body
    );
    std::thread::sleep(PAST_TRANSITION);
    // `creating-group` is held in `creating` for the whole table rather than
    // raced against the transition span.
    emulator.with_state(|store| {
        let group = store
            .replication_groups
            .get_mut("creating-group")
            .expect("the group just created must be stored");
        group.status = state::STATUS_CREATING.to_string();
        group.pending_status = Some(state::PendingTransition {
            due: SystemTime::now() + NEVER_SETTLES,
            status: state::STATUS_AVAILABLE.to_string(),
        });
    });

    // The axis: every reason the store refuses a start, with the endpoint list
    // and the target each one names.
    let cases: &[RefusalCase] = &[
        (
            "no target at all",
            &[
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "both targets at once",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("ServerlessCacheName", "idle-cache"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a serverless member named beside a replication group",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("AutoCutover", "true"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a replication group this account does not hold",
            &[
                ("ReplicationGroupId", "no-such-group"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "a replication group that is not available yet",
            &[
                ("ReplicationGroupId", "creating-group"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            400,
            "InvalidReplicationGroupState",
        ),
        (
            "a replication group that already carries a migration",
            &[
                ("ReplicationGroupId", "migrating-group"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            400,
            "ReplicationGroupAlreadyUnderMigrationFault",
        ),
        (
            "no endpoint to migrate from",
            &[("ReplicationGroupId", "settled-group")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an endpoint with no address",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("CustomerNodeEndpointList.member.1.Address", "   "),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an endpoint with no port",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a port below the lowest one",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "0"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a port past the highest one",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("CustomerNodeEndpointList.member.1.Address", "s.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "65536"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "two source endpoints for an unsharded group",
            &[
                ("ReplicationGroupId", "settled-group"),
                ("CustomerNodeEndpointList.member.1.Address", "first.example.internal"),
                ("CustomerNodeEndpointList.member.1.Port", "6379"),
                ("CustomerNodeEndpointList.member.2.Address", "second.example.internal"),
                ("CustomerNodeEndpointList.member.2.Port", "6380"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a serverless cache this account does not hold",
            &[("ServerlessCacheName", "no-such-cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "a serverless cache no migration was prepared on",
            &[("ServerlessCacheName", "idle-cache")],
            400,
            "ServerlessCacheNotUnderMigrationFault",
        ),
    ];

    for (case, request, status, code) in cases {
        let refused = emulator.call("StartMigration", &params(request)).await;
        assert_refused(case, &refused, *status, code);
    }

    // The list a request stores on the migration is bounded: one source per
    // shard of the target's keyspace, which the service's shard maximum bounds.
    // The bound itself is accepted and one past it is refused.
    let at_bound: Vec<(&str, i32)> = (0..state::MAX_SOURCE_ENDPOINTS)
        .map(|_| ("bulk.source.example.internal", 6379))
        .collect();
    let past_bound: Vec<(&str, i32)> = (0..state::MAX_SOURCE_ENDPOINTS + 1)
        .map(|_| ("bulk.source.example.internal", 6379))
        .collect();
    create_replication_group(&emulator, "bulk-source-group", 2).await;
    let refused = emulator
        .call(
            "StartMigration",
            &[
                params(&[("ReplicationGroupId", "bulk-source-group")]),
                endpoint_params(&past_bound),
            ]
            .concat(),
        )
        .await;
    assert_refused(
        "one source endpoint past the bound",
        &refused,
        400,
        "InvalidParameterValue",
    );
    let accepted = emulator
        .call(
            "StartMigration",
            &[
                params(&[("ReplicationGroupId", "bulk-source-group")]),
                endpoint_params(&at_bound),
            ]
            .concat(),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: the bound itself must be accepted, got {} {}",
        "source endpoints at the bound", accepted.status, accepted.body
    );

    // Every refusal above wrote nothing: the group that was available still is,
    // and the one that is not available still carries the status it was held in.
    let settled = describe_group(&emulator, "settled-group").await;
    assert_eq!(
        settled.text("Status"),
        Some(state::STATUS_AVAILABLE),
        "a refused start must leave the group it named alone, got {:?}",
        settled.text("Status")
    );
    let creating = describe_group(&emulator, "creating-group").await;
    assert_eq!(
        creating.text("Status"),
        Some(state::STATUS_CREATING),
        "a refused start must leave the group it named alone, got {:?}",
        creating.text("Status")
    );
    // A start refused on a group still carrying no migration leaves it startable,
    // which is what proves the refusals did not record one.
    let accepted = emulator
        .call(
            "StartMigration",
            &[
                params(&[("ReplicationGroupId", "settled-group")]),
                endpoint_params(&[("source.example.internal", 6379)]),
            ]
            .concat(),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "the group no refusal recorded a migration on must still accept one, got {} {}",
        accepted.status, accepted.body
    );
}

// ─────────────────── the migration into a serverless cache ───────────────────

/// One start refused because of the phase the open migration, or the cache
/// itself, is in.
#[derive(Debug, Clone, Copy)]
struct PhaseCase {
    case: &'static str,
    name: &'static str,
    /// The phase the open migration is in when the start arrives.
    opened_in: &'static str,
    /// The status the cache is held in.
    cache_status: &'static str,
    /// The pairs the start sends beside the cache name.
    request: &'static [(&'static str, &'static str)],
    /// The `awsQuery` error code the refusal must carry.
    code: &'static str,
}

/// One accepted start into a serverless cache.
#[derive(Debug, Clone, Copy)]
struct CacheCase {
    case: &'static str,
    name: &'static str,
    /// The phase the open migration is in when the start arrives.
    opened_in: &'static str,
    /// The pairs the start sends beside the cache name.
    request: &'static [(&'static str, &'static str)],
    /// The cutover mode the migration must report afterwards.
    cutover_mode: &'static str,
}

/// The axis: the phase the open migration is in, and which side the request asks
/// to carry out the cutover.
const CACHE_CASES: &[CacheCase] = &[
    CacheCase {
        case: "a prepared migration, cutover left to the caller",
        name: "prepared-cache",
        opened_in: state::MIGRATION_STATUS_PREPARED,
        request: &[],
        cutover_mode: state::CUTOVER_MODE_MANUAL,
    },
    CacheCase {
        case: "a prepared migration, cutover asked of the service",
        name: "auto-cutover-cache",
        opened_in: state::MIGRATION_STATUS_PREPARED,
        request: &[("AutoCutover", "true")],
        cutover_mode: state::CUTOVER_MODE_AUTOMATIC,
    },
    CacheCase {
        case: "a prepared migration, cutover declined explicitly",
        name: "manual-cutover-cache",
        opened_in: state::MIGRATION_STATUS_PREPARED,
        request: &[("AutoCutover", "false")],
        cutover_mode: state::CUTOVER_MODE_MANUAL,
    },
    CacheCase {
        // "After StopMigration, StartMigration requires this option to restart
        // the data sync."
        case: "a stopped migration restarted with the target's data cleared",
        name: "restarted-cache",
        opened_in: state::MIGRATION_STATUS_STOPPED,
        request: &[("ClearTargetCacheData", "true"), ("AutoCutover", "true")],
        cutover_mode: state::CUTOVER_MODE_AUTOMATIC,
    },
];

#[tokio::test]
async fn a_start_on_a_prepared_serverless_migration_syncs_and_the_cache_reports_it() {
    for case in CACHE_CASES {
        let emulator = Emulator::new();
        create_serverless_cache(&emulator, case.name).await;
        open_migration(&emulator, case.name, case.opened_in);

        let mut request = params(&[("ServerlessCacheName", case.name)]);
        request.extend(params(case.request));
        let started = emulator.call("StartMigration", &request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartMigration must answer 200, got {} {}",
            case.case, started.status, started.body
        );
        assert_eq!(
            started.text("ServerlessCacheName"),
            Some(case.name),
            "case {:?}: the answer must be the cache the request named, got {:?}",
            case.case,
            started.text("ServerlessCacheName")
        );
        // The cache keeps serving while the sync runs, so its own status is
        // unchanged and only the migration moves.
        assert_eq!(
            started.text("ServerlessCache/Status"),
            Some(state::SERVERLESS_STATUS_AVAILABLE),
            "case {:?}: the cache must stay {:?}, got {:?}",
            case.case,
            state::SERVERLESS_STATUS_AVAILABLE,
            started.text("ServerlessCache/Status")
        );

        for (label, reply) in [
            ("the answer", &started),
            ("a later read", &describe_cache(&emulator, case.name).await),
        ] {
            assert_eq!(
                reply.text("Migration/Status"),
                Some(state::MIGRATION_STATUS_SYNCING),
                "case {:?}: {label} must report the migration {:?}, got {:?} — {}",
                case.case,
                state::MIGRATION_STATUS_SYNCING,
                reply.text("Migration/Status"),
                reply.body
            );
            assert_eq!(
                reply.text("CutoverMode"),
                Some(case.cutover_mode),
                "case {:?}: {label} must report cutover mode {:?}, got {:?}",
                case.case,
                case.cutover_mode,
                reply.text("CutoverMode")
            );
            // "The migration endpoint, which is the standard endpoint of the
            // serverless cache": the same address and port the cache reports,
            // not a second name.
            assert_eq!(
                reply.text("MigrationEndpoint/Address"),
                reply.text("Endpoint/Address"),
                "case {:?}: {label} must report the cache's own endpoint as the migration \
                 endpoint, got {:?} against {:?}",
                case.case,
                reply.text("MigrationEndpoint/Address"),
                reply.text("Endpoint/Address")
            );
            assert_eq!(
                reply.text("MigrationEndpoint/Port"),
                reply.text("Endpoint/Port"),
                "case {:?}: {label} must report the cache's own port as the migration endpoint's, \
                 got {:?} against {:?}",
                case.case,
                reply.text("MigrationEndpoint/Port"),
                reply.text("Endpoint/Port")
            );
        }
    }
}

#[tokio::test]
async fn a_serverless_start_the_migrations_phase_does_not_admit_is_refused_and_writes_nothing() {
    // The axis: the phase the open migration is in, the status the cache is held
    // in, and what the request says about the data already in the target.
    let cases: &[PhaseCase] = &[
        PhaseCase {
            case: "a sync that is already running",
            name: "syncing-cache",
            opened_in: state::MIGRATION_STATUS_SYNCING,
            cache_status: state::SERVERLESS_STATUS_AVAILABLE,
            request: &[],
            code: "InvalidServerlessCacheStateFault",
        },
        PhaseCase {
            case: "a migration that is cutting over",
            name: "cutover-cache",
            opened_in: state::MIGRATION_STATUS_CUTOVER,
            cache_status: state::SERVERLESS_STATUS_AVAILABLE,
            request: &[],
            code: "InvalidServerlessCacheStateFault",
        },
        PhaseCase {
            case: "a cache that is not available yet",
            name: "creating-cache",
            opened_in: state::MIGRATION_STATUS_PREPARED,
            cache_status: state::SERVERLESS_STATUS_CREATING,
            request: &[],
            code: "InvalidServerlessCacheStateFault",
        },
        PhaseCase {
            case: "a stopped sync restarted without clearing the target",
            name: "stopped-cache",
            opened_in: state::MIGRATION_STATUS_STOPPED,
            cache_status: state::SERVERLESS_STATUS_AVAILABLE,
            request: &[],
            code: "InvalidParameterValue",
        },
        PhaseCase {
            case: "a stopped sync restarted keeping the target's data",
            name: "kept-data-cache",
            opened_in: state::MIGRATION_STATUS_STOPPED,
            cache_status: state::SERVERLESS_STATUS_AVAILABLE,
            request: &[("ClearTargetCacheData", "false")],
            code: "InvalidParameterValue",
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        create_serverless_cache(&emulator, case.name).await;
        open_migration(&emulator, case.name, case.opened_in);
        if case.cache_status != state::SERVERLESS_STATUS_AVAILABLE {
            hold_cache_status(&emulator, case.name, case.cache_status);
        }

        let mut request = params(&[("ServerlessCacheName", case.name)]);
        request.extend(params(case.request));
        let refused = emulator.call("StartMigration", &request).await;
        assert_refused(case.case, &refused, 400, case.code);

        // A refused start leaves the migration in the phase it was in: nothing
        // about the stored record changed.
        let described = describe_cache(&emulator, case.name).await;
        assert_eq!(
            described.text("Migration/Status"),
            Some(case.opened_in),
            "case {:?}: a refused start must leave the migration in {:?}, got {:?}",
            case.case,
            case.opened_in,
            described.text("Migration/Status")
        );
        // A describe reports the cache inside the list element, so the cache's
        // own `Status` is the FIRST one in the document: the shape carries it
        // well before the migration's.
        let statuses = described.all("Status");
        assert_eq!(
            statuses.first().copied(),
            Some(case.cache_status),
            "case {:?}: a refused start must leave the cache in {:?}, got {statuses:?}",
            case.case,
            case.cache_status
        );
    }
}
