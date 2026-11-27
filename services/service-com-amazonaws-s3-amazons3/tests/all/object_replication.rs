// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a bucket's replication configuration DOES: the object
//! versions it copies, where, and what the source version then reports.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, payload elements) rather than any handler's
//! internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A write into a bucket with an ENABLED rule that covers the key copies the version to
//!    the destination the rule names, byte-identically and under the same key and version
//!    id, where it reads back as a `REPLICA`. A rule that does not cover the key — another
//!    prefix, another tag — and a rule that is `Disabled` copy nothing.
//! 2. The source version reports the replication: `PENDING` while it is in flight and
//!    `COMPLETED` once it has settled, and `FAILED` when the destination the rule names
//!    cannot be written.
//! 3. Nothing already in the bucket is copied: an object written before the configuration
//!    was set stays where it is.
//! 4. A delete marker is replicated only by a rule that asks for it, and the permanent
//!    delete of one version is never replicated.
//! 5. A write into the destination bucket does not come back: a replica is not replicated,
//!    and a bucket with no configuration of its own replicates nothing.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::text;

/// The `Role` every configuration in this suite names: "the Amazon Resource Name (ARN) of
/// the Identity and Access Management (IAM) role that Amazon S3 assumes when replicating
/// objects" (`ReplicationConfiguration$Role`).
const ROLE: &str = "arn:aws:iam::123456789012:role/s3-replication";

/// The span a source version spends reporting `PENDING` before its replication settles,
/// plus a margin — `crate::state::REPLICATION_SETTLES_AFTER`, which is this emulator's own
/// stand-in for AWS's asynchronous replication.
const SETTLES_AFTER: Duration = Duration::from_millis(3_200);

/// How many destinations ONE write replicates to —
/// `crate::state::MAX_REPLICATION_DESTINATIONS_PER_WRITE`, this emulator's own ceiling on
/// the work one write may do, since a stored configuration may name up to 1,000
/// destinations and an object may be 16 MiB.
const MAX_DESTINATIONS_PER_WRITE: usize = 10;

/// `PUT /<bucket>` then `PUT /<bucket>?versioning` with `Enabled` — the state a replication
/// configuration requires of both the source and the destination bucket.
async fn versioned_bucket(router: &MultiServiceRouter, name: &str) {
    let created = send(router, "PUT", &format!("/{name}"), &[], "").await;
    assert_eq!(
        created.status,
        200,
        "the bucket {name:?} must be created first, got {} with {:?}",
        created.status,
        text(&created)
    );
    let versioned = send(
        router,
        "PUT",
        &format!("/{name}?versioning"),
        &[],
        "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    assert_eq!(
        versioned.status,
        200,
        "versioning must be enabled on {name:?}, got {} with {:?}",
        versioned.status,
        text(&versioned)
    );
}

/// `PUT /<bucket>?replication` — `PutBucketReplication` with one rule, whose body is the
/// `<Rule>` element a case names.
async fn put_replication(router: &MultiServiceRouter, name: &str, rules: &str) -> Answer {
    let body = format!(
        "<ReplicationConfiguration><Role>{ROLE}</Role>{rules}</ReplicationConfiguration>"
    );
    send(router, "PUT", &format!("/{name}?replication"), &[], &body).await
}

/// One `<Rule>` replicating to `destination`, with `scope` (a `Prefix` or a `Filter`
/// element, or nothing at all) and `extra` (a `DeleteMarkerReplication` element, say).
///
/// A rule carrying a `Filter` is written in the configuration's V2 schema, which requires a
/// `Priority` and a `DeleteMarkerReplication` of every rule ("when you add the Filter element
/// in the configuration, you must also add the following elements: DeleteMarkerReplication,
/// Status, and Priority", `PutBucketReplication`'s own documentation), so both are named for
/// exactly those rules — these rows are about which objects a rule COVERS, and a configuration
/// S3 would refuse covers nothing. A `DeleteMarkerReplication` the case names itself stands
/// instead, which is what the delete-marker rows vary.
fn rule(destination: &str, status: &str, scope: &str, extra: &str) -> String {
    let v2 = scope.contains("<Filter>");
    let priority = if v2 { "<Priority>1</Priority>" } else { "" };
    let delete_markers = match v2 && !extra.contains("DeleteMarkerReplication") {
        true => "<DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>",
        false => "",
    };
    format!(
        "<Rule><ID>only-rule</ID>{priority}<Status>{status}</Status>{scope}{extra}\
         {delete_markers}\
         <Destination><Bucket>arn:aws:s3:::{destination}</Bucket></Destination></Rule>"
    )
}

/// `PUT /<bucket>/<key>` — `PutObject`, answering what the write reported.
async fn put_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "PUT", &format!("/{name}/{key}"), headers, body).await
}

/// `GET /<bucket>/<key>` — `GetObject`, optionally of one version.
async fn get_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    version_id: Option<&str>,
) -> Answer {
    let uri = match version_id {
        Some(version) => format!("/{name}/{key}?versionId={version}"),
        None => format!("/{name}/{key}"),
    };
    send(router, "GET", &uri, &[], "").await
}

/// `DELETE /<bucket>/<key>` — `DeleteObject`, of the key (laying a delete marker) or of one
/// version (a permanent delete).
async fn delete_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    version_id: Option<&str>,
) -> Answer {
    let uri = match version_id {
        Some(version) => format!("/{name}/{key}?versionId={version}"),
        None => format!("/{name}/{key}"),
    };
    send(router, "DELETE", &uri, &[], "").await
}

/// One row of the coverage table: what the rule selects, the key written, and whether that
/// write must be replicated.
struct CoverageCase {
    case: &'static str,
    /// The `Status` the rule carries — `Enabled` or `Disabled`
    /// (`smithy_query(query="shape",shape="com.amazonaws.s3#ReplicationRuleStatus")`).
    status: &'static str,
    /// The scope element the rule carries: a `Prefix`, a `Filter`, or nothing.
    scope: &'static str,
    key: &'static str,
    /// The `x-amz-tagging` header the write carries, as the URL query string that member is
    /// declared to be (`PutObjectRequest$Tagging`).
    tagging: Option<&'static str>,
    replicated: bool,
}

/// The coverage table: the rows differ along what the rule generalises over — whether the
/// rule is enabled, which form its selection takes (none at all, the deprecated `Prefix`,
/// `Filter.Prefix`, `Filter.Tag`, `Filter.And`), and whether the key and the tag set of the
/// write fall inside it.
fn coverage_cases() -> Vec<CoverageCase> {
    vec![
        CoverageCase {
            case: "no selection at all covers the whole bucket",
            status: "Enabled",
            scope: "",
            key: "anywhere/one.bin",
            tagging: None,
            replicated: true,
        },
        CoverageCase {
            case: "an empty Prefix covers the whole bucket",
            status: "Enabled",
            scope: "<Prefix></Prefix>",
            key: "anywhere/two.bin",
            tagging: None,
            replicated: true,
        },
        CoverageCase {
            case: "a Prefix the key is under",
            status: "Enabled",
            scope: "<Prefix>eligible/</Prefix>",
            key: "eligible/three.bin",
            tagging: None,
            replicated: true,
        },
        CoverageCase {
            case: "a Prefix the key is NOT under",
            status: "Enabled",
            scope: "<Prefix>eligible/</Prefix>",
            key: "elsewhere/four.bin",
            tagging: None,
            replicated: false,
        },
        CoverageCase {
            case: "a Filter.Prefix the key is under",
            status: "Enabled",
            scope: "<Filter><Prefix>eligible/</Prefix></Filter>",
            key: "eligible/five.bin",
            tagging: None,
            replicated: true,
        },
        CoverageCase {
            case: "a Filter.Prefix the key is NOT under",
            status: "Enabled",
            scope: "<Filter><Prefix>eligible/</Prefix></Filter>",
            key: "elsewhere/six.bin",
            tagging: None,
            replicated: false,
        },
        CoverageCase {
            case: "a Filter.Tag the object carries",
            status: "Enabled",
            scope: "<Filter><Tag><Key>copy</Key><Value>yes</Value></Tag></Filter>",
            key: "tagged/seven.bin",
            tagging: Some("copy=yes"),
            replicated: true,
        },
        CoverageCase {
            case: "a Filter.Tag the object does NOT carry",
            status: "Enabled",
            scope: "<Filter><Tag><Key>copy</Key><Value>yes</Value></Tag></Filter>",
            key: "tagged/eight.bin",
            tagging: Some("copy=no"),
            replicated: false,
        },
        CoverageCase {
            case: "a Filter.And whose prefix and tag both hold",
            status: "Enabled",
            scope: "<Filter><And><Prefix>eligible/</Prefix>\
                    <Tag><Key>copy</Key><Value>yes</Value></Tag></And></Filter>",
            key: "eligible/nine.bin",
            tagging: Some("copy=yes"),
            replicated: true,
        },
        CoverageCase {
            case: "a Filter.And whose prefix holds but whose tag does not",
            status: "Enabled",
            scope: "<Filter><And><Prefix>eligible/</Prefix>\
                    <Tag><Key>copy</Key><Value>yes</Value></Tag></And></Filter>",
            key: "eligible/ten.bin",
            tagging: None,
            replicated: false,
        },
        CoverageCase {
            case: "a Disabled rule that would otherwise cover the key",
            status: "Disabled",
            scope: "<Prefix>eligible/</Prefix>",
            key: "eligible/eleven.bin",
            tagging: None,
            replicated: false,
        },
    ]
}

#[tokio::test]
async fn an_enabled_rule_replicates_the_versions_it_covers_and_nothing_else() {
    for row in coverage_cases() {
        let router = router();
        let source = "crr-source";
        let destination = "crr-destination";
        versioned_bucket(&router, source).await;
        versioned_bucket(&router, destination).await;
        let configured =
            put_replication(&router, source, &rule(destination, row.status, row.scope, "")).await;
        assert_eq!(
            configured.status,
            200,
            "case {:?}: the configuration must be accepted, got {} with {:?}",
            row.case,
            configured.status,
            text(&configured)
        );

        let body = format!("the bytes of {}", row.key).into_bytes();
        let headers: Vec<(&str, &str)> = row
            .tagging
            .map(|tagging| vec![("x-amz-tagging", tagging)])
            .unwrap_or_default();
        let written = put_object(&router, source, row.key, &body, &headers).await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: the write must be accepted, got {} with {:?}",
            row.case,
            written.status,
            text(&written)
        );
        let version_id = written
            .header("x-amz-version-id")
            .unwrap_or_else(|| panic!("case {:?}: a versioned write must report an id", row.case));

        let replica = get_object(&router, destination, row.key, None).await;
        if !row.replicated {
            assert_eq!(
                replica.status,
                404,
                "case {:?}: expected nothing replicated to {destination:?}, got {} with {:?}",
                row.case,
                replica.status,
                text(&replica)
            );
            // And the source version reports no replication status at all: no enabled rule
            // covers it, so there is nothing to report.
            let read = get_object(&router, source, row.key, None).await;
            assert_eq!(
                read.header("x-amz-replication-status"),
                None,
                "case {:?}: expected no replication status on an uncovered version, got {:?}",
                row.case,
                read.header("x-amz-replication-status")
            );
            continue;
        }

        assert_eq!(
            replica.status,
            200,
            "case {:?}: expected the replica to be readable in {destination:?}, got {} with \
             {:?}",
            row.case,
            replica.status,
            text(&replica)
        );
        assert_eq!(
            replica.body.as_ref(),
            body.as_slice(),
            "case {:?}: expected the replica to be byte-identical, got {:?}",
            row.case,
            text(&replica)
        );
        assert_eq!(
            replica.header("etag"),
            written.header("etag"),
            "case {:?}: expected the replica to carry the source's entity tag, got {:?}",
            row.case,
            replica.header("etag")
        );
        assert_eq!(
            replica.header("x-amz-version-id").as_deref(),
            Some(version_id.as_str()),
            "case {:?}: expected the replica to keep the source's version id {version_id:?}, \
             got {:?}",
            row.case,
            replica.header("x-amz-version-id")
        );
        assert_eq!(
            replica.header("x-amz-replication-status").as_deref(),
            Some("REPLICA"),
            "case {:?}: expected the copy to report REPLICA, got {:?}",
            row.case,
            replica.header("x-amz-replication-status")
        );

        // The source version reports the replication in flight: the copy is there, and the
        // status it reports settles a span later (see the next test).
        let read = get_object(&router, source, row.key, None).await;
        assert_eq!(
            read.header("x-amz-replication-status").as_deref(),
            Some("PENDING"),
            "case {:?}: expected the source version to report PENDING straight after the \
             write, got {:?}",
            row.case,
            read.header("x-amz-replication-status")
        );
    }
}

#[tokio::test]
async fn the_source_version_settles_on_what_the_replication_did() {
    // (case, the destination the rule names, whether that destination still exists when the
    // write is made, the status the source version settles on).
    //
    // The rows differ along what the rule's destination IS when the object is written: a
    // bucket this emulator holds with versioning enabled, and one that was there when the
    // configuration was accepted and is gone by the time the write is made — the destination
    // a replication role cannot write, which is the case AWS reports as a failed replication
    // rather than as a refused configuration. (A configuration whose destination is absent
    // when the PUT is made is refused outright instead, which
    // [`crate::s3_replication_destination_validation`] pins.)
    let rows: [(&str, &str, bool, &str); 2] = [
        (
            "a destination that can be written",
            "crr-settled-dest",
            true,
            "COMPLETED",
        ),
        (
            "a destination that is gone when the write is made",
            "crr-absent-dest",
            false,
            "FAILED",
        ),
    ];

    for (case, destination, destination_stands, expected) in rows {
        let router = router();
        let source = "crr-settled-source";
        versioned_bucket(&router, source).await;
        // The destination is stood up for every row: a rule may name only a bucket that
        // exists and keeps object versions, so that is what the configuration is accepted
        // against.
        versioned_bucket(&router, destination).await;
        let configured =
            put_replication(&router, source, &rule(destination, "Enabled", "", "")).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the configuration must be accepted, got {} with {:?}",
            configured.status,
            text(&configured)
        );
        if !destination_stands {
            // Taken away AFTER the configuration was accepted, which is what leaves the rule
            // naming a destination nothing answers to when the write is replicated.
            let removed = send(&router, "DELETE", &format!("/{destination}"), &[], "").await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: the destination must be deleted, got {} with {:?}",
                removed.status,
                text(&removed)
            );
        }

        let key = "settling/object.bin";
        let written = put_object(&router, source, key, b"settling bytes", &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be accepted, got {} with {:?}",
            written.status,
            text(&written)
        );

        let pending = get_object(&router, source, key, None).await;
        assert_eq!(
            pending.header("x-amz-replication-status").as_deref(),
            Some("PENDING"),
            "case {case:?}: expected PENDING straight after the write, got {:?}",
            pending.header("x-amz-replication-status")
        );

        // Once the span the emulator gives AWS's asynchronous replication has passed, the
        // version reports what the replication really did — and keeps reporting it.
        tokio::time::sleep(SETTLES_AFTER).await;
        for read in 1..=2 {
            let settled = get_object(&router, source, key, None).await;
            assert_eq!(
                settled.header("x-amz-replication-status").as_deref(),
                Some(expected),
                "case {case:?}: expected read {read} to report {expected:?}, got {:?}",
                settled.header("x-amz-replication-status")
            );
        }
    }
}

#[tokio::test]
async fn an_object_written_before_the_rule_is_never_backfilled() {
    let router = router();
    let source = "crr-backfill-source";
    let destination = "crr-backfill-destination";
    versioned_bucket(&router, source).await;
    versioned_bucket(&router, destination).await;

    // Two keys written BEFORE the configuration, one of them twice, so the rows differ in
    // what the destination would have to backfill.
    for key in ["before/one.bin", "before/two.bin"] {
        let written = put_object(&router, source, key, b"written before the rule", &[]).await;
        assert_eq!(
            written.status, 200,
            "the write of {key:?} must be accepted, got {} with {:?}",
            written.status,
            text(&written)
        );
    }
    let configured = put_replication(&router, source, &rule(destination, "Enabled", "", "")).await;
    assert_eq!(
        configured.status, 200,
        "the configuration must be accepted, got {} with {:?}",
        configured.status,
        text(&configured)
    );

    for key in ["before/one.bin", "before/two.bin"] {
        let replica = get_object(&router, destination, key, None).await;
        assert_eq!(
            replica.status, 404,
            "expected {key:?} not to be backfilled into {destination:?}, got {} with {:?}",
            replica.status,
            text(&replica)
        );
    }

    // A write made WHILE the rule is in force is replicated, which is what shows the
    // absence above is the no-backfill rule rather than a configuration that does nothing.
    let after = put_object(&router, source, "after/three.bin", b"written after", &[]).await;
    assert_eq!(
        after.status, 200,
        "the write after the rule must be accepted, got {} with {:?}",
        after.status,
        text(&after)
    );
    let replica = get_object(&router, destination, "after/three.bin", None).await;
    assert_eq!(
        replica.status, 200,
        "expected the write made under the rule to be replicated, got {} with {:?}",
        replica.status,
        text(&replica)
    );
}

#[tokio::test]
async fn a_delete_marker_is_replicated_only_where_the_rule_asks_for_it() {
    // (case, the DeleteMarkerReplication element the rule carries, whether the marker is
    // replicated).
    //
    // The rows differ along the one member that decides it: absent, `Disabled`, `Enabled`
    // (`smithy_query(query="shape",shape="com.amazonaws.s3#DeleteMarkerReplicationStatus")`
    // publishes `[Disabled, Enabled]`).
    let rows: [(&str, &str, bool); 3] = [
        ("no DeleteMarkerReplication at all", "", false),
        (
            "DeleteMarkerReplication Disabled",
            "<DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>",
            false,
        ),
        (
            "DeleteMarkerReplication Enabled",
            "<DeleteMarkerReplication><Status>Enabled</Status></DeleteMarkerReplication>",
            true,
        ),
    ];

    for (case, markers, replicated) in rows {
        let router = router();
        let source = "crr-marker-source";
        let destination = "crr-marker-destination";
        versioned_bucket(&router, source).await;
        versioned_bucket(&router, destination).await;
        let configured =
            put_replication(&router, source, &rule(destination, "Enabled", "", markers)).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the configuration must be accepted, got {} with {:?}",
            configured.status,
            text(&configured)
        );

        let key = "marked/object.bin";
        let written = put_object(&router, source, key, b"a version to hide", &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be accepted, got {}",
            written.status
        );
        let version_id = written
            .header("x-amz-version-id")
            .unwrap_or_else(|| panic!("case {case:?}: a versioned write must report an id"));
        let replica = get_object(&router, destination, key, None).await;
        assert_eq!(
            replica.status, 200,
            "case {case:?}: the version itself must be replicated first, got {}",
            replica.status
        );

        // A delete naming NO version lays a delete marker, which is what a rule may
        // replicate.
        let deleted = delete_object(&router, source, key, None).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete must answer 204, got {} with {:?}",
            deleted.status,
            text(&deleted)
        );
        let after_delete = get_object(&router, destination, key, None).await;
        let expected = if replicated { 404 } else { 200 };
        assert_eq!(
            after_delete.status, expected,
            "case {case:?}: expected the destination read to answer {expected} after the \
             delete, got {} with {:?}",
            after_delete.status,
            text(&after_delete)
        );
        // Whatever the marker did, the replicated VERSION is still there under its own id:
        // a marker hides a version, it does not take it away.
        let by_version = get_object(&router, destination, key, Some(&version_id)).await;
        assert_eq!(
            by_version.status, 200,
            "case {case:?}: expected the replicated version to stay addressable by its id, \
             got {} with {:?}",
            by_version.status,
            text(&by_version)
        );

        // The permanent delete of ONE version is never replicated: the destination keeps
        // its copy of that version.
        let purged = delete_object(&router, source, key, Some(&version_id)).await;
        assert_eq!(
            purged.status, 204,
            "case {case:?}: the version delete must answer 204, got {} with {:?}",
            purged.status,
            text(&purged)
        );
        let still_there = get_object(&router, destination, key, Some(&version_id)).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: expected the version delete not to be replicated, got {} with \
             {:?}",
            still_there.status,
            text(&still_there)
        );
    }
}

#[tokio::test]
async fn a_write_to_the_destination_never_propagates_back() {
    let router = router();
    let source = "crr-oneway-source";
    let destination = "crr-oneway-destination";
    versioned_bucket(&router, source).await;
    versioned_bucket(&router, destination).await;
    let configured = put_replication(&router, source, &rule(destination, "Enabled", "", "")).await;
    assert_eq!(
        configured.status, 200,
        "the configuration must be accepted, got {} with {:?}",
        configured.status,
        text(&configured)
    );
    // The destination replicates BACK to the source, so every copy the source makes lands
    // in a bucket whose own rule covers it: what keeps it from coming home again is that a
    // replica is not replicated, not the absence of a rule.
    let back = put_replication(
        &router,
        destination,
        &rule(source, "Enabled", "", ""),
    )
    .await;
    assert_eq!(
        back.status, 200,
        "the configuration pointing back must be accepted, got {} with {:?}",
        back.status,
        text(&back)
    );

    // Two rows: a key only the destination holds, and the key the source replicated — so
    // the rule covers it and a copy back would be visible either way.
    let replicated = put_object(&router, source, "shared/one.bin", b"from the source", &[]).await;
    assert_eq!(
        replicated.status, 200,
        "the source write must be accepted, got {}",
        replicated.status
    );
    // The replica the source made is in the destination, under the destination's own rule,
    // and is not replicated anywhere: it reports REPLICA rather than a replication of its
    // own, and the source still holds the version it wrote.
    let replica = get_object(&router, destination, "shared/one.bin", None).await;
    assert_eq!(
        replica.header("x-amz-replication-status").as_deref(),
        Some("REPLICA"),
        "expected the copy to stay a REPLICA under the destination's own rule, got {:?}",
        replica.header("x-amz-replication-status")
    );
    let source_version = get_object(&router, source, "shared/one.bin", None).await;
    assert_eq!(
        source_version.body.as_ref(),
        b"from the source",
        "expected the source to keep the bytes it wrote, got {:?}",
        text(&source_version)
    );

    for (key, body) in [
        ("destination-only.bin", b"written in the destination".to_vec()),
        ("shared/one.bin", b"overwritten in the destination".to_vec()),
    ] {
        let written = put_object(&router, destination, key, &body, &[]).await;
        assert_eq!(
            written.status, 200,
            "the destination write of {key:?} must be accepted, got {}",
            written.status
        );
        // That write IS covered by the destination's own rule, so it replicates to the
        // source — which is what makes the previous assertions about the SOURCE's own
        // version the interesting ones: the copy the source made never came back, and only
        // a write made in the destination itself reaches it.
        let back = get_object(&router, source, key, None).await;
        assert_eq!(
            back.status, 200,
            "expected the destination's OWN write of {key:?} to replicate to the source, got \
             {} with {:?}",
            back.status,
            text(&back)
        );
        assert_eq!(
            back.header("x-amz-replication-status").as_deref(),
            Some("REPLICA"),
            "expected that copy to be a REPLICA in the source, got {:?}",
            back.header("x-amz-replication-status")
        );
    }
}

#[tokio::test]
async fn one_write_replicates_to_no_more_destinations_than_the_ceiling() {
    let router = router();
    let source = "crr-fanout-source";
    versioned_bucket(&router, source).await;

    // One destination past the ceiling, each with its own rule covering the whole bucket:
    // the work one write would otherwise do is the number of destinations times the size
    // of the object, and both are a caller's to choose.
    let destinations: Vec<String> = (0..=MAX_DESTINATIONS_PER_WRITE)
        .map(|index| format!("crr-fanout-destination-{index}"))
        .collect();
    let mut rules = String::new();
    for (index, destination) in destinations.iter().enumerate() {
        versioned_bucket(&router, destination).await;
        rules.push_str(&format!(
            "<Rule><ID>rule-{index}</ID><Status>Enabled</Status><Priority>{index}</Priority>\
             <Filter><Prefix></Prefix></Filter>\
             <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>\
             <Destination><Bucket>arn:aws:s3:::{destination}</Bucket></Destination></Rule>"
        ));
    }
    let configured = put_replication(&router, source, &rules).await;
    assert_eq!(
        configured.status, 200,
        "the configuration must be accepted, got {} with {:?}",
        configured.status,
        text(&configured)
    );

    let key = "fanned/out.bin";
    let written = put_object(&router, source, key, b"fanned out bytes", &[]).await;
    assert_eq!(
        written.status, 200,
        "the write must be accepted, got {} with {:?}",
        written.status,
        text(&written)
    );

    // The destinations inside the ceiling hold the replica; the one past it does not.
    let mut holding = 0usize;
    for destination in &destinations {
        let replica = get_object(&router, destination, key, None).await;
        if replica.status == 200 {
            holding += 1;
            assert_eq!(
                replica.header("x-amz-replication-status").as_deref(),
                Some("REPLICA"),
                "the copy in {destination:?} must report REPLICA, got {:?}",
                replica.header("x-amz-replication-status")
            );
        }
    }
    assert_eq!(
        holding, MAX_DESTINATIONS_PER_WRITE,
        "expected exactly {MAX_DESTINATIONS_PER_WRITE} destinations to hold the replica, got \
         {holding} of {}",
        destinations.len()
    );

    // And the source version says so: the copies past the ceiling were not made, so the
    // replication did not complete. Waited for by looking at the version again until its
    // replication settles, never for a span — through the crate's one reading of that wait
    // ([`super::replication_settling`]).
    let settled = super::replication_settling::settled_replication_status(
        || async {
            get_object(&router, source, key, None)
                .await
                .header("x-amz-replication-status")
        },
        &format!("the write fanned out over the ceiling ({source}/{key})"),
    )
    .await;
    assert_eq!(
        settled, "FAILED",
        "expected the source version to settle on FAILED past the ceiling, got {settled:?}"
    );
}

#[tokio::test]
async fn a_rule_stores_the_replica_under_the_storage_class_it_names() {
    // (case, the `StorageClass` the rule's Destination names, the class the replica must be
    // stored under). `None` is "By default, Amazon S3 uses the storage class of the source
    // object to create the object replica" (`Destination$StorageClass`); every named value
    // is one of the model's own `StorageClass` enum
    // (`smithy_query(query="shape",shape="com.amazonaws.s3#StorageClass")`).
    let rows: [(&str, Option<&str>, &str); 4] = [
        ("no StorageClass keeps the source's", None, "STANDARD"),
        ("STANDARD_IA", Some("STANDARD_IA"), "STANDARD_IA"),
        ("GLACIER_IR", Some("GLACIER_IR"), "GLACIER_IR"),
        (
            "ONEZONE_IA",
            Some("ONEZONE_IA"),
            "ONEZONE_IA",
        ),
    ];

    for (case, named, expected) in rows {
        let router = router();
        let source = "crr-class-source";
        let destination = "crr-class-destination";
        versioned_bucket(&router, source).await;
        versioned_bucket(&router, destination).await;
        let storage_class = named
            .map(|class| format!("<StorageClass>{class}</StorageClass>"))
            .unwrap_or_default();
        let one = format!(
            "<Rule><ID>only-rule</ID><Status>Enabled</Status>\
             <Destination><Bucket>arn:aws:s3:::{destination}</Bucket>{storage_class}\
             </Destination></Rule>"
        );
        let configured = put_replication(&router, source, &one).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the configuration must be accepted, got {} with {:?}",
            configured.status,
            text(&configured)
        );

        let key = "classed/object.bin";
        let written = put_object(&router, source, key, b"classified bytes", &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be accepted, got {}",
            written.status
        );
        let replica = get_object(&router, destination, key, None).await;
        assert_eq!(
            replica.status, 200,
            "case {case:?}: the replica must be readable, got {} with {:?}",
            replica.status,
            text(&replica)
        );
        // `STANDARD` is reported as no header at all ("Amazon S3 returns this header for all
        // objects except for S3 Standard storage class objects",
        // `GetObjectOutput$StorageClass`).
        let reported = replica
            .header("x-amz-storage-class")
            .unwrap_or_else(|| "STANDARD".to_string());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the replica to be stored as {expected:?}, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn two_rules_naming_one_destination_are_settled_by_priority() {
    let router = router();
    let source = "crr-priority-source";
    let destination = "crr-priority-destination";
    let other = "crr-priority-other";
    versioned_bucket(&router, source).await;
    versioned_bucket(&router, destination).await;
    versioned_bucket(&router, other).await;

    // Two rules covering the same key for the SAME destination, differing in priority and
    // in the storage class they ask for, plus a third naming ANOTHER destination — which
    // conflicts with neither and so applies on its own.
    let rules = format!(
        "<Rule><ID>low</ID><Status>Enabled</Status><Priority>1</Priority>\
         <Filter><Prefix>shared/</Prefix></Filter>\
         <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>\
         <Destination><Bucket>arn:aws:s3:::{destination}</Bucket>\
         <StorageClass>STANDARD_IA</StorageClass></Destination></Rule>\
         <Rule><ID>high</ID><Status>Enabled</Status><Priority>2</Priority>\
         <Filter><Prefix>shared/</Prefix></Filter>\
         <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>\
         <Destination><Bucket>arn:aws:s3:::{destination}</Bucket>\
         <StorageClass>GLACIER_IR</StorageClass></Destination></Rule>\
         <Rule><ID>elsewhere</ID><Status>Enabled</Status><Priority>3</Priority>\
         <Filter><Prefix>shared/</Prefix></Filter>\
         <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>\
         <Destination><Bucket>arn:aws:s3:::{other}</Bucket></Destination></Rule>"
    );
    let configured = put_replication(&router, source, &rules).await;
    assert_eq!(
        configured.status, 200,
        "the configuration must be accepted, got {} with {:?}",
        configured.status,
        text(&configured)
    );

    let key = "shared/contested.bin";
    let written = put_object(&router, source, key, b"contested bytes", &[]).await;
    assert_eq!(
        written.status, 200,
        "the write must be accepted, got {}",
        written.status
    );

    // The conflict between the two rules naming one destination is settled by the higher
    // priority, so the replica there carries THAT rule's storage class.
    let contested = get_object(&router, destination, key, None).await;
    assert_eq!(
        contested.status, 200,
        "the replica in the contested destination must be readable, got {}",
        contested.status
    );
    assert_eq!(
        contested.header("x-amz-storage-class").as_deref(),
        Some("GLACIER_IR"),
        "expected the higher-priority rule's storage class on the replica, got {:?}",
        contested.header("x-amz-storage-class")
    );
    // The rule naming another destination conflicts with nothing, so it replicated too.
    let elsewhere = get_object(&router, other, key, None).await;
    assert_eq!(
        elsewhere.status, 200,
        "expected the rule naming another destination to replicate as well, got {} with {:?}",
        elsewhere.status,
        text(&elsewhere)
    );
    // And the source reports one replication for both of them.
    assert_eq!(
        get_object(&router, source, key, None)
            .await
            .header("x-amz-replication-status")
            .as_deref(),
        Some("PENDING"),
        "expected the source version to report its replication in flight"
    );
}
