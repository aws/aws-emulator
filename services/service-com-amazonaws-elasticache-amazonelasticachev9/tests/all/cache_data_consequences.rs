// SPDX-License-Identifier: CC0-1.0

//! The DATA CONSEQUENCE of the two multi-step cache flows a client drives to prove one:
//! taking a serverless cache's snapshot and restoring a new cache from it, and scaling a
//! replication group's shard and failing it over.
//!
//! The rules under test:
//!
//! * a snapshot of a cache is a copy of that cache's DATA SET as of the instant it was
//!   taken, so a cache created from that snapshot's ARN starts with exactly the keys the
//!   snapshot captured, the snapshot reports the size of that data set, and the snapshot
//!   outlives both the cache it was taken of and every cache restored from it;
//! * the members of a replication group hold ONE data set, so adding a replica and moving
//!   the primary role between members changes WHICH address the group publishes for the
//!   writer and changes nothing a client can read there.
//!
//! The tables span what the rules generalise over: how many keys were written and of which
//! sizes, what was written AFTER a snapshot was taken (which is not in it), which of a
//! cache's several snapshots a restore names, how many snapshots one restore names at once,
//! how many times one snapshot is restored, and how many members a shard holds when its
//! primary role moves. The recorded flows — one key, one snapshot, one restore; two
//! members scaled to three and failed over — are one row of each.
//!
//! Every assertion goes over the wire: the control plane through the same router the
//! production emulator mounts, and the data plane over a real socket the core opened,
//! dialled at the address that control plane published.

use crate::cache_data_plane::{command, endpoint_of, open, open_tls, Resp, Session};
use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// One key a flow writes before a snapshot or a failover.
///
/// The rows differ along what the rules generalise over: the SIZE of the value (none, a
/// word, a value far longer than one frame's worth of bytes) and the shape of the key name.
struct Written {
    case: &'static str,
    key: &'static str,
    value: String,
}

/// The keys every flow in this suite writes, built once per flow so the long value is not
/// repeated in the source.
fn written() -> Vec<Written> {
    vec![
        Written {
            case: "a word",
            key: "greeting",
            value: "pre-failover-value".to_string(),
        },
        Written {
            case: "an empty value",
            key: "empty",
            value: String::new(),
        },
        Written {
            case: "a value longer than one frame",
            key: "bulk",
            // 9 KiB, so the reply is read over more than one socket read.
            value: "0123456789abcdef".repeat(576),
        },
        Written {
            case: "a key name with separators",
            key: "tenant:42/session",
            value: "{\"user\":\"42\"}".to_string(),
        },
    ]
}

/// Write every row of `written` on `session` and assert each `SET` was accepted.
async fn write_all(case: &str, session: &mut Session, rows: &[Written]) {
    for row in rows {
        let reply = command(case, session, &["SET", row.key, &row.value]).await;
        assert_eq!(
            reply,
            Resp::Status("OK".to_string()),
            "case {case}/{}: a SET on the published endpoint must be accepted, got {reply:?}",
            row.case
        );
    }
}

/// Assert `session` reads back every row of `written`, and that `absent` is not there.
async fn assert_reads_back(case: &str, session: &mut Session, rows: &[Written], absent: &[&str]) {
    for row in rows {
        let reply = command(case, session, &["GET", row.key]).await;
        assert_eq!(
            reply,
            Resp::Bulk(row.value.clone()),
            "case {case}/{}: GET {} must answer the stored value ({} bytes), got {reply:?}",
            row.case,
            row.key,
            row.value.len()
        );
    }
    for key in absent {
        let reply = command(case, session, &["GET", key]).await;
        assert_eq!(
            reply,
            Resp::Nil,
            "case {case}: {key} was never stored in this cache's data set, got {reply:?}"
        );
    }
}

/// Create one serverless cache, wait until the clock has made it `available`, and answer
/// the endpoint it publishes: `(address, port)`.
async fn serverless_cache(emulator: &Emulator, name: &str, extra: &[(&str, &str)]) -> (String, String) {
    let mut pairs = params(&[("ServerlessCacheName", name), ("Engine", "redis")]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateServerlessCache", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {name}: CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );
    let described = describe_serverless_cache(emulator, name).await;
    endpoint_of(&described, "Endpoint/Address", "Endpoint/Port", name)
}

/// Read one serverless cache back once the clock has made it `available`.
async fn describe_serverless_cache(emulator: &Emulator, name: &str) -> Reply {
    poll_until(
        emulator,
        REGION,
        name,
        "the serverless cache reaching available",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", name)]),
        |reply| reply.text("member/Status") == Some("available"),
    )
    .await
}

/// Take one snapshot of one serverless cache, wait until the clock has made it `available`,
/// and answer `(ARN, BytesUsedForCache)`.
async fn serverless_snapshot(emulator: &Emulator, name: &str, cache: &str) -> (String, u64) {
    let created = emulator
        .call(
            "CreateServerlessCacheSnapshot",
            &params(&[
                ("ServerlessCacheSnapshotName", name),
                ("ServerlessCacheName", cache),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name}: CreateServerlessCacheSnapshot must answer 200, got {} {}",
        created.status, created.body
    );
    let settled = poll_until(
        emulator,
        REGION,
        name,
        "the serverless cache snapshot reaching available",
        "DescribeServerlessCacheSnapshots",
        &params(&[("ServerlessCacheSnapshotName", name)]),
        |reply| reply.text("ServerlessCacheSnapshot/Status") == Some("available"),
    )
    .await;
    let arn = settled
        .text("ARN")
        .unwrap_or_else(|| panic!("case {name}: the snapshot must publish its ARN, got {}", settled.body))
        .to_string();
    let size = settled
        .text("BytesUsedForCache")
        .unwrap_or_else(|| {
            panic!(
                "case {name}: the snapshot must report the size of the data set it captured, got {}",
                settled.body
            )
        })
        .parse::<u64>()
        .unwrap_or_else(|error| {
            panic!("case {name}: BytesUsedForCache must be a byte count, got {error}")
        });
    (arn, size)
}

/// Create one serverless cache restored from `arns`, wait until it is `available`, and open
/// a TLS RESP session on the endpoint it publishes — the wire a client of a serverless
/// cache uses.
async fn restored_session(emulator: &Emulator, name: &str, arns: &[&str]) -> Session {
    let mut extra: Vec<(String, String)> = Vec::new();
    for (index, arn) in arns.iter().enumerate() {
        extra.push((
            format!("SnapshotArnsToRestore.SnapshotArn.{}", index + 1),
            (*arn).to_string(),
        ));
    }
    let mut pairs = params(&[("ServerlessCacheName", name), ("Engine", "redis")]);
    pairs.extend(extra);
    let created = emulator.call("CreateServerlessCache", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {name}: a create restoring from {arns:?} must answer 200, got {} {}",
        created.status, created.body
    );
    let described = describe_serverless_cache(emulator, name).await;
    let (address, port) = endpoint_of(&described, "Endpoint/Address", "Endpoint/Port", name);
    open_tls(name, &address, &port).await
}

#[tokio::test]
async fn a_restored_serverless_cache_holds_the_data_set_its_snapshot_captured() {
    let emulator = Emulator::new();
    let rows = written();

    // The source cache, and the data a client puts in it over the TLS wire a serverless
    // cache is served on.
    let (address, port) = serverless_cache(&emulator, "origin", &[]).await;
    let mut origin = open_tls("origin", &address, &port).await;
    write_all("origin", &mut origin, &rows).await;

    // A snapshot of an EMPTY cache is the counter-row of the size assertion: its data set
    // holds nothing, so its size is zero.
    let (_, empty_size) = {
        serverless_cache(&emulator, "untouched", &[]).await;
        serverless_snapshot(&emulator, "snap-of-nothing", "untouched").await
    };
    assert_eq!(
        empty_size, 0,
        "case \"a snapshot of a cache nothing was written to\": expected size 0, got {empty_size}"
    );

    // The first snapshot: taken with the table's keys in the cache and nothing else.
    let (early_arn, early_size) = serverless_snapshot(&emulator, "snap-early", "origin").await;
    let written_bytes: usize = rows
        .iter()
        .map(|row| row.key.len() + row.value.len())
        .sum();
    assert!(
        early_size >= written_bytes as u64,
        "case \"snap-early\": the captured data set must account for the {written_bytes} bytes \
         written, got {early_size}"
    );

    // One more key, and a second snapshot. The LATE key is the row that pins the snapshot to
    // an INSTANT: it is in the second snapshot's data set and not in the first's.
    let late = command("origin", &mut origin, &["SET", "after-the-snapshot", "late"]).await;
    assert_eq!(
        late,
        Resp::Status("OK".to_string()),
        "the source cache must take a key written after its first snapshot, got {late:?}"
    );
    let (late_arn, late_size) = serverless_snapshot(&emulator, "snap-late", "origin").await;
    assert!(
        late_size > early_size,
        "case \"snap-late\": a snapshot taken after another key was written must report a larger \
         data set, got {late_size} after {early_size}"
    );

    // The axis: WHICH snapshots a restore names, and how many of them. Each row's cache is
    // created from nothing but those ARNs, so what it answers is what they captured.
    let keys: Vec<&str> = rows.iter().map(|row| row.key).collect();
    let cases: [(&str, &[&str], &[&str]); 5] = [
        // The recorded row: one snapshot, one restore.
        ("restored-early", &[early_arn.as_str()], &["after-the-snapshot"]),
        // The same snapshot restored a second time: restoring does not consume one.
        ("restored-early-again", &[early_arn.as_str()], &["after-the-snapshot"]),
        // The later snapshot, which carries the key the earlier one does not.
        ("restored-late", &[late_arn.as_str()], &[]),
        // Two snapshots at once: the keys of both, the later ARN's copy winning.
        (
            "restored-from-both",
            &[early_arn.as_str(), late_arn.as_str()],
            &[],
        ),
        // A snapshot of a cache that held nothing restores a cache that holds nothing.
        ("restored-from-nothing", &[], &[]),
    ];
    for (name, arns, absent) in cases {
        let mut session = restored_session(&emulator, name, arns).await;
        if arns.is_empty() {
            // The "restored-from-nothing" row is created from no ARN at all: every key of
            // the table is absent from it, which is what makes the rows above about the
            // snapshots rather than about the keys.
            let mut none_of_them: Vec<&str> = keys.clone();
            none_of_them.push("after-the-snapshot");
            assert_reads_back(name, &mut session, &[], &none_of_them).await;
            continue;
        }
        assert_reads_back(name, &mut session, &rows, absent).await;
        if name == "restored-late" || name == "restored-from-both" {
            let reply = command(name, &mut session, &["GET", "after-the-snapshot"]).await;
            assert_eq!(
                reply,
                Resp::Bulk("late".to_string()),
                "case {name}: the key written before this snapshot must be restored, got {reply:?}"
            );
        }
        // A restored cache's data set is its OWN: what it writes does not reach the
        // snapshot it came from, nor the cache that snapshot was taken of.
        let own = format!("written-in-{name}");
        command(name, &mut session, &["SET", &own, "mine"]).await;
        let from_origin = command("origin", &mut origin, &["GET", &own]).await;
        assert_eq!(
            from_origin,
            Resp::Nil,
            "case {name}: a key written in the restored cache must not appear in the source \
             cache, got {from_origin:?}"
        );
    }

    // The snapshot outlives the cache it was taken of: the source cache is deleted, stops
    // resolving, and a cache restored from its snapshot afterwards still holds the data.
    let deleted = emulator
        .call(
            "DeleteServerlessCache",
            &params(&[("ServerlessCacheName", "origin")]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "the source cache must be deletable, got {} {}",
        deleted.status, deleted.body
    );
    let gone = poll_until(
        &emulator,
        REGION,
        "origin",
        "the deleted cache ceasing to resolve",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", "origin")]),
        |reply| reply.status == 404,
    )
    .await;
    assert_eq!(
        gone.error_code(),
        "ServerlessCacheNotFoundFault",
        "a deleted cache must stop resolving, got {} {}",
        gone.status,
        gone.body
    );
    let mut after_delete = restored_session(&emulator, "restored-after-delete", &[&early_arn]).await;
    assert_reads_back(
        "restored-after-delete",
        &mut after_delete,
        &rows,
        &["after-the-snapshot"],
    )
    .await;
}

#[tokio::test]
async fn a_restore_naming_a_snapshot_this_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // One snapshot really does exist, so each row below is refused for naming NO snapshot
    // rather than because this account has none at all.
    serverless_cache(&emulator, "has-a-snapshot", &[]).await;
    let (arn, _) = serverless_snapshot(&emulator, "the-one-there-is", "has-a-snapshot").await;

    // The axis: HOW the ARN fails to name a stored snapshot — a well-formed ARN of a
    // snapshot nobody took, the right ARN with another snapshot name in it, a value that is
    // no ARN at all, and a list whose first ARN resolves and whose second does not.
    let other_name = arn.replace("the-one-there-is", "never-taken");
    let cases: [(&str, Vec<String>); 4] = [
        (
            "an ARN of a snapshot nobody took",
            vec!["arn:aws:elasticache:us-east-1:000000000000:serverlesscachesnapshot:none".to_string()],
        ),
        ("this account's ARN with another name in it", vec![other_name]),
        ("a value that is no ARN", vec!["the-one-there-is".to_string()]),
        (
            "a list whose second ARN resolves to nothing",
            vec![arn.clone(), "arn:aws:elasticache:us-east-1:000000000000:serverlesscachesnapshot:nor-this".to_string()],
        ),
    ];
    for (case, arns) in cases {
        let mut pairs = params(&[("ServerlessCacheName", "refused"), ("Engine", "redis")]);
        for (index, value) in arns.iter().enumerate() {
            pairs.push((
                format!("SnapshotArnsToRestore.SnapshotArn.{}", index + 1),
                value.clone(),
            ));
        }
        let refused = emulator.call("CreateServerlessCache", &pairs).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {:?}",
            refused.error_code()
        );
        // Nothing was created: the refused name still resolves to no cache.
        let described = emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", "refused")]),
            )
            .await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store no cache, got {} {}",
            described.status, described.body
        );
    }
}

/// Read one replication group back once the clock has made it `available` AND its shard
/// holds `members` member clusters.
async fn group_available_with(emulator: &Emulator, id: &str, members: usize) -> Reply {
    poll_until(
        emulator,
        REGION,
        id,
        "the group reaching available with its members provisioned",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| {
            reply.text("ReplicationGroup/Status") == Some("available")
                && reply.all("NodeGroupMember/CacheClusterId").len() == members
        },
    )
    .await
}

/// The `(cache cluster id, role)` of every member of the group's one shard, in the order the
/// answer reports them.
fn members_and_roles(described: &Reply) -> Vec<(String, String)> {
    let ids = described.all("NodeGroupMember/CacheClusterId");
    let roles = described.all("NodeGroupMember/CurrentRole");
    assert_eq!(
        ids.len(),
        roles.len(),
        "every member must report its role, got {ids:?} and {roles:?}"
    );
    ids.into_iter()
        .zip(roles)
        .map(|(id, role)| (id.to_string(), role.to_string()))
        .collect()
}

/// The member holding the primary role, which is the one the shard's `PrimaryEndpoint`
/// addresses.
fn primary_of(case: &str, described: &Reply) -> String {
    let members = members_and_roles(described);
    let primaries: Vec<&(String, String)> =
        members.iter().filter(|(_, role)| role == "primary").collect();
    assert_eq!(
        primaries.len(),
        1,
        "case {case}: a shard has exactly one primary, got {members:?}"
    );
    primaries
        .first()
        .map(|(id, _)| id.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_scaled_shard_moves_its_primary_role_and_keeps_the_data_written_before_the_move() {
    let emulator = Emulator::new();
    let rows = written();
    let id = "scale-failover-rg";

    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "scale+failover rg"),
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t3.micro"),
                ("NumCacheClusters", "2"),
                ("AutomaticFailoverEnabled", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationGroup must answer 200, got {} {}",
        created.status, created.body
    );

    // The group as created: two members, one of them the primary, and a primary endpoint a
    // client can reach.
    let described = group_available_with(&emulator, id, 2).await;
    let node_group_id = described
        .text("NodeGroup/NodeGroupId")
        .unwrap_or_else(|| panic!("the group must publish its shard's id, got {}", described.body))
        .to_string();
    let before_scale = primary_of("as created", &described);
    let (address, port) = endpoint_of(
        &described,
        "PrimaryEndpoint/Address",
        "PrimaryEndpoint/Port",
        "the shard's primary endpoint",
    );

    // What the failover must not lose, written at the primary endpoint the group published.
    let mut session = open("as created", &address, &port).await;
    write_all("as created", &mut session, &rows).await;
    assert_reads_back("as created", &mut session, &rows, &["never-written"]).await;

    // The scale: the shard is asked for two replicas, so it ends up holding three members.
    let scaled = emulator
        .call(
            "IncreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", id),
                ("NewReplicaCount", "2"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        scaled.status, 200,
        "IncreaseReplicaCount must answer 200, got {} {}",
        scaled.status, scaled.body
    );
    assert_eq!(
        scaled.text("ReplicationGroup/Status"),
        Some("modifying"),
        "the scale's own answer reports the work in progress, got {:?}",
        scaled.text("ReplicationGroup/Status")
    );
    let described = group_available_with(&emulator, id, 3).await;
    let members = members_and_roles(&described);
    assert_eq!(
        members.len(),
        3,
        "a shard asked for two replicas holds three members, got {members:?}"
    );
    assert_eq!(
        members.iter().filter(|(_, role)| role == "replica").count(),
        2,
        "the two added members are replicas, got {members:?}"
    );
    assert_eq!(
        primary_of("after the scale", &described),
        before_scale,
        "adding a replica does not move the primary role, got {members:?}"
    );

    // The failover: the primary role moves to ANOTHER member of the shard.
    let failed_over = emulator
        .call(
            "TestFailover",
            &params(&[
                ("ReplicationGroupId", id),
                ("NodeGroupId", &node_group_id),
            ]),
        )
        .await;
    assert_eq!(
        failed_over.status, 200,
        "TestFailover must answer 200, got {} {}",
        failed_over.status, failed_over.body
    );
    assert_eq!(
        failed_over.text("ReplicationGroup/Status"),
        Some("modifying"),
        "the failover's own answer reports the node replacement in progress, got {:?}",
        failed_over.text("ReplicationGroup/Status")
    );
    let described = group_available_with(&emulator, id, 3).await;
    let after_failover = primary_of("after the failover", &described);
    assert_ne!(
        after_failover, before_scale,
        "the primary role must sit on a different member after the failover, got {:?}",
        members_and_roles(&described)
    );
    assert_eq!(
        members_and_roles(&described)
            .iter()
            .find(|(id, _)| id == &before_scale)
            .map(|(_, role)| role.as_str()),
        Some("replica"),
        "the member that was the primary must be a replica now, got {:?}",
        members_and_roles(&described)
    );

    // The data consequence: the key written before the failover is still readable at the
    // (re-read) primary endpoint, whichever member that now addresses.
    let (address, port) = endpoint_of(
        &described,
        "PrimaryEndpoint/Address",
        "PrimaryEndpoint/Port",
        "the shard's primary endpoint after the failover",
    );
    let mut session = open("after the failover", &address, &port).await;
    assert_reads_back("after the failover", &mut session, &rows, &["never-written"]).await;
    // And the promoted member takes writes, which is what holding the primary role means.
    let accepted = command(
        "after the failover",
        &mut session,
        &["SET", "written-after-the-failover", "yes"],
    )
    .await;
    assert_eq!(
        accepted,
        Resp::Status("OK".to_string()),
        "the member the primary role moved to must take writes, got {accepted:?}"
    );
}
