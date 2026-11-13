// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHEN and FOR WHICH CALL a bucket's replication role is resolved.
//!
//! The rule under test: every object version a bucket records is copied to the destinations
//! its replication configuration names by Amazon S3 assuming the role that configuration
//! names — "the Identity and Access Management (IAM) role that Amazon S3 assumes when
//! replicating objects" (`ReplicationConfiguration$Role`) — so whichever CALL recorded that
//! version, and whatever the role trusted when the configuration was stored, a role whose
//! trust policy does not admit the Amazon S3 service principal at the moment of the write
//! copies nothing and the source version settles on the terminal `FAILED`
//! (`x-amz-replication-status`, "if one or more destinations fails replication the header
//! will return FAILED", `HeadObjectOutput$ReplicationStatus`), while a role whose trust
//! policy does admit it reaches `COMPLETED` and the replica is in the destination.
//!
//! Two suites, each a TABLE whose rows differ along what the rule generalises over:
//!
//! 1. WHICH call recorded the source version — `PutObject`, `CopyObject` and the
//!    `CompleteMultipartUpload` that assembles an upload — crossed with whether IAM shows S3
//!    can assume the role. The configured role is a property of the BUCKET, not of the call,
//!    so every one of those writes is replicated under it or not at all.
//! 2. WHEN the role was resolved: the trust policy in force at the write decides that
//!    write's replication, so one stored configuration replicates nothing, then everything,
//!    then nothing again as IAM's own `UpdateAssumeRolePolicy` grants and withdraws the
//!    assume — and the destination keeps only the version written while S3 could assume the
//!    role.
//!
//! Every call goes over the wire through a router holding S3 beside IAM over one shared
//! engine, the role and its trust policy are made by IAM's OWN `CreateRole` and
//! `UpdateAssumeRolePolicy`, and the replication status is read back with `HeadObject`: what
//! is asserted is HTTP behaviour over the records the owning services wrote.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;
use super::identity_authorization::{iam, router};
use super::multipart_upload_completion::{complete, completion_payload, initiate, upload_part};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{head_object, set_versioning};

/// The service principal a replication role's trust policy has to admit for S3 to assume it:
/// S3's own, as the replication role's trust relationship names it (Amazon S3 User Guide,
/// *Setting up permissions for replication*). Spelled out here rather than read from the
/// crate so this suite pins the value.
const S3_SERVICE_PRINCIPAL: &str = "s3.amazonaws.com";

/// A service principal that is NOT S3's, for the trust document a role S3 cannot assume
/// carries. One of the other service principals an execution role is routinely written for,
/// so the mistrusted leg differs from the healthy one in the principal alone.
const OTHER_SERVICE_PRINCIPAL: &str = "ec2.amazonaws.com";

/// The IAM action a trust policy grants for a principal to be able to assume the role.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// The role name every configuration in this suite names.
const ROLE_NAME: &str = "s3-replication-write-path-role";

/// The replication header `HeadObject` and `GetObject` report a version's replication outcome
/// in (`HeadObjectOutput$ReplicationStatus`).
const STATUS_HEADER: &str = "x-amz-replication-status";

/// The header a versioned write reports the id of the version it recorded in.
const VERSION_HEADER: &str = "x-amz-version-id";

/// The status a source version reports while its replication is still in flight.
const PENDING: &str = "PENDING";

/// The status a source version settles on once every copy its configuration called for was
/// made.
const COMPLETED: &str = "COMPLETED";

/// The terminal status a source version settles on when a copy its configuration called for
/// was not made.
const FAILED: &str = "FAILED";

/// The status the COPY carries in the destination bucket.
const REPLICA: &str = "REPLICA";

/// How long a settled status is waited FOR before the wait is called a failure — a generous
/// ceiling on an asynchronous outcome, not a measurement of one: replication is reported done
/// a short span past the write, and nothing here asserts how long that span is.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often the source version is looked at again while it still reports `PENDING`.
const SETTLE_POLL: Duration = Duration::from_millis(50);

/// The ARN of the role [`ROLE_NAME`] in this emulator's account.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE_NAME}",
        engine::emulator_account_id()
    )
}

/// A trust document with one statement allowing `service` to perform `sts:AssumeRole`.
fn trust_document(service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{service}\"}},\"Action\":\"{ASSUME_ROLE}\"}}]}}"
    )
}

/// Create the role [`ROLE_NAME`] through IAM's own `CreateRole`, carrying `trust` as its
/// `AssumeRolePolicyDocument`.
async fn create_role(router: &MultiServiceRouter, case: &str, trust: &str) {
    let (status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the role must succeed, got {status} with {body}"
    );
}

/// Replace that role's trust policy through IAM's own `UpdateAssumeRolePolicy` — the record
/// S3 reads when it resolves the role for the NEXT write.
async fn update_trust(router: &MultiServiceRouter, case: &str, trust: &str) {
    let (status, body) = iam(
        router,
        "UpdateAssumeRolePolicy",
        &[("RoleName", ROLE_NAME), ("PolicyDocument", trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: updating the role's trust policy must succeed, got {status} with {body}"
    );
}

/// A bucket with versioning enabled — the state a replication configuration requires of both
/// the source and the destination bucket.
async fn versioned_bucket(router: &MultiServiceRouter, name: &str) {
    bucket(router, name).await;
    set_versioning(router, name, "Enabled").await;
}

/// `PUT /<bucket>?replication` — one enabled rule replicating the whole bucket to
/// `destination` under the role [`role_arn`].
async fn put_replication(
    router: &MultiServiceRouter,
    case: &str,
    source: &str,
    destination: &str,
) {
    let body = format!(
        "<ReplicationConfiguration><Role>{}</Role>\
         <Rule><ID>only-rule</ID><Status>Enabled</Status>\
         <Destination><Bucket>arn:aws:s3:::{destination}</Bucket></Destination></Rule>\
         </ReplicationConfiguration>",
        role_arn()
    );
    let configured = super::bucket_lifecycle::send(
        router,
        "PUT",
        &format!("/{source}?replication"),
        &[],
        &body,
    )
    .await;
    assert_eq!(
        configured.status,
        200,
        "case {case:?}: the configuration must be accepted whatever the role trusts, got {} \
         with {:?}",
        configured.status,
        text(&configured)
    );
}

/// Which CALL records the source version whose replication is then read — the dimension the
/// first table spans.
#[derive(Clone, Copy, Debug)]
enum WritePath {
    /// `PutObject`: the payload travels in the request body.
    Put,
    /// `CopyObject`: the version is recorded from another object's bytes, named by
    /// `x-amz-copy-source`.
    Copy,
    /// `CompleteMultipartUpload`: the version is assembled from the parts of an upload.
    Multipart,
}

impl WritePath {
    /// The name this path goes by in an assertion's message.
    fn label(self) -> &'static str {
        match self {
            Self::Put => "PutObject",
            Self::Copy => "CopyObject",
            Self::Multipart => "CompleteMultipartUpload",
        }
    }

    /// Record one version of `key` in `bucket` through THIS call, answering the id of the
    /// version it recorded.
    ///
    /// Each path brings its own prerequisites: the copy needs a bucket to copy FROM (a plain
    /// one — what is under test is the DESTINATION bucket's replication), and the multipart
    /// completion needs an upload with a part in it.
    async fn write(
        self,
        router: &MultiServiceRouter,
        case: &str,
        bucket_name: &str,
        key: &str,
        body: &[u8],
    ) -> String {
        let written = match self {
            Self::Put => put_object(router, bucket_name, key, body, &[]).await,
            Self::Copy => {
                let origin = "replication-write-path-origin";
                bucket(router, origin).await;
                let seeded = put_object(router, origin, key, body, &[]).await;
                assert_eq!(
                    seeded.status,
                    200,
                    "case {case:?}: the bytes to copy must be stored first, got {} with {:?}",
                    seeded.status,
                    text(&seeded)
                );
                super::bucket_lifecycle::send_bytes(
                    router,
                    "PUT",
                    &format!("/{bucket_name}/{key}"),
                    &[("x-amz-copy-source", &format!("/{origin}/{key}"))],
                    b"",
                )
                .await
            }
            Self::Multipart => {
                let upload = initiate(router, bucket_name, key).await;
                let etag = upload_part(router, bucket_name, key, &upload, 1, body).await;
                complete(
                    router,
                    bucket_name,
                    key,
                    &upload,
                    &completion_payload(&[(1, etag)]),
                    &[],
                )
                .await
            }
        };
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the {} must be accepted, got {} with {:?}",
            self.label(),
            written.status,
            text(&written)
        );
        written.header(VERSION_HEADER).unwrap_or_else(|| {
            panic!(
                "case {case:?}: a versioned {} must report a version id, got headers {:?}",
                self.label(),
                written.headers
            )
        })
    }
}

/// The `x-amz-replication-status` the source version at `key` SETTLES on: `HeadObject` is
/// looked at again until it reports something other than `PENDING`, or until
/// [`SETTLE_DEADLINE`] has passed with nothing settled, because "Amazon S3 replicates objects
/// asynchronously" (`PutBucketReplication`) — the condition is waited for, never a span.
async fn settled_status(
    router: &MultiServiceRouter,
    case: &str,
    bucket_name: &str,
    key: &str,
) -> String {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let answer = head_object(router, bucket_name, key, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the source version must read back, got {} with {:?}",
            answer.status,
            text(&answer)
        );
        let status = answer.header(STATUS_HEADER).unwrap_or_else(|| {
            panic!(
                "case {case:?}: a version a replication rule covers must report {STATUS_HEADER}, \
                 got headers {:?}",
                answer.headers
            )
        });
        if status != PENDING {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the replication to settle, got {PENDING:?} still"
        );
        tokio::time::sleep(SETTLE_POLL).await;
    }
}

/// `GET /<bucket>/<key>` in the destination bucket — the replica, or the 404 of a destination
/// nothing was copied into.
async fn replica_of(router: &MultiServiceRouter, destination: &str, key: &str) -> Answer {
    get_object(router, destination, key, "", &[]).await
}

/// One row of the write-path table: which call records the source version, whether IAM shows
/// S3 can assume the configured role, and so whether the copy is made.
struct Row {
    path: WritePath,
    /// The service principal the role's trust document admits.
    trusts: &'static str,
    /// `true` is a replica in the destination and a source version settling on `COMPLETED`;
    /// `false` is nothing in the destination and `FAILED`.
    replicated: bool,
}

/// The table: every write path crossed with a role S3 can assume and a role it cannot, so the
/// outcome is attributable to the trust policy rather than to the call.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for path in [WritePath::Put, WritePath::Copy, WritePath::Multipart] {
        rows.push(Row {
            path,
            trusts: S3_SERVICE_PRINCIPAL,
            replicated: true,
        });
        rows.push(Row {
            path,
            trusts: OTHER_SERVICE_PRINCIPAL,
            replicated: false,
        });
    }
    rows
}

/// RULE: the role a bucket's replication configuration names is assumed for the copies EVERY
/// write into that bucket owes, whichever call recorded the version — so under a role S3
/// cannot assume no copy is made and the source version settles on `FAILED`, and under one it
/// can the replica is in the destination and the source settles on `COMPLETED`.
#[tokio::test]
async fn every_write_path_is_replicated_only_under_a_role_s3_can_assume() {
    for row in rows() {
        let case = format!(
            "{} under a role trusting {}",
            row.path.label(),
            row.trusts
        );
        let router = router();
        create_role(&router, &case, &trust_document(row.trusts)).await;

        let source = "replication-write-path-source";
        let destination = "replication-write-path-destination";
        versioned_bucket(&router, source).await;
        versioned_bucket(&router, destination).await;
        put_replication(&router, &case, source, destination).await;

        let key = "replicated/payload.bin";
        let body = format!("the bytes {} recorded", row.path.label()).into_bytes();
        let version_id = row.path.write(&router, &case, source, key, &body).await;

        let settled = settled_status(&router, &case, source, key).await;
        let expected = if row.replicated { COMPLETED } else { FAILED };
        assert_eq!(
            settled, expected,
            "case {case:?}: expected the source version to settle on {expected:?}, got \
             {settled:?}"
        );

        let replica = replica_of(&router, destination, key).await;
        if row.replicated {
            assert_eq!(
                replica.status,
                200,
                "case {case:?}: expected the copy in {destination:?}, got {} with {:?}",
                replica.status,
                text(&replica)
            );
            assert_eq!(
                replica.body.as_ref(),
                body.as_slice(),
                "case {case:?}: expected the replica to carry the source's bytes, got {:?}",
                text(&replica)
            );
            assert_eq!(
                replica.header(STATUS_HEADER).as_deref(),
                Some(REPLICA),
                "case {case:?}: expected the copy to report {REPLICA:?}, got {:?}",
                replica.header(STATUS_HEADER)
            );
            assert_eq!(
                replica.header(VERSION_HEADER).as_deref(),
                Some(version_id.as_str()),
                "case {case:?}: expected the replica to keep the source's version id \
                 {version_id:?}, got {:?}",
                replica.header(VERSION_HEADER)
            );
        } else {
            assert_eq!(
                replica.status,
                404,
                "case {case:?}: expected nothing in {destination:?}, got {} with {:?}",
                replica.status,
                text(&replica)
            );
        }
    }
}

/// RULE: the role is resolved for each WRITE rather than when the configuration is stored, so
/// the trust policy in force at that write decides its replication — one stored configuration
/// replicates nothing while S3 cannot assume the role, everything once it can, and nothing
/// again once the assume is withdrawn, and the destination holds only the version written
/// while S3 could assume it.
#[tokio::test]
async fn the_trust_policy_in_force_at_the_write_decides_that_writes_replication() {
    let case = "one configuration whose role's trust policy changes between writes";
    let router = router();
    // The role starts out trusting another service only, so the first write replicates
    // nothing — and the configuration naming it is still accepted and read back.
    create_role(&router, case, &trust_document(OTHER_SERVICE_PRINCIPAL)).await;

    let source = "replication-trust-change-source";
    let destination = "replication-trust-change-destination";
    versioned_bucket(&router, source).await;
    versioned_bucket(&router, destination).await;
    put_replication(&router, case, source, destination).await;

    let reported = super::bucket_lifecycle::send(
        &router,
        "GET",
        &format!("/{source}?replication"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        reported.element("Role").as_deref(),
        Some(role_arn().as_str()),
        "case {case:?}: expected the stored configuration to report the role it was given, got \
         {:?}",
        text(&reported)
    );

    let key = "replicated/ledger.bin";

    // Leg 1: S3 cannot assume the role, so nothing is copied.
    let withheld = b"written while the assume is withheld".to_vec();
    WritePath::Put
        .write(&router, case, source, key, &withheld)
        .await;
    let settled = settled_status(&router, case, source, key).await;
    assert_eq!(
        settled, FAILED,
        "case {case:?}: expected the write under a role S3 cannot assume to settle on \
         {FAILED:?}, got {settled:?}"
    );
    let empty = replica_of(&router, destination, key).await;
    assert_eq!(
        empty.status,
        404,
        "case {case:?}: expected nothing in {destination:?} yet, got {} with {:?}",
        empty.status,
        text(&empty)
    );

    // Leg 2: the SAME configuration, with the assume granted — the next write replicates.
    update_trust(&router, case, &trust_document(S3_SERVICE_PRINCIPAL)).await;
    let granted = b"written while the assume is granted".to_vec();
    let granted_version = WritePath::Put
        .write(&router, case, source, key, &granted)
        .await;
    let settled = settled_status(&router, case, source, key).await;
    assert_eq!(
        settled, COMPLETED,
        "case {case:?}: expected the write under a role S3 can assume to settle on \
         {COMPLETED:?}, got {settled:?}"
    );
    let copied = replica_of(&router, destination, key).await;
    assert_eq!(
        copied.status,
        200,
        "case {case:?}: expected the copy in {destination:?}, got {} with {:?}",
        copied.status,
        text(&copied)
    );
    assert_eq!(
        copied.body.as_ref(),
        granted.as_slice(),
        "case {case:?}: expected the replica to carry the bytes of the write that replicated, \
         got {:?}",
        text(&copied)
    );

    // Leg 3: the assume withdrawn again — the next write replicates nothing, and the
    // destination still holds the version leg 2 copied.
    update_trust(&router, case, &trust_document(OTHER_SERVICE_PRINCIPAL)).await;
    let withdrawn = b"written once the assume is withdrawn".to_vec();
    let withdrawn_version = WritePath::Put
        .write(&router, case, source, key, &withdrawn)
        .await;
    let settled = settled_status(&router, case, source, key).await;
    assert_eq!(
        settled, FAILED,
        "case {case:?}: expected the write made once the assume was withdrawn to settle on \
         {FAILED:?}, got {settled:?}"
    );
    let held = replica_of(&router, destination, key).await;
    assert_eq!(
        held.header(VERSION_HEADER).as_deref(),
        Some(granted_version.as_str()),
        "case {case:?}: expected {destination:?} to still hold the version written while the \
         assume was granted ({granted_version:?}) rather than {withdrawn_version:?}, got {:?}",
        held.header(VERSION_HEADER)
    );
    assert_eq!(
        held.body.as_ref(),
        granted.as_slice(),
        "case {case:?}: expected {destination:?} to still hold the bytes of the write that \
         replicated, got {:?}",
        text(&held)
    );
}
