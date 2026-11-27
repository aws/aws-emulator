// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ElastiCache's FILTERED reads, and for what its event
//! log holds.
//!
//! Three rules are under test, each over a table of inputs rather than one
//! recorded request:
//!
//! * `DescribeSnapshots`' selector members (`CacheClusterId`,
//!   `ReplicationGroupId`, `SnapshotName`, `SnapshotSource`) NARROW the snapshot
//!   list, so a well-formed value that selects no stored snapshot is answered
//!   with HTTP 200 and an empty `Snapshots` element — not with one of the
//!   operation's not-found faults, which belong to the calls that act on a
//!   named cluster or snapshot.
//! * a request that names two members which are the two sides of ONE selection
//!   (`ReplicationGroupId` and `CacheClusterId`) is refused on the combination
//!   before any lookup, so the refusal is the same whichever of the two ids the
//!   store happens to hold.
//! * a filter member whose vocabulary the service states
//!   (`DescribeServerlessCacheSnapshots$SnapshotType`) refuses a value outside
//!   that vocabulary rather than accepting it as a filter that matches nothing.
//! * the event log narrates the work this service goes on to do to a resource,
//!   so a create that answers with a resource already in its settled state
//!   records no event, while a create the clock owes a transition does.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`], so
//! a response that is not computed from persisted state cannot pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{
    account_subnet_ids, create_available_cluster, poll_cluster_status, subnet_group_params,
};
use crate::harness::{params, poll_until, Emulator, Reply, REGION};
use crate::snapshots::{create_snapshot, describe_snapshots, reported_names};

/// AWS's refusal for a `SnapshotType` outside the two types a serverless cache
/// snapshot is ever of. The whole sentence is fixed text, so it is written out
/// here rather than read from the service it is asserted against.
const BAD_SNAPSHOT_TYPE: &str =
    "Invalid SnapshotType Specified. If specified, should be either 'automated' or 'manual'";

/// AWS's refusal for a `DescribeSnapshots$SnapshotSource` in neither
/// vocabulary: the sibling sentence to [`BAD_SNAPSHOT_TYPE`], naming the two
/// sources a snapshot is ever of rather than the spellings the filter accepts.
const BAD_SNAPSHOT_SOURCE: &str =
    "Invalid snapshot source specified. If specified, should be either 'automated' or 'manual'";

/// AWS's refusal for a `DescribeEvents$SourceType` outside the modelled
/// `SourceType` enum: the whole sentence is fixed text, listing the event
/// source types the service serves in its own order.
const EVENT_SOURCE_TYPE: &str = "Invalid event source type. Valid types are \
     cache-cluster,cache-parameter-group,cache-security-group,cache-subnet-group,\
     replication-group,user,user-group,serverless-cache,serverless-cache-snapshot";

/// One filter row: the case, the form pairs sent, and the `SnapshotName`s the
/// read must report — in the stable total order a describe answers in, which is
/// by stored name.
type FilterRow<'a> = (&'static str, Vec<(&'a str, &'a str)>, Vec<&'static str>);

/// `DescribeServerlessCacheSnapshots` with `pairs`.
async fn describe_serverless_snapshots(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator
        .call("DescribeServerlessCacheSnapshots", &params(pairs))
        .await
}

/// The events recorded under `source`, as their messages.
async fn events_of(emulator: &Emulator, case: &str, source: &str) -> Vec<String> {
    let reply = emulator
        .call("DescribeEvents", &params(&[("SourceIdentifier", source)]))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeEvents must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .all("Event/Message")
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_snapshot_filter_that_selects_nothing_is_a_successful_empty_page() {
    let emulator = Emulator::new();
    // One cluster with a snapshot, one cluster without, and a replication group
    // whose member cluster is snapshotted — so every selector below has
    // something it must keep and something it must drop.
    create_available_cluster(&emulator, "held-source", &[]).await;
    create_available_cluster(&emulator, "no-snapshots", &[]).await;
    let taken = create_snapshot(&emulator, "held-snap", "held-source", &[]).await;
    assert_eq!(
        taken.status, 200,
        "the cluster's snapshot must be taken, got {}",
        taken.body
    );
    let group = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "rg-filtered"),
                ("ReplicationGroupDescription", "the filtered-read suite"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the replication group must be created, got {}",
        group.body
    );
    // The member cluster of a group is named after it; a snapshot of one is
    // what carries the group's id, which `ReplicationGroupId` selects on.
    poll_cluster_status(
        &emulator,
        "rg-filtered",
        "rg-filtered-001",
        state::STATUS_AVAILABLE,
    )
    .await;
    let group_snap = create_snapshot(&emulator, "group-snap", "rg-filtered-001", &[]).await;
    assert_eq!(
        group_snap.status, 200,
        "the group member's snapshot must be taken, got {}",
        group_snap.body
    );

    // The axis: which selector is sent, and how the value relates to what the
    // store holds — a value that selects records, the same value in another
    // case, a value nothing was created under, a resource that exists but owns
    // no snapshot, the longest identifier a lookup accepts, and two selectors
    // that cannot both hold.
    let longest_unknown = "g".repeat(state::LOOKUP_IDENTIFIER_MAX_LEN);
    let rows: [FilterRow; 12] = [
        ("no selector at all", vec![], vec!["group-snap", "held-snap"]),
        (
            "a cluster that owns a snapshot",
            vec![("CacheClusterId", "held-source")],
            vec!["held-snap"],
        ),
        (
            "that cluster named in another case",
            vec![("CacheClusterId", "HELD-SOURCE")],
            vec!["held-snap"],
        ),
        (
            "a cluster the account holds that owns no snapshot",
            vec![("CacheClusterId", "no-snapshots")],
            vec![],
        ),
        (
            "a cluster id nothing was created under",
            vec![("CacheClusterId", "no-such-cluster")],
            vec![],
        ),
        (
            "the longest cluster id a lookup takes, unknown",
            vec![("CacheClusterId", longest_unknown.as_str())],
            vec![],
        ),
        (
            "the member cluster of the replication group",
            vec![("CacheClusterId", "rg-filtered-001")],
            vec!["group-snap"],
        ),
        (
            "the replication group that owns a snapshot",
            vec![("ReplicationGroupId", "rg-filtered")],
            vec!["group-snap"],
        ),
        (
            "that group named in another case",
            vec![("ReplicationGroupId", "RG-Filtered")],
            vec!["group-snap"],
        ),
        (
            "a replication group id nothing was created under",
            vec![("ReplicationGroupId", "no-such-group")],
            vec![],
        ),
        (
            "a cluster that owns a snapshot and a name it does not carry",
            vec![
                ("CacheClusterId", "held-source"),
                ("SnapshotName", "group-snap"),
            ],
            vec![],
        ),
        (
            "an unknown cluster beside a source that selects everything",
            vec![
                ("CacheClusterId", "no-such-cluster"),
                ("SnapshotSource", "manual"),
            ],
            vec![],
        ),
    ];

    for (case, filters, expected) in rows {
        let reply = describe_snapshots(&emulator, &filters).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a selector is answered with a page, not a fault; expected HTTP 200, \
             got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "",
            "case {case}: no error may be reported, got {}",
            reply.body
        );
        assert_eq!(
            reported_names(&reply),
            expected,
            "case {case}: expected {expected:?}, got {}",
            reply.body
        );
        assert!(
            reply.marker().is_none(),
            "case {case}: a page that returned everything must carry no Marker, got {}",
            reply.body
        );
    }

    // A read reports: the probes above changed nothing a later caller sees.
    let after = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&after),
        vec!["group-snap", "held-snap"],
        "case after probing: a describe must not change what a later caller sees, got {}",
        after.body
    );
}

#[tokio::test]
async fn naming_both_snapshot_sources_is_refused_before_either_is_resolved() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "both-cluster", &[]).await;
    let taken = create_snapshot(&emulator, "both-snap", "both-cluster", &[]).await;
    assert_eq!(
        taken.status, 200,
        "the snapshot must be taken, got {}",
        taken.body
    );
    let group = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "both-group"),
                ("ReplicationGroupDescription", "the filtered-read suite"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the replication group must be created, got {}",
        group.body
    );

    // The axis: which of the two ids the store actually holds, how each is
    // spelled, and what else the request narrows by. The combination is refused
    // the same way in every row, which is what "before either is resolved"
    // means: neither id's existence can change the answer.
    let rows: [(&str, &[(&str, &str)]); 6] = [
        (
            "neither id is held",
            &[
                ("ReplicationGroupId", "no-such-group"),
                ("CacheClusterId", "no-such-cluster"),
            ],
        ),
        (
            "the cluster is held, the group is not",
            &[
                ("ReplicationGroupId", "no-such-group"),
                ("CacheClusterId", "both-cluster"),
            ],
        ),
        (
            "the group is held, the cluster is not",
            &[
                ("ReplicationGroupId", "both-group"),
                ("CacheClusterId", "no-such-cluster"),
            ],
        ),
        (
            "both are held",
            &[
                ("ReplicationGroupId", "both-group"),
                ("CacheClusterId", "both-cluster"),
            ],
        ),
        (
            "both are held, named in another case",
            &[
                ("ReplicationGroupId", "BOTH-GROUP"),
                ("CacheClusterId", "Both-Cluster"),
            ],
        ),
        (
            "both, beside a snapshot name the store holds",
            &[
                ("ReplicationGroupId", "both-group"),
                ("CacheClusterId", "both-cluster"),
                ("SnapshotName", "both-snap"),
            ],
        ),
    ];

    for (case, filters) in rows {
        let reply = describe_snapshots(&emulator, filters).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterCombination",
            "case {case}: expected InvalidParameterCombination, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            state::SNAPSHOT_SOURCE_EXCLUSIVE_MESSAGE,
            "case {case}: expected the message {:?}, got {:?}",
            state::SNAPSHOT_SOURCE_EXCLUSIVE_MESSAGE,
            reply.error_message()
        );
        for value in filters.iter().map(|(_, value)| *value) {
            assert!(
                !reply.error_message().contains(value),
                "case {case}: the refusal names the two members, not the request's values; got \
                 {:?}",
                reply.error_message()
            );
        }
    }

    // The contrast the rule rests on: each id ALONE is served, so the refusal
    // above is the combination speaking and not either lookup.
    for (case, filters) in [
        ("the cluster alone", &[("CacheClusterId", "both-cluster")]),
        ("the group alone", &[("ReplicationGroupId", "both-group")]),
    ] {
        let reply = describe_snapshots(&emulator, filters).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: one source alone must be served, got {} {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_serverless_snapshot_type_outside_its_vocabulary_is_refused() {
    let emulator = Emulator::new();
    let cache = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", "type-cache"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        cache.status, 200,
        "the serverless cache must be created, got {}",
        cache.body
    );
    // A snapshot is taken of a SETTLED cache, so the suite looks again until the
    // create has settled it rather than waiting a fixed time.
    poll_until(
        &emulator,
        REGION,
        "type-cache",
        "the serverless cache settling at available",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", "type-cache")]),
        |reply| reply.text("Status") == Some("available"),
    )
    .await;
    let snapshot = emulator
        .call(
            "CreateServerlessCacheSnapshot",
            &params(&[
                ("ServerlessCacheSnapshotName", "type-snap"),
                ("ServerlessCacheName", "type-cache"),
            ]),
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "the serverless cache snapshot must be taken, got {}",
        snapshot.body
    );

    // The vocabulary is the one a snapshot reports its own type under
    // (`ServerlessCacheSnapshot$SnapshotType`), matched ignoring ASCII case: a
    // snapshot a caller asked for is `manual`, and no stored snapshot is
    // `automated`, so that value selects nothing and is still served.
    for (case, value, expected) in [
        ("the type every stored snapshot is of", "manual", vec!["type-snap"]),
        ("that type in upper case", "MANUAL", vec!["type-snap"]),
        ("that type in mixed case", "Manual", vec!["type-snap"]),
        ("the other type of the vocabulary", "automated", vec![]),
        ("the other type in mixed case", "Automated", vec![]),
    ] {
        let reply = describe_serverless_snapshots(&emulator, &[("SnapshotType", value)]).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.all("ServerlessCacheSnapshotName"),
            expected,
            "case {case}: expected {expected:?}, got {}",
            reply.body
        );
    }

    // The axis: every way a value can fall outside the vocabulary — a near
    // miss, the spellings another operation's source filter accepts
    // (`DescribeSnapshots$SnapshotSource` takes `system` and `user`, which this
    // member does not), the empty value, and one with surrounding space.
    for (case, value) in [
        ("a near miss", "automatic"),
        ("another filter's spelling for an automatic snapshot", "system"),
        ("another filter's spelling for a manual one", "user"),
        ("a value of the wrong kind entirely", "type-snap"),
        ("the empty value", ""),
        ("the right word with surrounding space", " manual "),
    ] {
        let reply = describe_serverless_snapshots(&emulator, &[("SnapshotType", value)]).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            BAD_SNAPSHOT_TYPE,
            "case {case}: expected the message {BAD_SNAPSHOT_TYPE:?}, got {:?}",
            reply.error_message()
        );
    }

    // The value is refused BEFORE anything is looked up: a request that also
    // names a snapshot or a cache the store does not hold is still answered
    // with the bad value rather than with either not-found fault.
    for (case, pairs) in [
        (
            "beside a snapshot name nothing was created under",
            &[
                ("SnapshotType", "automatic"),
                ("ServerlessCacheSnapshotName", "no-such-snapshot"),
            ],
        ),
        (
            "beside a cache name nothing was created under",
            &[
                ("SnapshotType", "automatic"),
                ("ServerlessCacheName", "no-such-cache"),
            ],
        ),
    ] {
        let reply = describe_serverless_snapshots(&emulator, pairs).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "InvalidParameterValue"),
            "case {case}: the bad value is refused before any lookup, got {} {}",
            reply.status,
            reply.body
        );
    }

    // And the read changed nothing: the stored snapshot is still reported.
    let after = describe_serverless_snapshots(&emulator, &[]).await;
    assert_eq!(
        after.all("ServerlessCacheSnapshotName"),
        vec!["type-snap"],
        "case after probing: a describe must not change what a later caller sees, got {}",
        after.body
    );
}

#[tokio::test]
async fn a_snapshot_source_outside_its_vocabulary_is_refused_with_the_same_sentence() {
    let emulator = Emulator::new();

    // The sibling filter of the serverless `SnapshotType`:
    // `DescribeSnapshots$SnapshotSource` names a source either the way the
    // member documents it (`system` / `user`) or the way a snapshot reports its
    // own (`automated` / `manual`), and a value in neither vocabulary is
    // refused with ElastiCache's own sentence for the two sources — the same
    // sentence, whichever value missed and however it missed. The axis is how
    // it misses: a status rather than a source, a near miss, a plural, the
    // empty value, and a value with surrounding space.
    for (case, value) in [
        ("a word of another vocabulary", "scheduled"),
        ("a snapshot's status rather than its source", "creating"),
        ("a near miss", "manually"),
        ("a plural of an accepted spelling", "users"),
        ("the empty value", ""),
        ("an accepted spelling with surrounding space", " manual "),
    ] {
        let reply = describe_snapshots(&emulator, &[("SnapshotSource", value)]).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            BAD_SNAPSHOT_SOURCE,
            "case {case}: expected the message {BAD_SNAPSHOT_SOURCE:?}, got {:?}",
            reply.error_message()
        );
    }
}

#[tokio::test]
async fn every_filter_vocabulary_is_refused_as_a_bad_parameter_value() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "vocabulary-cluster", &[]).await;

    // The same rule at the other two filter members whose vocabulary the
    // service states: `DescribeCacheParameters$Source` ("Valid values: user |
    // system | engine-default") names the rejected value and then the allowed
    // ones, and `DescribeEvents$SourceType` — the one member of this API that
    // targets the modelled `SourceType` enum, so the value is refused before
    // the handler runs — states the types it serves and interpolates nothing.
    // Both are the operation's modelled `InvalidParameterValue`, NOT the
    // framework's generic validation code, which neither operation models.
    //
    // The axis is which member carries the value and how the value misses:
    // another member's vocabulary, a near miss, the wrong case of a real
    // source type (a type IS an identity here, matched as the enum spells it)
    // and the empty value.
    for (case, pairs, expected) in [
        (
            "a Source of another vocabulary",
            vec![
                ("CacheParameterGroupName", "default.redis7"),
                ("Source", "automated"),
            ],
            "Invalid Source: automated. Allowed values are: engine-default, system, user",
        ),
        (
            "a Source that is a near miss",
            vec![
                ("CacheParameterGroupName", "default.redis7"),
                ("Source", "engine-defaults"),
            ],
            "Invalid Source: engine-defaults. Allowed values are: engine-default, system, user",
        ),
        (
            "the empty Source",
            vec![("CacheParameterGroupName", "default.redis7"), ("Source", "")],
            "Invalid Source: . Allowed values are: engine-default, system, user",
        ),
        (
            "a SourceType no event source is of",
            vec![("SourceType", "cache-node")],
            EVENT_SOURCE_TYPE,
        ),
        (
            "a SourceType of another vocabulary",
            vec![("SourceType", "user")],
            // `user` IS a member of the enum, so this row is served; the rows
            // around it are what the refusal is asserted on.
            "",
        ),
        (
            "a real SourceType in another case",
            vec![("SourceType", "CACHE-CLUSTER")],
            EVENT_SOURCE_TYPE,
        ),
        ("the empty SourceType", vec![("SourceType", "")], EVENT_SOURCE_TYPE),
    ] {
        let action = if pairs.iter().any(|(member, _)| *member == "SourceType") {
            "DescribeEvents"
        } else {
            "DescribeCacheParameters"
        };
        let reply = emulator.call(action, &params(&pairs)).await;
        if expected.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case}: a value the vocabulary holds must be served, got {} {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            expected,
            "case {case}: expected the message {expected:?}, got {:?}",
            reply.error_message()
        );
    }
}

/// One create row: the case, the `Action`, the form pairs it sends, the stored
/// identity the resource is recorded under, and whether the clock owes that
/// resource a transition — which is what decides whether its creation is an
/// event at all.
struct CreateRow<'a> {
    case: &'static str,
    action: &'static str,
    request: &'a [(String, String)],
    source: &'static str,
    clock_owes_a_transition: bool,
}

#[tokio::test]
async fn only_a_create_the_clock_owes_a_transition_records_an_event() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;

    // The axis: whether the created resource is one this service goes on to
    // provision. A resource a create answers with in its settled state has no
    // work to narrate, so its creation records nothing; one the clock owes a
    // transition records its creation (and, later, that transition).
    let parameter_group = params(&[
        ("CacheParameterGroupName", "Ready-Params"),
        ("CacheParameterGroupFamily", "redis7"),
        ("Description", "the filtered-read suite"),
    ]);
    let user = params(&[
        ("UserId", "Ready-User"),
        ("UserName", "ready-user"),
        ("Engine", "redis"),
        ("AccessString", "on ~* +@all"),
        ("NoPasswordRequired", "true"),
    ]);
    let cluster = crate::harness::cluster_create_params("Owed-Cluster", &[]);
    let serverless = params(&[
        ("ServerlessCacheName", "Owed-Serverless"),
        ("Engine", "redis"),
    ]);
    let subnet_group =
        subnet_group_params("Ready-Subnets", "the filtered-read suite", &subnets[..1]);
    let rows = [
        CreateRow {
            case: "a cache parameter group, usable as soon as it exists",
            action: "CreateCacheParameterGroup",
            request: &parameter_group,
            source: "ready-params",
            clock_owes_a_transition: false,
        },
        CreateRow {
            case: "a cache subnet group, usable as soon as it exists",
            action: "CreateCacheSubnetGroup",
            request: &subnet_group,
            source: "ready-subnets",
            clock_owes_a_transition: false,
        },
        CreateRow {
            case: "an RBAC user, active as soon as it exists",
            action: "CreateUser",
            request: &user,
            source: "ready-user",
            clock_owes_a_transition: false,
        },
        CreateRow {
            case: "a cache cluster, which the clock owes available",
            action: "CreateCacheCluster",
            request: &cluster,
            source: "owed-cluster",
            clock_owes_a_transition: true,
        },
        CreateRow {
            case: "a serverless cache, which the clock owes available",
            action: "CreateServerlessCache",
            request: &serverless,
            source: "owed-serverless",
            clock_owes_a_transition: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let created = emulator.call(row.action, row.request).await;
        assert_eq!(
            created.status, 200,
            "case {case}: {} must answer 200, got {} {}",
            row.action, created.status, created.body
        );
        let messages = events_of(&emulator, case, row.source).await;
        if row.clock_owes_a_transition {
            assert!(
                messages.iter().any(|message| message.contains("created")),
                "case {case}: the create must be recorded under the resource, got {messages:?}"
            );
        } else {
            assert!(
                messages.is_empty(),
                "case {case}: a create with nothing to narrate must record no event, got \
                 {messages:?}"
            );
        }
    }

    // The absence is about the CREATE, not about the source: something DONE to
    // one of those resources is recorded under the very same source.
    let changed = crate::modify_cache_parameter_group::modify(
        &emulator,
        "ready-params",
        &[("timeout", "300")],
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "the parameter group must be modified, got {} {}",
        changed.status, changed.body
    );
    let messages = events_of(&emulator, "a change to the ready group", "ready-params").await;
    assert_eq!(
        messages.len(),
        1,
        "case a change to the ready group: the change must be the one recorded event, got \
         {messages:?}"
    );

    // And the transitions the clock owed the provisioned pair are recorded
    // beside their creates, which is the other half of the rule. The suite
    // LOOKS AGAIN until the log holds both rather than waiting a fixed time.
    for (case, source) in [
        ("the cluster", "owed-cluster"),
        ("the serverless cache", "owed-serverless"),
    ] {
        poll_until(
            &emulator,
            REGION,
            case,
            "the transition the clock owed being recorded beside the create",
            "DescribeEvents",
            &params(&[("SourceIdentifier", source)]),
            |reply| reply.all("Event/Message").len() == 2,
        )
        .await;
    }
}
