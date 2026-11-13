// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ROLE a bucket's replication is carried out under: "the Amazon
//! Resource Name (ARN) of the Identity and Access Management (IAM) role that Amazon S3
//! assumes when replicating objects" (`ReplicationConfiguration$Role`).
//!
//! The rule under test: every copy a replication configuration calls for is made by S3
//! assuming that role and acting as it, so a role whose trust policy does not let the Amazon
//! S3 service principal assume it replicates NOTHING — no replica is written to any
//! destination, no delete marker is replicated — and the source version reports the failure
//! (`x-amz-replication-status: FAILED`, "if one or more destinations fails replication the
//! header will return FAILED", `HeadObjectOutput$ReplicationStatus`). The role's own
//! PERMISSIONS decide the same thing one destination at a time: a role denied the read of the
//! source's versions copies nothing anywhere, and one denied the write of a destination's
//! objects leaves that destination untouched while the rest of the configuration still
//! applies. The configuration itself is still accepted and reported back: the role is
//! resolved when the copy is made, not when the configuration is stored.
//!
//! A role nothing can be shown about is not held against the replication: this emulator
//! refuses only what IAM ANSWERED, so a role of this account IAM holds no record of — and one
//! IAM holds no identity policy for — leaves the copies to be made, which is why every other
//! replication suite in this crate, which mounts S3 alone, still replicates under a role no IAM
//! record backs. A role that STATES permissions is the authority on what it may do, so the
//! replication actions have to be allowed by them.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! principal the trust document names, in which form (one string or a list), which action
//! it names (the assume, a wildcard over it, another action), which effect it states, the
//! casing it names them in, how many statements it carries, and whether IAM holds the role
//! at all. Every call goes over the wire through the router, IAM's roles are made by IAM's
//! OWN `CreateRole`, and the replication status is read back with `HeadObject` — so what is
//! asserted is HTTP behaviour over the records the owning services wrote.

use std::sync::Arc;
use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;

/// The Region every request in this suite is signed for, and so the endpoint it addresses.
const REGION: &str = "us-east-1";

/// The service principal a replication role's trust policy has to admit for S3 to assume
/// it: S3's own, as the replication role's trust relationship names it (Amazon S3 User
/// Guide, *Setting up permissions for replication*). Spelled out here rather than read from
/// the crate so this suite pins the value.
const S3_SERVICE_PRINCIPAL: &str = "s3.amazonaws.com";

/// The IAM action a trust policy grants for a principal to be able to assume the role.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// The role name every row's replication configuration names.
const ROLE_NAME: &str = "s3-replication-role";

/// The span a source version spends reporting `PENDING` before its replication settles,
/// plus a margin — `crate::state::REPLICATION_SETTLES_AFTER`, this emulator's stand-in for
/// AWS's asynchronous replication.
const SETTLES_AFTER: Duration = Duration::from_millis(3_200);

/// An emulator holding S3 beside the IAM the role resolution asks: the question is put over
/// the emulator's own in-process callback, so IAM has to be mounted for it to answer at all.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here is signed: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One S3 request, addressed path-style to the Regional endpoint.
    async fn s3(&self, method: &str, uri: &str, headers: &[(&str, &str)], body: &[u8]) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// One awsQuery call to IAM, as the account's own credential.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/iam/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create the role [`ROLE_NAME`] through IAM's own `CreateRole`, carrying `trust` as its
    /// `AssumeRolePolicyDocument`.
    async fn create_role(&self, case: &str, trust: &str) {
        let (status, body) = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", ROLE_NAME.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the role must succeed, got {status} with {body}"
        );
    }

    /// Embed `document` in the role [`ROLE_NAME`] as an inline permissions policy through
    /// IAM's own `PutRolePolicy` — the policies a simulation of what the role may DO reads.
    async fn put_role_policy(&self, case: &str, name: &str, document: &str) {
        let (status, body) = self
            .iam(
                "PutRolePolicy",
                &[
                    ("RoleName", ROLE_NAME.to_string()),
                    ("PolicyName", name.to_string()),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: embedding the policy must succeed, got {status} with {body}"
        );
    }

    /// `PUT /<bucket>` then `PUT /<bucket>?versioning` with `Enabled` — the state a
    /// replication configuration requires of both the source and the destination bucket.
    async fn versioned_bucket(&self, case: &str, name: &str) {
        let created = self.s3("PUT", &format!("/{name}"), &[], b"").await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the bucket {name:?} must be created, got {} with {:?}",
            created.status,
            text(&created)
        );
        let versioned = self
            .s3(
                "PUT",
                &format!("/{name}?versioning"),
                &[("content-type", "application/xml")],
                b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
            )
            .await;
        assert_eq!(
            versioned.status,
            200,
            "case {case:?}: versioning must be enabled on {name:?}, got {} with {:?}",
            versioned.status,
            text(&versioned)
        );
    }

    /// `PUT /<bucket>?replication` — one enabled rule replicating the whole bucket to
    /// `destination` under `role_arn`, carrying `extra` (a `DeleteMarkerReplication`
    /// element, say).
    async fn put_replication(
        &self,
        bucket: &str,
        role_arn: &str,
        destination: &str,
        extra: &str,
    ) -> Answer {
        let body = format!(
            "<ReplicationConfiguration><Role>{role_arn}</Role>\
             <Rule><ID>only-rule</ID><Status>Enabled</Status>{extra}\
             <Destination><Bucket>arn:aws:s3:::{destination}</Bucket></Destination></Rule>\
             </ReplicationConfiguration>"
        );
        self.s3(
            "PUT",
            &format!("/{bucket}?replication"),
            &[("content-type", "application/xml")],
            body.as_bytes(),
        )
        .await
    }

    /// `PUT /<bucket>/<key>` — `PutObject`.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) -> Answer {
        self.s3(
            "PUT",
            &format!("/{bucket}/{key}"),
            &[("content-type", "application/octet-stream")],
            body,
        )
        .await
    }

    /// `HEAD /<bucket>/<key>` — `HeadObject`, which is where `x-amz-replication-status` is
    /// read.
    async fn head_object(&self, bucket: &str, key: &str) -> Answer {
        self.s3("HEAD", &format!("/{bucket}/{key}"), &[], b"").await
    }

    /// `GET /<bucket>/<key>` — `GetObject`.
    async fn get_object(&self, bucket: &str, key: &str) -> Answer {
        self.s3("GET", &format!("/{bucket}/{key}"), &[], b"").await
    }

    /// `DELETE /<bucket>/<key>` — `DeleteObject` of the key, which lays a delete marker on a
    /// versioned bucket.
    async fn delete_object(&self, bucket: &str, key: &str) -> Answer {
        self.s3("DELETE", &format!("/{bucket}/{key}"), &[], b"").await
    }
}

/// The ARN of the role [`ROLE_NAME`] in this emulator's account.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE_NAME}",
        engine::emulator_account_id()
    )
}

/// A trust document with one statement: `effect` for the `Principal.Service` element
/// `service` (written as JSON, so a row may name one string or a list) performing `action`.
fn trust(effect: &str, service: &str, action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":\"{action}\"}}]}}"
    )
}

/// The trust document a role S3's replication can assume carries.
fn s3_trust() -> String {
    trust("Allow", &format!("\"{S3_SERVICE_PRINCIPAL}\""), ASSUME_ROLE)
}

/// The response body as text, for an assertion's message.
fn text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the
/// round trip.
///
/// A copy: an integration-test module cannot be imported across crates, so every suite that
/// drives an awsQuery service holds its own.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One row of the trust table: what IAM holds for the role, and what the replication of a
/// write under it must then do.
struct Row {
    case: &'static str,
    /// The trust document IAM holds the role under, or `None` for a row whose role IAM
    /// holds no record of at all.
    trust: Option<String>,
    /// Whether the copy is made: `true` is a replica in the destination and a source version
    /// settling on `COMPLETED`, `false` is nothing in the destination and `FAILED`.
    replicated: bool,
}

/// The trust table: the rows differ along what the rule generalises over — which principal
/// the document names and in which form, which action it names, which effect it states, the
/// casing, how many statements it carries, and whether IAM holds the role at all.
fn trust_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a trust document admitting the S3 service principal",
            trust: Some(s3_trust()),
            replicated: true,
        },
        Row {
            case: "the S3 principal among a list of services",
            trust: Some(trust(
                "Allow",
                &format!("[\"firehose.amazonaws.com\",\"{S3_SERVICE_PRINCIPAL}\"]"),
                ASSUME_ROLE,
            )),
            replicated: true,
        },
        Row {
            case: "a service wildcard in the action",
            trust: Some(trust(
                "Allow",
                &format!("\"{S3_SERVICE_PRINCIPAL}\""),
                "sts:*",
            )),
            replicated: true,
        },
        Row {
            case: "the principal and the action named in another casing",
            trust: Some(trust("Allow", "\"S3.AMAZONAWS.COM\"", "STS:ASSUMEROLE")),
            replicated: true,
        },
        Row {
            case: "a document whose second statement admits the S3 principal",
            trust: Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {{\"Effect\":\"Allow\",\"Principal\":{{\"Service\":\"ec2.amazonaws.com\"}},\
                 \"Action\":\"{ASSUME_ROLE}\"}},\
                 {{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"Service\":\"{S3_SERVICE_PRINCIPAL}\"}},\
                 \"Action\":\"{ASSUME_ROLE}\"}}]}}"
            )),
            replicated: true,
        },
        Row {
            case: "a trust document admitting another service only",
            trust: Some(trust("Allow", "\"firehose.amazonaws.com\"", ASSUME_ROLE)),
            replicated: false,
        },
        Row {
            case: "a trust document naming an action that is not the assume",
            trust: Some(trust(
                "Allow",
                &format!("\"{S3_SERVICE_PRINCIPAL}\""),
                "s3:GetObject",
            )),
            replicated: false,
        },
        Row {
            case: "a trust document that explicitly denies the S3 principal",
            trust: Some(trust(
                "Deny",
                &format!("\"{S3_SERVICE_PRINCIPAL}\""),
                ASSUME_ROLE,
            )),
            replicated: false,
        },
        Row {
            case: "a trust document whose statement names no principal service at all",
            trust: Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
                 \"Action\":\"{ASSUME_ROLE}\"}}]}}",
                engine::emulator_account_id()
            )),
            replicated: false,
        },
        Row {
            // Nothing IAM answered withholds this role, so the replication proceeds: a
            // refusal has to be grounded in a record IAM really holds, and an execution role
            // is routinely provisioned out of band.
            case: "a role IAM holds no record of",
            trust: None,
            replicated: true,
        },
    ]
}

/// RULE: a write is copied to the destinations its bucket's configuration names only where
/// IAM shows S3 can assume the role that configuration names; under a role it cannot assume
/// nothing is copied and the source version settles on `FAILED`.
#[tokio::test]
async fn a_write_is_replicated_only_under_a_role_iam_shows_s3_can_assume() {
    for row in trust_rows() {
        let emulator = Emulator::new();
        if let Some(document) = row.trust.as_deref() {
            emulator.create_role(row.case, document).await;
        }
        let source = "replication-role-source";
        let destination = "replication-role-destination";
        emulator.versioned_bucket(row.case, source).await;
        emulator.versioned_bucket(row.case, destination).await;

        let configured = emulator
            .put_replication(source, &role_arn(), destination, "")
            .await;
        assert_eq!(
            configured.status,
            200,
            "case {:?}: the configuration must be accepted whatever the role trusts, got {} \
             with {:?}",
            row.case,
            configured.status,
            text(&configured)
        );

        let key = "replicated/object.bin";
        let body = b"the bytes a replication copies".to_vec();
        let written = emulator.put_object(source, key, &body).await;
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
            .unwrap_or_else(|| panic!("case {:?}: a versioned write reports an id", row.case));

        // Replication is asynchronous whichever way it ends, so the source version reports
        // PENDING straight after the write in every row.
        let pending = emulator.head_object(source, key).await;
        assert_eq!(
            pending.header("x-amz-replication-status").as_deref(),
            Some("PENDING"),
            "case {:?}: expected PENDING straight after the write, got {:?}",
            row.case,
            pending.header("x-amz-replication-status")
        );

        tokio::time::sleep(SETTLES_AFTER).await;
        let expected = if row.replicated { "COMPLETED" } else { "FAILED" };
        // Read twice: the settled outcome is terminal, so it keeps reporting it.
        for read in 1..=2 {
            let settled = emulator.head_object(source, key).await;
            assert_eq!(
                settled.header("x-amz-replication-status").as_deref(),
                Some(expected),
                "case {:?}: expected read {read} of the source to report {expected:?}, got {:?}",
                row.case,
                settled.header("x-amz-replication-status")
            );
        }

        let replica = emulator.get_object(destination, key).await;
        if row.replicated {
            assert_eq!(
                replica.status,
                200,
                "case {:?}: expected the copy in {destination:?}, got {} with {:?}",
                row.case,
                replica.status,
                text(&replica)
            );
            assert_eq!(
                replica.body.as_ref(),
                body.as_slice(),
                "case {:?}: expected the replica to carry the source's bytes, got {:?}",
                row.case,
                text(&replica)
            );
            assert_eq!(
                replica.header("x-amz-replication-status").as_deref(),
                Some("REPLICA"),
                "case {:?}: expected the copy to report REPLICA, got {:?}",
                row.case,
                replica.header("x-amz-replication-status")
            );
            assert_eq!(
                replica.header("x-amz-version-id").as_deref(),
                Some(version_id.as_str()),
                "case {:?}: expected the replica to keep the source's version id {version_id:?}, \
                 got {:?}",
                row.case,
                replica.header("x-amz-version-id")
            );
        } else {
            assert_eq!(
                replica.status,
                404,
                "case {:?}: expected nothing in {destination:?}, got {} with {:?}",
                row.case,
                replica.status,
                text(&replica)
            );
        }
    }
}

/// RULE: a delete marker is replicated under the same role, so a role S3 cannot assume
/// marks no destination either.
#[tokio::test]
async fn a_delete_marker_is_replicated_only_under_a_role_iam_shows_s3_can_assume() {
    // (case, the trust document IAM holds the role under, whether the marker is replicated)
    let rows: [(&str, String, bool); 2] = [
        ("a role S3 can assume", s3_trust(), true),
        (
            "a role trusting another service only",
            trust("Allow", "\"firehose.amazonaws.com\"", ASSUME_ROLE),
            false,
        ),
    ];

    for (case, document, replicated) in rows {
        let emulator = Emulator::new();
        emulator.create_role(case, &document).await;
        let source = "marker-role-source";
        let destination = "marker-role-destination";
        emulator.versioned_bucket(case, source).await;
        emulator.versioned_bucket(case, destination).await;
        let configured = emulator
            .put_replication(
                source,
                &role_arn(),
                destination,
                "<DeleteMarkerReplication><Status>Enabled</Status></DeleteMarkerReplication>",
            )
            .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the configuration must be accepted, got {} with {:?}",
            configured.status,
            text(&configured)
        );

        let key = "marked/object.bin";
        let written = emulator.put_object(source, key, b"bytes to delete").await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be accepted, got {} with {:?}",
            written.status,
            text(&written)
        );
        let deleted = emulator.delete_object(source, key).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete must be accepted, got {} with {:?}",
            deleted.status,
            text(&deleted)
        );

        // The destination answers 404 either way — the marker hides the replica, and under a
        // withheld role there is no replica to hide. What tells them apart is the delete
        // marker: "x-amz-delete-marker: true" is reported only where the marker really was
        // laid.
        let read = emulator.get_object(destination, key).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the destination's key to be gone, got {} with {:?}",
            read.status,
            text(&read)
        );
        let marker = read.header("x-amz-delete-marker");
        if replicated {
            assert_eq!(
                marker.as_deref(),
                Some("true"),
                "case {case:?}: expected the marker to be replicated, got {marker:?}"
            );
        } else {
            assert_eq!(
                marker, None,
                "case {case:?}: expected no marker in the destination, got {marker:?}"
            );
        }
    }
}

/// The source bucket of every permissions row, and its destination.
const PERMISSION_SOURCE: &str = "permission-role-source";
/// See [`PERMISSION_SOURCE`].
const PERMISSION_DESTINATION: &str = "permission-role-destination";

/// How long a poll waits for a replication to settle before the suite calls it stuck, and how
/// often it looks. The deadline is far longer than the span this emulator gives AWS's
/// asynchronous replication, so a loaded host cannot fail a row; what is asserted is the
/// CONDITION (a terminal status), never the time it took.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
/// See [`SETTLE_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The statements a role whose policy GRANTS the replication is written with: the actions the
/// Amazon S3 User Guide (*Setting up permissions for replication*) names, on the source's
/// versions and on the destination's objects.
fn granting_statements() -> String {
    format!(
        "{{\"Effect\":\"Allow\",\"Action\":\
         [\"s3:GetObjectVersionForReplication\",\"s3:GetObjectVersionAcl\"],\
         \"Resource\":{}}},\
         {{\"Effect\":\"Allow\",\"Action\":[\"s3:ReplicateObject\",\"s3:ReplicateDelete\"],\
         \"Resource\":{}}}",
        objects_of(PERMISSION_SOURCE),
        objects_of(PERMISSION_DESTINATION)
    )
}

/// A policy document carrying `statements`.
fn policy(statements: &str) -> String {
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")
}

/// The granting policy with `extra` statements after it — what a row that DENIES one action
/// states beside the grant.
fn granting_policy_with(extra: &str) -> String {
    if extra.is_empty() {
        return policy(&granting_statements());
    }
    policy(&format!("{},{extra}", granting_statements()))
}

/// One statement: `effect` for `actions` on `resources`, both written as JSON so a row may
/// name one value or a list.
fn statement(effect: &str, actions: &str, resources: &str) -> String {
    format!(
        "{{\"Effect\":\"{effect}\",\"Action\":{actions},\"Resource\":{resources}}}"
    )
}

/// The objects of `bucket` as a policy's `Resource` names them, quoted for a JSON document.
fn objects_of(bucket: &str) -> String {
    format!("\"arn:aws:s3:::{bucket}/*\"")
}

/// Poll the source version until its replication reports a TERMINAL status — `COMPLETED` or
/// `FAILED` — and report which. The wait is for the condition, bounded by
/// [`SETTLE_DEADLINE`].
async fn settled_replication_status(emulator: &Emulator, key: &str, case: &str) -> String {
    let deadline = ::std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = emulator.head_object(PERMISSION_SOURCE, key).await;
        let status = read.header("x-amz-replication-status").unwrap_or_default();
        if status == "COMPLETED" || status == "FAILED" {
            return status;
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {case:?}: the replication never reached a terminal status; the last one read \
             was {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the permissions table: what the role's own policy says, and what the
/// replication of a write and of a delete marker under it must then do.
struct PermissionRow {
    case: &'static str,
    /// The inline permissions policy embedded in the role, or `None` for a role carrying
    /// none at all.
    policy: Option<String>,
    /// Whether the object copy is made.
    replicated: bool,
    /// Whether a delete marker laid on the source is replicated.
    marker_replicated: bool,
}

/// The permissions table: the rows differ along what the rule generalises over — whether the
/// role STATES permissions at all, which ACTION a statement names (each replication action, a
/// wildcard over them, another action), which RESOURCE it names (this destination's objects,
/// the source's, another bucket's), and which EFFECT it states.
///
/// The actions are the ones replication is performed under (Amazon S3 User Guide, *Setting up
/// permissions for replication*): `s3:GetObjectVersionForReplication` on the source's versions,
/// `s3:ReplicateObject` and `s3:ReplicateDelete` on the destination's objects. A role that
/// states permissions has to be ALLOWED them — a copy its policies do not allow is one AWS
/// would not make either — while a role IAM holds no identity policy for is held back only by a
/// statement that denies one of them.
fn permission_rows() -> Vec<PermissionRow> {
    vec![
        PermissionRow {
            // Nothing IAM answered denies anything, and an empty policy set is no evidence:
            // a replication role is routinely provisioned out of band.
            case: "a role carrying no permissions policy at all",
            policy: None,
            replicated: true,
            marker_replicated: true,
        },
        PermissionRow {
            case: "a policy granting the replication actions on the right resources",
            policy: Some(granting_policy_with("")),
            replicated: true,
            marker_replicated: true,
        },
        PermissionRow {
            case: "a policy granting everything through wildcards",
            policy: Some(policy(&statement("Allow", "\"s3:*\"", "\"*\""))),
            replicated: true,
            marker_replicated: true,
        },
        PermissionRow {
            case: "a grant beside a deny of the write of a replica at the destination",
            policy: Some(granting_policy_with(&statement(
                "Deny",
                "\"s3:ReplicateObject\"",
                &objects_of(PERMISSION_DESTINATION),
            ))),
            replicated: false,
            // The marker is a replicated DELETE, which that statement says nothing about.
            marker_replicated: true,
        },
        PermissionRow {
            case: "a grant beside a deny of every replication action at the destination",
            policy: Some(granting_policy_with(&statement(
                "Deny",
                "\"s3:Replicate*\"",
                &objects_of(PERMISSION_DESTINATION),
            ))),
            replicated: false,
            marker_replicated: false,
        },
        PermissionRow {
            case: "a grant beside a deny of the replicated delete alone",
            policy: Some(granting_policy_with(&statement(
                "Deny",
                "\"s3:ReplicateDelete\"",
                &objects_of(PERMISSION_DESTINATION),
            ))),
            replicated: true,
            marker_replicated: false,
        },
        PermissionRow {
            case: "a grant beside a deny at ANOTHER bucket's objects",
            policy: Some(granting_policy_with(&statement(
                "Deny",
                "\"s3:ReplicateObject\"",
                &objects_of("some-other-bucket"),
            ))),
            replicated: true,
            marker_replicated: true,
        },
        PermissionRow {
            case: "a grant beside a deny of the read of the source's versions",
            policy: Some(granting_policy_with(&statement(
                "Deny",
                "\"s3:GetObjectVersionForReplication\"",
                &objects_of(PERMISSION_SOURCE),
            ))),
            // Every copy begins with that read, so nothing is copied and no marker is laid.
            replicated: false,
            marker_replicated: false,
        },
        PermissionRow {
            case: "a policy stating permissions that do not reach the destination at all",
            policy: Some(policy(&statement(
                "Allow",
                "\"s3:GetObjectVersionForReplication\"",
                &objects_of(PERMISSION_SOURCE),
            ))),
            // The role states what it may do and the destination write is not among it.
            replicated: false,
            marker_replicated: false,
        },
        PermissionRow {
            case: "a policy granting the object copy but not the replicated delete",
            policy: Some(policy(&format!(
                "{},{}",
                statement(
                    "Allow",
                    "\"s3:GetObjectVersionForReplication\"",
                    &objects_of(PERMISSION_SOURCE)
                ),
                statement(
                    "Allow",
                    "\"s3:ReplicateObject\"",
                    &objects_of(PERMISSION_DESTINATION)
                )
            ))),
            replicated: true,
            marker_replicated: false,
        },
        PermissionRow {
            case: "a policy about another action entirely",
            policy: Some(policy(&statement(
                "Allow",
                "\"s3:PutObject\"",
                &objects_of(PERMISSION_DESTINATION),
            ))),
            replicated: false,
            marker_replicated: false,
        },
    ]
}

/// RULE: a write is copied to a destination only where the role the configuration names may
/// make that copy — the read of the source's versions and the write of the destination's
/// objects — and a copy that could not be made settles the source version on `FAILED` with the
/// destination left untouched. A role that states permissions has to be allowed them; one IAM
/// holds no identity policy for is held back only by a statement that denies them.
#[tokio::test]
async fn a_write_is_replicated_only_where_the_roles_permissions_allow_it() {
    for row in permission_rows() {
        let emulator = Emulator::new();
        emulator.create_role(row.case, &s3_trust()).await;
        if let Some(document) = row.policy.as_deref() {
            emulator
                .put_role_policy(row.case, "replication", document)
                .await;
        }
        emulator.versioned_bucket(row.case, PERMISSION_SOURCE).await;
        emulator
            .versioned_bucket(row.case, PERMISSION_DESTINATION)
            .await;
        let configured = emulator
            .put_replication(
                PERMISSION_SOURCE,
                &role_arn(),
                PERMISSION_DESTINATION,
                "<DeleteMarkerReplication><Status>Enabled</Status></DeleteMarkerReplication>",
            )
            .await;
        assert_eq!(
            configured.status,
            200,
            "case {:?}: the configuration must be accepted whatever the role may do, got {} \
             with {:?}",
            row.case,
            configured.status,
            text(&configured)
        );

        let key = "replicated/object.bin";
        let body = b"the bytes a replication copies".to_vec();
        let written = emulator.put_object(PERMISSION_SOURCE, key, &body).await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: the write must be accepted, got {} with {:?}",
            row.case,
            written.status,
            text(&written)
        );

        let expected = if row.replicated { "COMPLETED" } else { "FAILED" };
        let settled = settled_replication_status(&emulator, key, row.case).await;
        assert_eq!(
            settled, expected,
            "case {:?}: expected the source version to settle on {expected:?}, got {settled:?}",
            row.case
        );

        let replica = emulator.get_object(PERMISSION_DESTINATION, key).await;
        if row.replicated {
            assert_eq!(
                replica.status,
                200,
                "case {:?}: expected the copy in the destination, got {} with {:?}",
                row.case,
                replica.status,
                text(&replica)
            );
            assert_eq!(
                replica.body.as_ref(),
                body.as_slice(),
                "case {:?}: expected the replica to carry the source's bytes, got {:?}",
                row.case,
                text(&replica)
            );
        } else {
            assert_eq!(
                replica.status,
                404,
                "case {:?}: expected the destination to hold nothing, got {} with {:?}",
                row.case,
                replica.status,
                text(&replica)
            );
        }

        // And the DELETE half of the same grant, on a key of its own so the object copy above
        // is left as it is: the marker reaches the destination only where the role is not
        // denied the replicated delete there.
        let marked = "marked/object.bin";
        let seeded = emulator
            .put_object(PERMISSION_SOURCE, marked, b"bytes to delete")
            .await;
        assert_eq!(
            seeded.status,
            200,
            "case {:?}: the write to delete must be accepted, got {} with {:?}",
            row.case,
            seeded.status,
            text(&seeded)
        );
        let deleted = emulator.delete_object(PERMISSION_SOURCE, marked).await;
        assert_eq!(
            deleted.status,
            204,
            "case {:?}: the delete must be accepted, got {} with {:?}",
            row.case,
            deleted.status,
            text(&deleted)
        );
        let read = emulator.get_object(PERMISSION_DESTINATION, marked).await;
        let marker = read.header("x-amz-delete-marker");
        if row.marker_replicated && row.replicated {
            assert_eq!(
                marker.as_deref(),
                Some("true"),
                "case {:?}: expected the marker to be replicated, got {marker:?}",
                row.case
            );
        } else if !row.marker_replicated {
            assert_eq!(
                marker, None,
                "case {:?}: expected no marker in the destination, got {marker:?}",
                row.case
            );
        }
    }
}

/// RULE: the role is resolved when a copy is made, not when the configuration is stored — so
/// `PutBucketReplication` accepts a configuration whatever its role trusts, and
/// `GetBucketReplication` reports that role back.
#[tokio::test]
async fn the_configuration_is_stored_and_reported_whatever_its_role_trusts() {
    for (case, document) in [
        ("a role S3 can assume", Some(s3_trust())),
        (
            "a role trusting another service only",
            Some(trust("Allow", "\"firehose.amazonaws.com\"", ASSUME_ROLE)),
        ),
        ("a role IAM holds no record of", None),
    ] {
        let emulator = Emulator::new();
        if let Some(document) = document.as_deref() {
            emulator.create_role(case, document).await;
        }
        let source = "configuration-role-source";
        let destination = "configuration-role-destination";
        emulator.versioned_bucket(case, source).await;
        emulator.versioned_bucket(case, destination).await;
        let configured = emulator
            .put_replication(source, &role_arn(), destination, "")
            .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the configuration must be accepted, got {} with {:?}",
            configured.status,
            text(&configured)
        );

        let reported = emulator
            .s3("GET", &format!("/{source}?replication"), &[], b"")
            .await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: the configuration must be reported back, got {} with {:?}",
            reported.status,
            text(&reported)
        );
        let role = reported.element("Role");
        assert_eq!(
            role.as_deref(),
            Some(role_arn().as_str()),
            "case {case:?}: expected the stored role to be reported, got {role:?}"
        );
    }
}
