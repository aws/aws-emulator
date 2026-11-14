// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteServerlessCache`.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from persisted state cannot pass. The request
//! builder and the XML reader are the shared `harness` ones, so this target
//! keeps one harness rather than a copy that could drift.
//!
//! The rule under test: a delete resolves the serverless cache the request
//! names (folded to the stored, lowercased identity), refuses one that is not in
//! its settled usable status with the modelled state fault and one that names no
//! stored cache with the modelled not-found fault, and otherwise reports the
//! cache it is taking away as `DELETING` and lets the clock remove it — after
//! which every read of it answers the not-found fault.
//!
//! The tests are tables: each row is one input along the axis the rule
//! generalises over (the engine and the members the cache carries, the case the
//! name is sent in, which status the cache is in when the delete arrives, and
//! whether a final snapshot is asked for), so no single recorded request is what
//! makes them pass.
//!
//! The final snapshot a delete takes is the snapshot rule, so what it stores is
//! asserted by the `serverless_cache_snapshots` suite beside this one; the row
//! below is only about a `FinalSnapshotName` that cannot name a snapshot at all.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};

/// One row of the delete table: a cache to create, and the identity it is stored
/// and reported under.
#[derive(Debug, Clone, Copy)]
struct DeleteCase {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The `ServerlessCacheName` the requests send.
    request_name: &'static str,
    /// The identity the cache is stored under — `ServerlessCacheName` "is
    /// stored as a lowercase string".
    stored_name: &'static str,
    /// The engine the cache runs, which the delete's response reports back.
    engine: &'static str,
    /// The form pairs sent beside the name on the create.
    extra: &'static [(&'static str, &'static str)],
}

/// The axis: the engine, the members the cache carries, and the case its name is
/// sent in. A delete holds over all of them, so every row is deleted the same
/// way.
const CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "redis, nothing but the engine",
        request_name: "plain-redis-cache",
        stored_name: "plain-redis-cache",
        engine: "redis",
        extra: &[("Engine", "redis")],
    },
    DeleteCase {
        case: "valkey, a described cache named in mixed case",
        request_name: "Valkey-Cache",
        stored_name: "valkey-cache",
        engine: "valkey",
        extra: &[("Engine", "valkey"), ("Description", "the valkey one")],
    },
    DeleteCase {
        case: "memcached, snapshot members set",
        request_name: "MEMCACHED-CACHE",
        stored_name: "memcached-cache",
        engine: "memcached",
        extra: &[
            ("Engine", "memcached"),
            ("SnapshotRetentionLimit", "7"),
            ("DailySnapshotTime", "03:00"),
        ],
    },
    DeleteCase {
        case: "redis, a customer-managed key and a security group",
        request_name: "kms-redis-cache",
        stored_name: "kms-redis-cache",
        engine: "redis",
        extra: &[
            ("Engine", "redis"),
            (
                "KmsKeyId",
                "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
            ),
            ("SecurityGroupIds.member.1", "sg-0123456789abcdef0"),
        ],
    },
];

/// Create one serverless cache and insist the create itself succeeded.
async fn create_cache(emulator: &Emulator, case: &str, name: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("ServerlessCacheName", name)]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateServerlessCache", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );
    created
}

/// `DeleteServerlessCache` with the pairs a row sends.
async fn delete_cache(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator
        .call("DeleteServerlessCache", &params(pairs))
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

/// Assert `reply` is the modelled refusal `code` with HTTP `status`.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case}: expected HTTP {status}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case}: expected error code {code:?}, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
}

#[tokio::test]
async fn a_delete_reports_the_cache_as_deleting_and_the_clock_removes_it() {
    let emulator = Emulator::new();

    // Every row's cache, plus one that is never deleted: a delete takes away the
    // cache it names and nothing else.
    let mut created = Vec::new();
    for case in CASES {
        created.push(
            create_cache(&emulator, case.case, case.request_name, case.extra)
                .await
                .text("ARN")
                .map(str::to_string)
                .unwrap_or_else(|| panic!("case {}: the create must report an ARN", case.case)),
        );
    }
    create_cache(&emulator, "survivor", "survivor-cache", &[("Engine", "redis")]).await;

    // Read every cache back: the read is answered once the create has settled
    // it, and `available` is the status a delete is accepted from.
    for case in CASES {
        let described = describe_cache(&emulator, case.request_name).await;
        assert_eq!(
            described.text("Status"),
            Some(state::SERVERLESS_STATUS_AVAILABLE),
            "case {}: the read after the create must report the cache available, got {}",
            case.case,
            described.body
        );
    }

    for (case, arn) in CASES.iter().zip(&created) {
        let deleted = delete_cache(
            &emulator,
            &[("ServerlessCacheName", case.request_name)],
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {}: DeleteServerlessCache must answer 200, got {} {}",
            case.case, deleted.status, deleted.body
        );
        for (member, expected) in [
            ("ServerlessCacheName", case.stored_name),
            ("Status", state::SERVERLESS_STATUS_DELETING),
            ("Engine", case.engine),
            ("ARN", arn.as_str()),
        ] {
            assert_eq!(
                deleted.text(member),
                Some(expected),
                "case {}: expected {member} {expected:?} in the delete's response, got {:?}",
                case.case,
                deleted.text(member)
            );
        }

        // The read that follows is answered once the removal the clock owed has
        // been carried through, so the cache is gone from it — and a cache the
        // store no longer holds cannot be deleted again.
        let gone = describe_cache(&emulator, case.request_name).await;
        assert_refused(case.case, &gone, 404, "ServerlessCacheNotFoundFault");
        let again = delete_cache(
            &emulator,
            &[("ServerlessCacheName", case.request_name)],
        )
        .await;
        assert_refused(case.case, &again, 404, "ServerlessCacheNotFoundFault");
    }

    // The list then holds only the cache nothing deleted.
    let listed = emulator
        .call("DescribeServerlessCaches", &params(&[]))
        .await;
    assert_eq!(
        listed.status, 200,
        "DescribeServerlessCaches must answer 200, got {} {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.all("ServerlessCacheName"),
        vec!["survivor-cache"],
        "only the cache nothing deleted may be left, got {}",
        listed.body
    );
    let survivor = describe_cache(&emulator, "survivor-cache").await;
    assert_eq!(
        survivor.text("Status"),
        Some(state::SERVERLESS_STATUS_AVAILABLE),
        "the cache nothing deleted must be untouched, got {:?}",
        survivor.text("Status")
    );
}

#[tokio::test]
async fn a_delete_this_emulator_cannot_honour_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    create_cache(&emulator, "available", "available-cache", &[("Engine", "redis")]).await;
    // Created first and waited out, so the cache the refusal rows name is in its
    // settled, deletable status before the row that is about to be refused runs.
    std::thread::sleep(PAST_TRANSITION);
    // This one is still inside its create span while the table below runs.
    create_cache(&emulator, "creating", "creating-cache", &[("Engine", "valkey")]).await;

    // The axis: why the delete cannot be honoured — the name resolves no cache
    // (in three forms), the cache is not in a status it can be deleted from, or
    // the final snapshot it asks for cannot be named.
    let rows: &[RefusalCase] = &[
        (
            "a name no cache was created under",
            &[("ServerlessCacheName", "no-such-cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "the same absent name in mixed case",
            &[("ServerlessCacheName", "No-Such-Cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "a name that identifies nothing at all",
            &[("ServerlessCacheName", "")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
        (
            "a cache still inside its create span",
            &[("ServerlessCacheName", "creating-cache")],
            400,
            "InvalidServerlessCacheStateFault",
        ),
        (
            "a final snapshot name no snapshot can be stored under",
            &[
                ("ServerlessCacheName", "available-cache"),
                ("FinalSnapshotName", "not a name"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, pairs, status, code) in rows {
        let refused = delete_cache(&emulator, pairs).await;
        assert_refused(case, &refused, *status, code);
    }

    // Nothing the table asked for happened: both stored caches are still there,
    // the one a row named is still available, and no cache was minted.
    let listed = emulator
        .call("DescribeServerlessCaches", &params(&[]))
        .await;
    assert_eq!(
        listed.all("ServerlessCacheName"),
        vec!["available-cache", "creating-cache"],
        "a refused delete must leave the store alone, got {}",
        listed.body
    );
    let untouched = describe_cache(&emulator, "available-cache").await;
    assert_eq!(
        untouched.text("Status"),
        Some(state::SERVERLESS_STATUS_AVAILABLE),
        "the cache a refused delete named must still be available, got {:?}",
        untouched.text("Status")
    );

    // A delete that IS honoured cannot be honoured twice: inside the deletion
    // span the cache is in a status no delete is accepted from, and past it the
    // cache is gone.
    let first = delete_cache(&emulator, &[("ServerlessCacheName", "available-cache")]).await;
    assert_eq!(
        first.status, 200,
        "the first delete must answer 200, got {} {}",
        first.status, first.body
    );
    let second = delete_cache(&emulator, &[("ServerlessCacheName", "available-cache")]).await;
    assert_refused(
        "a delete of a cache already deleting",
        &second,
        400,
        "InvalidServerlessCacheStateFault",
    );
    std::thread::sleep(PAST_TRANSITION);
    let third = delete_cache(&emulator, &[("ServerlessCacheName", "available-cache")]).await;
    assert_refused(
        "a delete of a cache the clock has removed",
        &third,
        404,
        "ServerlessCacheNotFoundFault",
    );
}

#[tokio::test]
async fn a_deleted_cache_leaves_its_user_group_and_is_recorded_in_the_event_log() {
    let emulator = Emulator::new();
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "delete-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "CreateUserGroup must answer 200, got {} {}",
        group.status, group.body
    );
    // The group has to be usable before a cache can be attached to it.
    std::thread::sleep(PAST_TRANSITION);
    create_cache(
        &emulator,
        "grouped",
        "grouped-cache",
        &[("Engine", "redis"), ("UserGroupId", "delete-group")],
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);

    let serving = emulator
        .call(
            "DescribeUserGroups",
            &params(&[("UserGroupId", "delete-group")]),
        )
        .await;
    assert_eq!(
        serving.all("ServerlessCaches/member"),
        vec!["grouped-cache"],
        "the group must report the cache it serves, got {}",
        serving.body
    );

    let deleted = delete_cache(&emulator, &[("ServerlessCacheName", "grouped-cache")]).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteServerlessCache must answer 200, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.text("UserGroupId"),
        Some("delete-group"),
        "the delete must report the cache's stored user group, got {:?}",
        deleted.text("UserGroupId")
    );

    // Deleting a serverless cache is one of the events this service records, and
    // the log outlives the record it is about.
    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceIdentifier", "grouped-cache"),
                ("SourceType", state::SOURCE_TYPE_SERVERLESS_CACHE),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "DescribeEvents must answer 200, got {} {}",
        events.status, events.body
    );
    assert!(
        events
            .all("Message")
            .iter()
            .any(|message| message.contains("deleted")),
        "the delete must be recorded in the event log, got {}",
        events.body
    );

    // Past the deletion span the cache is gone, and the group no longer reports
    // a cache that does not exist.
    std::thread::sleep(PAST_TRANSITION);
    let gone = describe_cache(&emulator, "grouped-cache").await;
    assert_refused("a removed cache", &gone, 404, "ServerlessCacheNotFoundFault");
    let after = emulator
        .call(
            "DescribeUserGroups",
            &params(&[("UserGroupId", "delete-group")]),
        )
        .await;
    assert!(
        after.all("ServerlessCaches/member").is_empty(),
        "the group must not report a cache the clock removed, got {}",
        after.body
    );
}
