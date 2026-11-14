// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the serverless cache snapshot:
//! `CreateServerlessCacheSnapshot` and the `DescribeServerlessCacheSnapshots`
//! that reads one back, plus the final snapshot a `DeleteServerlessCache` takes.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, using the shared `harness` request builder and XML reader —
//! so a response that is not computed from persisted state cannot pass, and this
//! target keeps one harness rather than a copy that could drift.
//!
//! The rule under test: a create copies the serverless cache the request names
//! into a snapshot resource of its own, keyed by the snapshot's lowercased name
//! and holding the cache's name, engine and major engine version as they stood
//! at that instant; it answers the snapshot as `creating` and the clock owes it
//! `available`; and it refuses a request whose source cache the account does not
//! hold with the modelled not-found fault, whichever name it asked for. The
//! snapshot outlives the cache, which is why a read of it still answers after
//! the cache is gone.
//!
//! The tests are tables: each row is one input along an axis the rule
//! generalises over (the engine the source cache runs, the case and the length
//! of the snapshot's name, which key encrypts it, whether it carries tags, and —
//! for the refusals — every way a name can fail to name a stored cache), so no
//! single recorded request is what makes them pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION, REGION};

/// One row of the snapshot table: a serverless cache to create, and the snapshot
/// taken of it.
#[derive(Debug, Clone, Copy)]
struct SnapshotCase {
    /// What the row is called in a failure message.
    case: &'static str,
    /// The `ServerlessCacheName` the create sends, and the identity it is stored
    /// under — the name "is stored as a lowercase string".
    cache_request_name: &'static str,
    cache_stored_name: &'static str,
    /// The engine the source cache runs, which the snapshot's configuration
    /// reports back.
    engine: &'static str,
    /// The form pairs sent beside the name and the engine on the cache create.
    cache_extra: &'static [(&'static str, &'static str)],
    /// The `ServerlessCacheSnapshotName` the snapshot create sends, and the
    /// identity it is stored under.
    snapshot_request_name: &'static str,
    snapshot_stored_name: &'static str,
    /// The form pairs sent beside the two names on the snapshot create.
    snapshot_extra: &'static [(&'static str, &'static str)],
    /// The `KmsKeyId` the snapshot must report: the one its own request named,
    /// else the source cache's, else none at all.
    kms_key_id: Option<&'static str>,
    /// The tags the snapshot must carry, as `(key, value)` pairs.
    tags: &'static [(&'static str, &'static str)],
}

/// A customer managed key, in the ARN form `KmsKeyId` accepts. KMS is not
/// mounted beside this service in these tests, so the identifier a request names
/// is what is stored and reported.
const CACHE_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";
/// See [`CACHE_KEY`]: a second key, named by a snapshot's own request.
const SNAPSHOT_KEY: &str =
    "arn:aws:kms:us-east-1:000000000000:key/66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A 64-character snapshot name: `ServerlessCacheSnapshotName` "must be between
/// 1 and 255 characters", so a long name is as good a name as a short one.
const LONG_SNAPSHOT_NAME: &str =
    "a-serverless-cache-snapshot-name-of-sixty-four-characters-exactl";

/// The axis: the engine the source cache runs, the case and length of the
/// snapshot's name, which key encrypts the snapshot, and whether it carries
/// tags. The create rule holds over all of them, so every row is snapshotted and
/// read back the same way.
const CASES: &[SnapshotCase] = &[
    SnapshotCase {
        case: "redis, nothing but the two names",
        cache_request_name: "redis-cache",
        cache_stored_name: "redis-cache",
        engine: "redis",
        cache_extra: &[],
        snapshot_request_name: "redis-snapshot",
        snapshot_stored_name: "redis-snapshot",
        snapshot_extra: &[],
        kms_key_id: None,
        tags: &[],
    },
    SnapshotCase {
        case: "valkey, the snapshot named in mixed case",
        cache_request_name: "Valkey-Cache",
        cache_stored_name: "valkey-cache",
        engine: "valkey",
        cache_extra: &[("Description", "the valkey one")],
        snapshot_request_name: "Valkey-Snapshot",
        snapshot_stored_name: "valkey-snapshot",
        snapshot_extra: &[],
        kms_key_id: None,
        tags: &[],
    },
    SnapshotCase {
        case: "memcached, a single-character snapshot name",
        cache_request_name: "MEMCACHED-CACHE",
        cache_stored_name: "memcached-cache",
        engine: "memcached",
        cache_extra: &[("SnapshotRetentionLimit", "7"), ("DailySnapshotTime", "03:00")],
        snapshot_request_name: "X",
        snapshot_stored_name: "x",
        snapshot_extra: &[],
        kms_key_id: None,
        tags: &[],
    },
    SnapshotCase {
        case: "redis, a long snapshot name and tags",
        cache_request_name: "tagged-cache",
        cache_stored_name: "tagged-cache",
        engine: "redis",
        cache_extra: &[],
        snapshot_request_name: LONG_SNAPSHOT_NAME,
        snapshot_stored_name: LONG_SNAPSHOT_NAME,
        snapshot_extra: &[
            ("Tags.Tag.1.Key", "team"),
            ("Tags.Tag.1.Value", "platform"),
            ("Tags.Tag.2.Key", "stage"),
            ("Tags.Tag.2.Value", "test"),
        ],
        kms_key_id: None,
        tags: &[("team", "platform"), ("stage", "test")],
    },
    SnapshotCase {
        case: "valkey, the source cache's customer managed key",
        cache_request_name: "encrypted-cache",
        cache_stored_name: "encrypted-cache",
        engine: "valkey",
        cache_extra: &[("KmsKeyId", CACHE_KEY)],
        snapshot_request_name: "inherited-key-snapshot",
        snapshot_stored_name: "inherited-key-snapshot",
        snapshot_extra: &[],
        kms_key_id: Some(CACHE_KEY),
        tags: &[],
    },
    SnapshotCase {
        case: "valkey, a key named by the snapshot's own request",
        cache_request_name: "own-key-cache",
        cache_stored_name: "own-key-cache",
        engine: "valkey",
        cache_extra: &[("KmsKeyId", CACHE_KEY)],
        snapshot_request_name: "own-key-snapshot",
        snapshot_stored_name: "own-key-snapshot",
        snapshot_extra: &[("KmsKeyId", SNAPSHOT_KEY)],
        kms_key_id: Some(SNAPSHOT_KEY),
        tags: &[],
    },
];

/// Create one serverless cache and insist the create itself succeeded.
async fn create_cache(
    emulator: &Emulator,
    case: &str,
    name: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[("ServerlessCacheName", name), ("Engine", engine)]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateServerlessCache", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );
    created
}

/// `CreateServerlessCacheSnapshot` with the pairs a row sends.
async fn create_snapshot(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator
        .call("CreateServerlessCacheSnapshot", &params(pairs))
        .await
}

/// `DescribeServerlessCacheSnapshots` with the pairs a row sends.
async fn describe_snapshots(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator
        .call("DescribeServerlessCacheSnapshots", &params(pairs))
        .await
}

/// Read one snapshot back by name, asserting the describe itself succeeded.
async fn describe_snapshot(emulator: &Emulator, case: &str, name: &str) -> Reply {
    let described =
        describe_snapshots(emulator, &[("ServerlessCacheSnapshotName", name)]).await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeServerlessCacheSnapshots must answer 200, got {} {}",
        described.status, described.body
    );
    described
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

/// The members a snapshot must report for `case`, checked on whichever response
/// carries it — the create's or a later describe's. `status` is the one the
/// snapshot is expected to be in at that point.
fn assert_snapshot(case: &SnapshotCase, reply: &Reply, status: &str, major_engine_version: &str) {
    for (member, expected) in [
        ("ServerlessCacheSnapshotName", case.snapshot_stored_name),
        ("Status", status),
        ("SnapshotType", state::SERVERLESS_SNAPSHOT_TYPE_MANUAL),
        // The configuration is the SOURCE CACHE's, copied at the instant the
        // snapshot was taken.
        ("ServerlessCacheName", case.cache_stored_name),
        ("Engine", case.engine),
        ("MajorEngineVersion", major_engine_version),
    ] {
        assert_eq!(
            reply.text(member),
            Some(expected),
            "case {}: expected {member} {expected:?}, got {:?} ({})",
            case.case,
            reply.text(member),
            reply.body
        );
    }
    // The ARN names this snapshot, in this Region, under the resource type an
    // ElastiCache serverless cache snapshot carries.
    let arn = reply
        .text("ARN")
        .unwrap_or_else(|| panic!("case {}: the snapshot must report an ARN", case.case));
    for fragment in [
        "arn:",
        &format!(":{REGION}:"),
        &format!(":serverlesscachesnapshot:{}", case.snapshot_stored_name),
    ] {
        assert!(
            arn.contains(fragment),
            "case {}: the ARN {arn:?} must contain {fragment:?}",
            case.case
        );
    }
    assert_eq!(
        reply.text("KmsKeyId"),
        case.kms_key_id,
        "case {}: expected KmsKeyId {:?}, got {:?}",
        case.case,
        case.kms_key_id,
        reply.text("KmsKeyId")
    );
    assert!(
        reply.text("CreateTime").is_some(),
        "case {}: the snapshot must report when it was taken, got {}",
        case.case,
        reply.body
    );
    // A manual snapshot does not expire, and this emulator serves no cache data
    // plane, so neither member is reported rather than invented.
    for absent in ["ExpiryTime", "BytesUsedForCache"] {
        assert_eq!(
            reply.text(absent),
            None,
            "case {}: {absent} must not be reported, got {:?}",
            case.case,
            reply.text(absent)
        );
    }
}

#[tokio::test]
async fn every_stored_cache_can_be_snapshotted_and_the_snapshot_is_read_back() {
    assert!(
        PAST_TRANSITION > state::SERVERLESS_CACHE_TRANSITION
            && PAST_TRANSITION > state::SNAPSHOT_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the cache span ({:?}) and the \
         snapshot span ({:?}) this service publishes",
        state::SERVERLESS_CACHE_TRANSITION,
        state::SNAPSHOT_TRANSITION
    );
    let emulator = Emulator::new();

    // Every row's source cache. The major engine version each one runs is read
    // off the cache itself, so the assertion below is that the snapshot copied
    // the cache rather than that it carries a version this test chose.
    let mut versions = Vec::new();
    for case in CASES {
        let created = create_cache(
            &emulator,
            case.case,
            case.cache_request_name,
            case.engine,
            case.cache_extra,
        )
        .await;
        versions.push(
            created
                .text("MajorEngineVersion")
                .map(str::to_string)
                .unwrap_or_else(|| {
                    panic!("case {}: the cache must report its major version", case.case)
                }),
        );
    }
    // A snapshot is taken of a settled cache, so each cache is read back first:
    // that read is answered once the create has settled it.
    for case in CASES {
        let described = emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", case.cache_request_name)]),
            )
            .await;
        assert_eq!(
            described.text("Status"),
            Some("available"),
            "case {}: the read after the create must report the cache available, got {}",
            case.case,
            described.body
        );
    }

    for (case, version) in CASES.iter().zip(&versions) {
        let mut pairs = vec![
            ("ServerlessCacheSnapshotName", case.snapshot_request_name),
            ("ServerlessCacheName", case.cache_request_name),
        ];
        pairs.extend_from_slice(case.snapshot_extra);
        let created = create_snapshot(&emulator, &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {}: CreateServerlessCacheSnapshot must answer 200, got {} {}",
            case.case, created.status, created.body
        );
        // A created snapshot is NOT ready: the span is never zero, so the call
        // itself reports the transient status.
        assert_snapshot(case, &created, state::STATUS_CREATING, version);

        // Read straight back: the stored record is the one the create answered
        // with, and the read is answered once the snapshot the clock owed is
        // available.
        let immediately =
            describe_snapshot(&emulator, case.case, case.snapshot_request_name).await;
        assert_snapshot(case, &immediately, state::STATUS_AVAILABLE, version);
        assert_eq!(
            immediately.text("CreateTime"),
            created.text("CreateTime"),
            "case {}: a read must report the instant the snapshot was taken at, got {:?}",
            case.case,
            immediately.text("CreateTime")
        );

        // Taking a snapshot is one of the events this service records, under the
        // snapshot itself.
        let events = emulator
            .call(
                "DescribeEvents",
                &params(&[
                    ("SourceIdentifier", case.snapshot_stored_name),
                    (
                        "SourceType",
                        state::SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT,
                    ),
                ]),
            )
            .await;
        assert!(
            events
                .all("Message")
                .iter()
                .any(|message| message.contains("created")),
            "case {}: the create must be recorded in the event log, got {}",
            case.case,
            events.body
        );
    }

    // Every snapshot stays available, with the rest of what it reports
    // unchanged.
    for (case, version) in CASES.iter().zip(&versions) {
        let settled = describe_snapshot(&emulator, case.case, case.snapshot_request_name).await;
        assert_snapshot(case, &settled, state::STATUS_AVAILABLE, version);
    }

    // The tags a create was given are the snapshot's own, resolved through the
    // ARN it reports — the one copy the tagging operations read.
    for case in CASES {
        let described = describe_snapshot(&emulator, case.case, case.snapshot_request_name).await;
        let arn = described.text("ARN").expect("the snapshot reports an ARN");
        let listed = emulator
            .call("ListTagsForResource", &params(&[("ResourceName", arn)]))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {}: ListTagsForResource must answer 200 for a stored snapshot, got {} {}",
            case.case, listed.status, listed.body
        );
        let keys: Vec<&str> = case.tags.iter().map(|(key, _)| *key).collect();
        let values: Vec<&str> = case.tags.iter().map(|(_, value)| *value).collect();
        assert_eq!(
            listed.all("Key"),
            keys,
            "case {}: expected tag keys {keys:?}, got {}",
            case.case,
            listed.body
        );
        assert_eq!(
            listed.all("Value"),
            values,
            "case {}: expected tag values {values:?}, got {}",
            case.case,
            listed.body
        );
    }

    // Every row's snapshot is stored, and nothing else is.
    let listed = describe_snapshots(&emulator, &[]).await;
    let mut expected: Vec<&str> = CASES.iter().map(|case| case.snapshot_stored_name).collect();
    expected.sort_unstable();
    assert_eq!(
        listed.all("ServerlessCacheSnapshotName"),
        expected,
        "the store must hold exactly the snapshots the table took, got {}",
        listed.body
    );
}

#[tokio::test]
async fn a_source_cache_the_account_does_not_hold_is_refused_whatever_the_name() {
    let emulator = Emulator::new();
    // One stored cache with one snapshot of it, so the refusals below are about
    // the NAME and not about an empty store. A cache CLUSTER with a similar id
    // is created too: a provisioned cluster is a different resource, so its id
    // is not a serverless-cache name.
    create_cache(&emulator, "stored", "stored-cache", "redis", &[]).await;
    let cluster = emulator
        .call(
            "CreateCacheCluster",
            &params(&[
                ("CacheClusterId", "shadow-cluster"),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "the cache cluster the table needs must be created, got {}",
        cluster.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let seeded = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "stored-snapshot"),
            ("ServerlessCacheName", "stored-cache"),
        ],
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "the snapshot the table needs must be taken, got {}",
        seeded.body
    );

    // The axis: every way a name can fail to name a stored serverless cache.
    let cases = [
        ("never created", "no-such-cache"),
        ("a prefix of a stored name", "stored"),
        ("a stored name with a suffix", "stored-cache-2"),
        ("the id of a cache cluster", "shadow-cluster"),
        ("the name of a stored SNAPSHOT", "stored-snapshot"),
        ("a single character", "x"),
        ("the same absent name in mixed case", "No-Such-Cache"),
        (
            "a long name",
            "a-serverless-cache-name-that-nobody-ever-created-in-this-account",
        ),
    ];

    for (label, name) in cases {
        let refused = create_snapshot(
            &emulator,
            &[
                ("ServerlessCacheSnapshotName", "would-be-snapshot"),
                ("ServerlessCacheName", name),
            ],
        )
        .await;
        assert_refused(label, &refused, 404, "ServerlessCacheNotFoundFault");
        // The message is built from the request: it names the cache the caller
        // asked for, exactly as asked for.
        let expected = format!("Serverless cache with name {name} not found.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {label:?} ({name}): the fault must name the requested cache, got {:?}",
            refused.error_message()
        );
        // And the refused call stored nothing: the name it asked for is free.
        let absent = describe_snapshots(
            &emulator,
            &[("ServerlessCacheSnapshotName", "would-be-snapshot")],
        )
        .await;
        assert_refused(
            label,
            &absent,
            404,
            "ServerlessCacheSnapshotNotFoundFault",
        );
    }

    // The one snapshot the account does hold is untouched.
    let listed = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        listed.all("ServerlessCacheSnapshotName"),
        vec!["stored-snapshot"],
        "a refused create must leave the store alone, got {}",
        listed.body
    );
}

#[tokio::test]
async fn a_snapshot_this_service_cannot_take_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    create_cache(&emulator, "available", "available-cache", "redis", &[]).await;
    // Created first and waited out, so the cache the rows name is in the settled
    // status a snapshot is taken from before the table runs.
    std::thread::sleep(PAST_TRANSITION);
    let taken = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "taken-name"),
            ("ServerlessCacheName", "available-cache"),
        ],
    )
    .await;
    assert_eq!(
        taken.status, 200,
        "the snapshot whose name the table reuses must be taken, got {}",
        taken.body
    );
    // This one is still inside its create span while the table below runs, so it
    // has no settled data set to copy.
    create_cache(&emulator, "creating", "creating-cache", "valkey", &[]).await;

    // The axis: why the snapshot cannot be taken — the name is not one a
    // snapshot can be stored under, the name is already another snapshot's, or
    // the source cache is not in a status it can be copied from.
    let rows: &[RefusalCase] = &[
        (
            "a name no snapshot can be stored under",
            &[
                ("ServerlessCacheSnapshotName", ""),
                ("ServerlessCacheName", "available-cache"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a name carrying a character a name may not",
            &[
                ("ServerlessCacheSnapshotName", "not a name"),
                ("ServerlessCacheName", "available-cache"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a name another snapshot already holds",
            &[
                ("ServerlessCacheSnapshotName", "taken-name"),
                ("ServerlessCacheName", "available-cache"),
            ],
            400,
            "ServerlessCacheSnapshotAlreadyExistsFault",
        ),
        (
            "that same name in mixed case",
            &[
                ("ServerlessCacheSnapshotName", "Taken-Name"),
                ("ServerlessCacheName", "available-cache"),
            ],
            400,
            "ServerlessCacheSnapshotAlreadyExistsFault",
        ),
        (
            "a source cache still inside its create span",
            &[
                ("ServerlessCacheSnapshotName", "of-a-creating-cache"),
                ("ServerlessCacheName", "creating-cache"),
            ],
            400,
            "InvalidServerlessCacheStateFault",
        ),
    ];

    for (case, pairs, status, code) in rows {
        let refused = create_snapshot(&emulator, pairs).await;
        assert_refused(case, &refused, *status, code);
    }

    // The refusal messages are built from the request and from what the store
    // holds: the already-taken name names the snapshot it resolves to (in the
    // stored, lowercase form), and the state fault names the cache and the
    // status it is in.
    let taken_again = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "Taken-Name"),
            ("ServerlessCacheName", "available-cache"),
        ],
    )
    .await;
    assert_eq!(
        taken_again.error_message(),
        "Serverless cache snapshot taken-name already exists.",
        "the already-exists fault must name the snapshot the name resolves to, got {:?}",
        taken_again.error_message()
    );
    let of_creating = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "of-a-creating-cache"),
            ("ServerlessCacheName", "creating-cache"),
        ],
    )
    .await;
    let expected = format!(
        "Serverless cache creating-cache is in {} state, not {}.",
        state::SERVERLESS_STATUS_CREATING,
        state::SERVERLESS_STATUS_AVAILABLE
    );
    assert_eq!(
        of_creating.error_message(),
        expected,
        "the state fault must name the cache and the status it is in, got {:?}",
        of_creating.error_message()
    );

    // A cache on its way out cannot be copied either: a delete puts it in a
    // status that has no settled data set.
    let deleted = emulator
        .call(
            "DeleteServerlessCache",
            &params(&[("ServerlessCacheName", "available-cache")]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteServerlessCache must answer 200, got {} {}",
        deleted.status, deleted.body
    );
    let while_deleting = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "of-a-deleting-cache"),
            ("ServerlessCacheName", "available-cache"),
        ],
    )
    .await;
    assert_refused(
        "a source cache the clock is removing",
        &while_deleting,
        400,
        "InvalidServerlessCacheStateFault",
    );
    // And past the removal the cache is gone, so the same call is a not-found.
    std::thread::sleep(PAST_TRANSITION);
    let when_gone = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "of-a-removed-cache"),
            ("ServerlessCacheName", "available-cache"),
        ],
    )
    .await;
    assert_refused(
        "a source cache the clock has removed",
        &when_gone,
        404,
        "ServerlessCacheNotFoundFault",
    );

    // Nothing the table asked for was stored: the one snapshot taken before it
    // ran is still the only one, and it outlived the cache it was taken of.
    let listed = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        listed.all("ServerlessCacheSnapshotName"),
        vec!["taken-name"],
        "a refused create must store nothing, got {}",
        listed.body
    );
    assert_eq!(
        listed.text("ServerlessCacheName"),
        Some("available-cache"),
        "the snapshot must still report the cache it was taken of, got {}",
        listed.body
    );
}

#[tokio::test]
async fn the_describe_pages_and_filters_what_the_store_holds() {
    let emulator = Emulator::new();
    for (cache, engine) in [("first-cache", "redis"), ("second-cache", "valkey")] {
        create_cache(&emulator, cache, cache, engine, &[]).await;
    }
    std::thread::sleep(PAST_TRANSITION);
    // Five snapshots over two caches, named out of alphabetical order: a page is
    // served in the store's own total order, not in the order they were taken.
    let taken = [
        ("snap-e", "first-cache"),
        ("snap-a", "second-cache"),
        ("snap-c", "first-cache"),
        ("snap-b", "second-cache"),
        ("snap-d", "first-cache"),
    ];
    for (snapshot, cache) in taken {
        let created = create_snapshot(
            &emulator,
            &[
                ("ServerlessCacheSnapshotName", snapshot),
                ("ServerlessCacheName", cache),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {snapshot}: the snapshot must be taken, got {} {}",
            created.status, created.body
        );
    }

    let all = vec!["snap-a", "snap-b", "snap-c", "snap-d", "snap-e"];
    let listed = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        listed.all("ServerlessCacheSnapshotName"),
        all,
        "an unfiltered describe must report every stored snapshot in order, got {}",
        listed.body
    );
    assert_eq!(
        listed.text("NextToken"),
        None,
        "a page that holds everything carries no token, got {:?}",
        listed.text("NextToken")
    );

    // The page-size axis: every size from one record to more records than the
    // store holds walks the same five snapshots.
    for page_size in [1_usize, 2, 4, 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut pairs = vec![("MaxResults", size.as_str())];
            if let Some(resume) = token.as_deref() {
                pairs.push(("NextToken", resume));
            }
            let page = describe_snapshots(&emulator, &pairs).await;
            assert_eq!(
                page.status, 200,
                "case MaxResults={page_size}: the page must answer 200, got {} {}",
                page.status, page.body
            );
            let names = page.all("ServerlessCacheSnapshotName");
            assert!(
                names.len() <= page_size,
                "case MaxResults={page_size}: a page must hold at most {page_size} records, got \
                 {names:?}"
            );
            seen.extend(names.iter().map(|name| (*name).to_string()));
            match page.text("NextToken") {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case MaxResults={page_size}: the walk must report every snapshot exactly once, got \
             {seen:?}"
        );
    }

    // The filter axis: the source cache, the snapshot's own name, and the type.
    for (cache, expected) in [
        ("first-cache", vec!["snap-c", "snap-d", "snap-e"]),
        ("second-cache", vec!["snap-a", "snap-b"]),
        ("FIRST-CACHE", vec!["snap-c", "snap-d", "snap-e"]),
    ] {
        let filtered = describe_snapshots(&emulator, &[("ServerlessCacheName", cache)]).await;
        assert_eq!(
            filtered.all("ServerlessCacheSnapshotName"),
            expected,
            "case {cache}: only that cache's snapshots may be reported, got {}",
            filtered.body
        );
    }
    for kind in [
        state::SERVERLESS_SNAPSHOT_TYPE_MANUAL,
        "MANUAL",
    ] {
        let filtered = describe_snapshots(&emulator, &[("SnapshotType", kind)]).await;
        assert_eq!(
            filtered.all("ServerlessCacheSnapshotName"),
            all,
            "case {kind}: every stored snapshot is of that type, got {}",
            filtered.body
        );
    }
    // A type no stored snapshot carries selects nothing — the store holds no
    // snapshot ElastiCache took on its own.
    let other_type = describe_snapshots(&emulator, &[("SnapshotType", "automatic")]).await;
    assert_eq!(
        other_type.status, 200,
        "a type filter nothing matches must still answer 200, got {} {}",
        other_type.status, other_type.body
    );
    assert!(
        other_type.all("ServerlessCacheSnapshotName").is_empty(),
        "a type no stored snapshot carries must report nothing, got {}",
        other_type.body
    );
    // Two filters that cannot both hold report nothing either.
    let crossed = describe_snapshots(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "snap-a"),
            ("ServerlessCacheName", "first-cache"),
        ],
    )
    .await;
    assert!(
        crossed.all("ServerlessCacheSnapshotName").is_empty(),
        "a snapshot of another cache must not be reported, got {}",
        crossed.body
    );

    // The refusals: a page size the operation does not serve, a token it did not
    // mint, and a name that resolves nothing.
    let rows: &[RefusalCase] = &[
        (
            "a page size past the documented maximum",
            &[("MaxResults", "51")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a page size no page can be",
            &[("MaxResults", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint",
            &[("NextToken", "not-a-token")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a snapshot name no snapshot holds",
            &[("ServerlessCacheSnapshotName", "no-such-snapshot")],
            404,
            "ServerlessCacheSnapshotNotFoundFault",
        ),
        (
            "a source cache no cache holds",
            &[("ServerlessCacheName", "no-such-cache")],
            404,
            "ServerlessCacheNotFoundFault",
        ),
    ];
    for (case, pairs, status, code) in rows {
        let refused = describe_snapshots(&emulator, pairs).await;
        assert_refused(case, &refused, *status, code);
    }
    // The largest page the operation documents is served, not refused.
    let largest = describe_snapshots(
        &emulator,
        &[(
            "MaxResults",
            &state::SERVERLESS_SNAPSHOT_MAX_RESULTS_MAX.to_string(),
        )],
    )
    .await;
    assert_eq!(
        largest.all("ServerlessCacheSnapshotName"),
        all,
        "the documented maximum page size must be served, got {} {}",
        largest.status,
        largest.body
    );
}

#[tokio::test]
async fn the_store_bounds_how_many_snapshots_either_mint_route_takes() {
    let emulator = Emulator::new();
    create_cache(&emulator, "quota", "quota-cache", "redis", &[]).await;
    create_cache(&emulator, "doomed", "doomed-cache", "valkey", &[]).await;
    std::thread::sleep(PAST_TRANSITION);

    // Fill the store to its published bound. Every snapshot up to it is taken,
    // so the refusals below are about the bound and not about any one name.
    for index in 0..state::SERVERLESS_CACHE_SNAPSHOT_QUOTA {
        let name = format!("quota-snapshot-{index}");
        let created = create_snapshot(
            &emulator,
            &[
                ("ServerlessCacheSnapshotName", name.as_str()),
                ("ServerlessCacheName", "quota-cache"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name}: a snapshot inside the bound must be taken, got {} {}",
            created.status, created.body
        );
    }
    let expected_quota_message = format!(
        "This account already holds the maximum of {} serverless cache snapshots.",
        state::SERVERLESS_CACHE_SNAPSHOT_QUOTA
    );

    // The axis: both routes that mint a serverless cache snapshot. Neither may
    // grow the store past the bound.
    let past_bound = create_snapshot(
        &emulator,
        &[
            ("ServerlessCacheSnapshotName", "one-too-many"),
            ("ServerlessCacheName", "quota-cache"),
        ],
    )
    .await;
    assert_refused(
        "a create past the bound",
        &past_bound,
        400,
        "ServerlessCacheSnapshotQuotaExceededFault",
    );
    assert_eq!(
        past_bound.error_message(),
        expected_quota_message,
        "the quota fault must state the bound, got {:?}",
        past_bound.error_message()
    );

    // A delete models no quota fault, so its `FinalSnapshotName` is refused as a
    // value it cannot accept — and the delete does NOT happen.
    let final_past_bound = emulator
        .call(
            "DeleteServerlessCache",
            &params(&[
                ("ServerlessCacheName", "doomed-cache"),
                ("FinalSnapshotName", "one-final-too-many"),
            ]),
        )
        .await;
    assert_refused(
        "a final snapshot past the bound",
        &final_past_bound,
        400,
        "InvalidParameterValue",
    );
    assert_eq!(
        final_past_bound.error_message(),
        format!("Invalid value one-final-too-many for FinalSnapshotName: {expected_quota_message}"),
        "the refusal must name the member and state the bound, got {:?}",
        final_past_bound.error_message()
    );

    // Neither refusal stored anything, and the cache the delete named is still
    // there, untouched.
    for name in ["one-too-many", "one-final-too-many"] {
        let absent = describe_snapshots(&emulator, &[("ServerlessCacheSnapshotName", name)]).await;
        assert_refused(name, &absent, 404, "ServerlessCacheSnapshotNotFoundFault");
    }
    let untouched = emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", "doomed-cache")]),
        )
        .await;
    assert_eq!(
        untouched.text("Status"),
        Some(state::SERVERLESS_STATUS_AVAILABLE),
        "a refused delete must leave the cache alone, got {}",
        untouched.body
    );
}

#[tokio::test]
async fn a_final_snapshot_is_taken_of_the_cache_a_delete_takes_away() {
    let emulator = Emulator::new();
    // The axis: the engine the cache runs and the case its final snapshot's name
    // is sent in. A final snapshot is the same snapshot a create takes, so every
    // row is read back the same way.
    let rows = [
        ("redis", "final-redis-cache", "Final-Redis-Snapshot", "final-redis-snapshot"),
        ("valkey", "final-valkey-cache", "final-valkey-snapshot", "final-valkey-snapshot"),
        ("memcached", "final-memcached-cache", "F", "f"),
    ];
    for (engine, cache, _, _) in rows {
        create_cache(&emulator, cache, cache, engine, &[]).await;
    }
    std::thread::sleep(PAST_TRANSITION);

    for (engine, cache, requested, stored) in rows {
        let deleted = emulator
            .call(
                "DeleteServerlessCache",
                &params(&[
                    ("ServerlessCacheName", cache),
                    ("FinalSnapshotName", requested),
                ]),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {cache}: a delete that asks for a final snapshot must answer 200, got {} {}",
            deleted.status, deleted.body
        );

        // The snapshot was taken of the cache as it stood before the delete, so
        // it reports the cache's own configuration.
        let described = describe_snapshots(
            &emulator,
            &[("ServerlessCacheSnapshotName", requested)],
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {cache}: the final snapshot must be stored, got {} {}",
            described.status, described.body
        );
        for (member, expected) in [
            ("ServerlessCacheSnapshotName", stored),
            ("ServerlessCacheName", cache),
            ("Engine", engine),
            ("SnapshotType", state::SERVERLESS_SNAPSHOT_TYPE_MANUAL),
        ] {
            assert_eq!(
                described.text(member),
                Some(expected),
                "case {cache}: expected {member} {expected:?}, got {:?} ({})",
                described.text(member),
                described.body
            );
        }

        // A name the final snapshot already holds cannot be taken again, and a
        // name no snapshot can be stored under is refused outright.
        let again = emulator
            .call(
                "DeleteServerlessCache",
                &params(&[
                    ("ServerlessCacheName", cache),
                    ("FinalSnapshotName", requested),
                ]),
            )
            .await;
        assert_refused(
            cache,
            &again,
            400,
            "ServerlessCacheSnapshotAlreadyExistsFault",
        );
    }

    // Past the deletion span every cache is gone and every final snapshot is
    // still there, settled: a snapshot outlives the cache it was taken of.
    std::thread::sleep(PAST_TRANSITION);
    let mut expected: Vec<&str> = rows.iter().map(|(_, _, _, stored)| *stored).collect();
    expected.sort_unstable();
    let listed = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        listed.all("ServerlessCacheSnapshotName"),
        expected,
        "every final snapshot must outlive its cache, got {}",
        listed.body
    );
    for status in listed.all("Status") {
        assert_eq!(
            status,
            state::STATUS_AVAILABLE,
            "a settled final snapshot must report {:?}, got {status:?}",
            state::STATUS_AVAILABLE
        );
    }
    let caches = emulator
        .call("DescribeServerlessCaches", &params(&[]))
        .await;
    assert!(
        caches.all("ServerlessCacheName").is_empty(),
        "every cache the table deleted must be gone, got {}",
        caches.body
    );
    // The snapshot is still reported under the cache it was taken of, even
    // though that cache is gone.
    for (_, cache, _, stored) in rows {
        let by_source = describe_snapshots(&emulator, &[("ServerlessCacheName", cache)]).await;
        assert_eq!(
            by_source.all("ServerlessCacheSnapshotName"),
            vec![stored],
            "case {cache}: the snapshot of a removed cache must still be reported, got {}",
            by_source.body
        );
    }
}
