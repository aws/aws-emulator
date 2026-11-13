// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of an Amazon S3 ACCESS POINT: which bucket a request addressed to one
//! reaches, and which policies decide what it may do there.
//!
//! The rule under test: **an access point is a named data path onto ONE bucket, so a request
//! addressed to the access point addresses the objects of the bucket it fronts, and is
//! authorized against the access point's own policy in addition to the caller's
//! identity-based documents and the bucket's policy — over the resource the access point
//! names (`<access-point-arn>/object/<key>`), with an explicit `Deny` in any of the three
//! refusing it 403 `AccessDenied`.** "Access points are named network endpoints that are
//! attached to buckets that you can use to perform S3 object operations", and "access point
//! policies … are evaluated in addition to the bucket policy" (Amazon S3 User Guide,
//! *Managing data access with Amazon S3 access points*); "an explicit deny in any policy
//! overrides any allows" (IAM User Guide, *Policy evaluation logic*). The path is the scope
//! of that document: the same object read by BUCKET NAME is not governed by it.
//!
//! The access point itself is created through S3 Control, which this crate cannot reach; its
//! data path is recorded here through the ONE store method that service writes it with
//! (`state::access_points::ComAmazonawsS3Amazons3State::record_access_point`), so what this
//! suite drives is the same record a `CreateAccessPoint` leaves behind. The cross-service
//! suite `core/router/tests/all/s3_access_point_policy_denial.rs` drives the whole chain
//! through both services' own APIs.
//!
//! The tables span what the rule generalises over rather than one recorded call:
//!
//! * the ADDRESS the access point is reached by — its ARN and its alias as the first path
//!   segment, and the `<name>-<account-id>` first label of its endpoint host, which is the
//!   form an AWS SDK rewrites an access point ARN into;
//! * the OPERATION — the read, the HEAD, the write, the listing and the delete, each
//!   addressed through the access point and read back through the bucket;
//! * the form of the KEY — a bare name and a nested one;
//! * what the access point's policy SAYS — a `Deny` selected by the calling principal's ARN,
//!   one selected by a wildcard principal, one scoped to a key prefix the request is outside
//!   of, one about another principal, and an `Allow` that is the only grant the request has;
//! * and where the grant comes from — the identity's own documents, the access point's
//!   policy, and a bucket policy that delegates to its access points with the
//!   `s3:DataAccessPointAccount` condition key.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_s3_amazons3::state::access_points::AccessPointRoute;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;
// The percent-encoder this test binary already owns — for an awsQuery body's components, and
// for an access point ARN travelling as ONE path segment.
use super::identity_authorization::form_encode;

/// The Region every request in this suite is signed for, and the Region the access point's
/// ARN names.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential, which writes this
/// suite's fixtures: no identity-based policy bounds it.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The bytes every object in this suite holds.
const BODY: &[u8] = b"the object behind the access point";

/// The code and status S3 answers an authorization refusal with (Amazon S3 "Error
/// responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";

/// The IAM action a read of an object's current version is authorized under.
const GET_OBJECT: &str = "s3:GetObject";

/// The reason clause IAM publishes for a request a resource-based document explicitly denies.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// An emulator holding S3 beside the two services a decision about a session's call reads
/// from: IAM (the role and its identity policies) and STS (the session a role is assumed
/// into). The engine is kept, because the access point's data path is recorded in its store.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One role session a row calls as.
struct Session {
    /// The session's access key id, which is where this emulator reads WHO is calling.
    access_key_id: String,
    /// The assumed-role session ARN `sts:GetCallerIdentity` reports, which a refusal names.
    session_arn: String,
    /// The ARN of the role the session is of — the value `aws:PrincipalArn` answers with.
    role_arn: String,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One S3 request to `host` at `uri`, signed with `access_key_id` in its SigV4 credential
    /// scope — which is where this emulator reads which principal a request speaks for.
    async fn s3_at(&self, access_key_id: &str, host: &str, method: &str, uri: &str, body: &[u8]) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
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

    /// One S3 request to the Region's PATH-STYLE endpoint, whose own host names no bucket.
    async fn s3(&self, access_key_id: &str, method: &str, uri: &str, body: &[u8]) -> Answer {
        self.s3_at(
            access_key_id,
            &format!("s3.{REGION}.amazonaws.com"),
            method,
            uri,
            body,
        )
        .await
    }

    /// One awsQuery call to IAM or STS, as the account's own credential.
    async fn query(&self, signing_name: &str, version: &str, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "the {action} call must succeed, got {status} with {body}"
        );
        body
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given), assume
    /// it, and answer the session that assume minted.
    async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Session {
        let account = engine::emulator_account_id();
        let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", role_arn.clone()),
                    ("RoleSessionName", role_name.to_string()),
                ],
            )
            .await;
        Session {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {assumed}")),
            session_arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {assumed}")),
            role_arn,
        }
    }

    /// `PUT /<bucket>` as the account.
    async fn create_bucket(&self, bucket: &str) {
        let created = self.s3(ACCOUNT_KEY, "PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            created.status,
            200,
            "the bucket {bucket:?} must be created first, got {} with {}",
            created.status,
            body_text(&created)
        );
    }

    /// `PUT /<bucket>/<key>` as the account — a fixture the rows address.
    async fn write_fixture(&self, bucket: &str, key: &str, body: &[u8]) {
        let written = self
            .s3(ACCOUNT_KEY, "PUT", &format!("/{bucket}/{key}"), body)
            .await;
        assert_eq!(
            written.status,
            200,
            "the fixture {key:?} must be stored first, got {} with {}",
            written.status,
            body_text(&written)
        );
    }

    /// `PUT /<bucket>?policy` as the account.
    async fn put_bucket_policy(&self, bucket: &str, document: &str) {
        let stored = self
            .s3(
                ACCOUNT_KEY,
                "PUT",
                &format!("/{bucket}?policy"),
                document.as_bytes(),
            )
            .await;
        assert_eq!(
            stored.status,
            204,
            "the bucket policy must be stored, got {} with {}",
            stored.status,
            body_text(&stored)
        );
    }

    /// Record the data path of an access point named `name` over `bucket`, through the one
    /// store method S3 Control's `CreateAccessPoint` writes it with, and answer it.
    fn create_access_point(&self, name: &str, bucket: &str) -> AccessPointRoute {
        let route = AccessPointRoute {
            arn: access_point_arn(name),
            name: name.to_string(),
            account_id: engine::emulator_account_id().to_string(),
            alias: format!("{name}-0123456789abcdefghijklmn-s3alias"),
            bucket: bucket.to_string(),
            // An Internet-origin access point: these rows are about the policy decision, and
            // `core/router/tests/all/s3_access_point_vpc_network_origin.rs` drives the
            // VPC-confined one.
            vpc_id: None,
            policy: None,
        };
        let state = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
            .expect("the object store must be reachable");
        assert!(
            state.lock().record_access_point(route.clone()),
            "the access point {name:?} must be recorded"
        );
        route
    }

    /// Put `policy` on the access point whose ARN is `arn`, or remove the document it carries
    /// — what S3 Control's `PutAccessPointPolicy` and `DeleteAccessPointPolicy` write.
    fn put_access_point_policy(&self, arn: &str, policy: Option<String>) {
        let state = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
            .expect("the object store must be reachable");
        assert!(
            state.lock().set_access_point_policy(arn, policy),
            "the access point {arn:?} must hold the policy"
        );
    }
}

/// The ARN of an access point named `name` in this emulator's account and Region.
fn access_point_arn(name: &str) -> String {
    format!(
        "arn:aws:s3:{REGION}:{}:accesspoint/{name}",
        engine::emulator_account_id()
    )
}

/// The resource an access point policy names one of the access point's objects by:
/// `<access-point-arn>/object/<key>` (Amazon S3 User Guide, *Access point policy examples*).
fn access_point_object(arn: &str, key: &str) -> String {
    format!("{arn}/object/{key}")
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// An answer's body as text.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// A policy document carrying exactly `statements`.
fn document(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The sentence S3 refuses a call with, written out here rather than built from the service's
/// own helper so the wording has something to be wrong about.
fn denial_message(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: \"{resource}\" {reason}")
}

/// The clause IAM publishes for a request no document grants.
fn no_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// One ADDRESS an access point is reached by: the case, the host the request is sent to, the
/// path a request about one key carries, and the path a LISTING of the access point carries.
struct Address {
    case: &'static str,
    host: String,
    /// The path of a request about `key` through this address.
    path: Box<dyn Fn(&str) -> String>,
    /// The path of a `ListObjectsV2` through this address, which addresses the access point
    /// itself rather than one of its objects.
    listing: String,
}

/// Every published address of `route`, each with the request form it arrives in — the three
/// of them (see the module docs).
fn addresses(route: &AccessPointRoute) -> Vec<Address> {
    let path_style_host = format!("s3.{REGION}.amazonaws.com");
    let arn = route.arn.clone();
    let encoded_arn = form_encode(&route.arn);
    let alias = route.alias.clone();
    vec![
        Address {
            case: "the access point ARN as the path's bucket label",
            host: path_style_host.clone(),
            path: Box::new(move |key| format!("/{}/{key}", form_encode(&arn))),
            listing: format!("/{encoded_arn}?list-type=2"),
        },
        Address {
            case: "the access point alias as the path's bucket label",
            host: path_style_host,
            path: Box::new({
                let alias = alias.clone();
                move |key| format!("/{alias}/{key}")
            }),
            listing: format!("/{alias}?list-type=2"),
        },
        Address {
            case: "the access point's own endpoint host",
            host: format!("{}.s3.{REGION}.amazonaws.com", route.endpoint_label()),
            path: Box::new(|key| format!("/{key}")),
            listing: "/?list-type=2".to_string(),
        },
    ]
}

/// Rule, first half: every address of an access point reaches the OBJECTS of the bucket it
/// fronts — the read, the HEAD, the write, the listing and the delete alike.
///
/// The rows differ along the address form and the form of the key; each write through the
/// access point is read back through the BUCKET's own address, which is what proves the two
/// addresses reach one store rather than two.
#[tokio::test]
async fn every_address_of_an_access_point_reaches_the_bucket_it_fronts() {
    let emulator = Emulator::new();
    let bucket = "access-point-fronted-bucket";
    emulator.create_bucket(bucket).await;
    let route = emulator.create_access_point("fronting-ap", bucket);

    // The keys each address is driven over: a bare name and a nested one.
    let keys = ["report.txt", "data/2026/report.csv"];
    for address in addresses(&route) {
        let case = address.case;
        for key in keys {
            emulator.write_fixture(bucket, key, BODY).await;

            // The read, through the access point: the bytes the bucket holds.
            let read = emulator
                .s3_at(ACCOUNT_KEY, &address.host, "GET", &(address.path)(key), b"")
                .await;
            assert_eq!(
                (read.status, body_text(&read).as_str()),
                (200, String::from_utf8_lossy(BODY).as_ref()),
                "case {case:?} {key:?}: the read through the access point must serve the \
                 object's bytes, got {} with {}",
                read.status,
                body_text(&read)
            );
            // The HEAD of the same object, which reports its length.
            let headed = emulator
                .s3_at(ACCOUNT_KEY, &address.host, "HEAD", &(address.path)(key), b"")
                .await;
            assert_eq!(
                (headed.status, headed.header("content-length")),
                (200, Some(BODY.len().to_string())),
                "case {case:?} {key:?}: the HEAD through the access point must report the \
                 object, got {} with {:?}",
                headed.status,
                headed.header("content-length")
            );

            // The WRITE, through the access point, read back through the bucket's own
            // address: one store, reached two ways.
            let written = b"written through the access point";
            let wrote = emulator
                .s3_at(
                    ACCOUNT_KEY,
                    &address.host,
                    "PUT",
                    &(address.path)(key),
                    written,
                )
                .await;
            assert_eq!(
                wrote.status,
                200,
                "case {case:?} {key:?}: the write through the access point must succeed, got \
                 {} with {}",
                wrote.status,
                body_text(&wrote)
            );
            let back = emulator
                .s3(ACCOUNT_KEY, "GET", &format!("/{bucket}/{key}"), b"")
                .await;
            assert_eq!(
                (back.status, body_text(&back).as_str()),
                (200, String::from_utf8_lossy(written).as_ref()),
                "case {case:?} {key:?}: the bucket's own address must read the bytes the \
                 access point write stored, got {} with {}",
                back.status,
                body_text(&back)
            );

            // The LISTING, through the access point: the key is one of the bucket's.
            let listed = emulator
                .s3_at(ACCOUNT_KEY, &address.host, "GET", &address.listing, b"")
                .await;
            assert!(
                listed.status == 200 && body_text(&listed).contains(key),
                "case {case:?} {key:?}: the listing through the access point must report the \
                 key, got {} with {}",
                listed.status,
                body_text(&listed)
            );

            // And the DELETE, after which the bucket's own address holds nothing there.
            let deleted = emulator
                .s3_at(
                    ACCOUNT_KEY,
                    &address.host,
                    "DELETE",
                    &(address.path)(key),
                    b"",
                )
                .await;
            assert_eq!(
                deleted.status,
                204,
                "case {case:?} {key:?}: the delete through the access point must succeed, got \
                 {} with {}",
                deleted.status,
                body_text(&deleted)
            );
            let gone = emulator
                .s3(ACCOUNT_KEY, "GET", &format!("/{bucket}/{key}"), b"")
                .await;
            assert_eq!(
                (gone.status, gone.error_code().as_str()),
                (404, "NoSuchKey"),
                "case {case:?} {key:?}: the bucket must hold nothing at the deleted key, got \
                 {} with {}",
                gone.status,
                body_text(&gone)
            );
        }
    }
}

/// Rule, the other half of the first: an access point's data plane serves the OBJECTS of the
/// bucket it fronts, so a bucket-level WRITE or DELETE addressed to the access point — the
/// creates, the deletes and the configuration writes that are not an access point's
/// operations — does not reach that bucket at all.
///
/// The rows differ along which bucket-level call is made, which method it uses and which of
/// the access point's addresses it is sent to. Each one asserts both halves: the call is
/// refused, and the bucket behind the access point still holds the object and the policy it
/// held before — which is what a rewrite that reached the bucket would have taken away.
#[tokio::test]
async fn a_bucket_level_write_addressed_to_an_access_point_does_not_reach_the_bucket() {
    let emulator = Emulator::new();
    let bucket = "access-point-protected-bucket";
    let key = "kept/report.txt";
    emulator.create_bucket(bucket).await;
    emulator.write_fixture(bucket, key, BODY).await;
    let policy = document(vec![json!({
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
        "Action": GET_OBJECT,
        "Resource": format!("arn:aws:s3:::{bucket}/*"),
    })]);
    emulator.put_bucket_policy(bucket, &policy).await;
    let route = emulator.create_access_point("protecting-ap", bucket);
    let endpoint_host = format!("{}.s3.{REGION}.amazonaws.com", route.endpoint_label());
    let path_style_host = format!("s3.{REGION}.amazonaws.com");

    // (case, method, the host, the path)
    let rows: [(&str, &str, &String, String); 5] = [
        (
            "DeleteBucket at the access point's own endpoint",
            "DELETE",
            &endpoint_host,
            "/".to_string(),
        ),
        (
            "PutBucketPolicy at the access point's own endpoint",
            "PUT",
            &endpoint_host,
            "/?policy".to_string(),
        ),
        (
            "DeleteBucketPolicy at the access point's own endpoint",
            "DELETE",
            &endpoint_host,
            "/?policy".to_string(),
        ),
        (
            "DeleteBucket addressed by the access point ARN",
            "DELETE",
            &path_style_host,
            format!("/{}", form_encode(&route.arn)),
        ),
        (
            "PutBucketPolicy addressed by the access point alias",
            "PUT",
            &path_style_host,
            format!("/{}?policy", route.alias),
        ),
    ];
    for (case, method, host, path) in rows {
        let answer = emulator
            .s3_at(ACCOUNT_KEY, host, method, &path, b"{}")
            .await;
        assert!(
            answer.status >= 400,
            "case {case:?}: a bucket-level call addressed to an access point must be \
             refused, got {} with {}",
            answer.status,
            body_text(&answer)
        );
        // The bucket behind it is untouched: the object is still readable and the policy it
        // was given is still the one it carries.
        let read = emulator
            .s3(ACCOUNT_KEY, "GET", &format!("/{bucket}/{key}"), b"")
            .await;
        assert_eq!(
            (read.status, body_text(&read).as_str()),
            (200, String::from_utf8_lossy(BODY).as_ref()),
            "case {case:?}: the fronted bucket must still hold its object, got {} with {}",
            read.status,
            body_text(&read)
        );
        let held = emulator
            .s3(ACCOUNT_KEY, "GET", &format!("/{bucket}?policy"), b"")
            .await;
        assert_eq!(
            (held.status, body_text(&held).as_str()),
            (200, policy.as_str()),
            "case {case:?}: the fronted bucket must still carry its own policy, got {} with \
             {}",
            held.status,
            body_text(&held)
        );
    }
}

/// One access point policy a row installs, and what it means for the two reads the row makes.
struct PolicyRow {
    case: &'static str,
    /// The document, built from the session the row calls as and the access point it is on.
    policy: fn(&Session, &AccessPointRoute) -> String,
    /// Whether the read THROUGH the access point must be refused by an explicit deny.
    denies_the_access_point_path: bool,
}

/// Rule, second half: the access point's own policy decides the requests made THROUGH the
/// access point, and only those — an explicit `Deny` there refuses that path 403
/// `AccessDenied` naming the access point's own resource, while the same object read by
/// BUCKET NAME is served.
///
/// The session's identity policy allows the read on both the bucket's objects and the access
/// point's, so every refusal below can only come from the access point's document, and the
/// direct read can only be refused if that document were applied where it does not belong.
///
/// The rows differ along what selects the principal (its ARN through `aws:PrincipalArn`, a
/// wildcard, another principal) and the resource scope the statement covers (every object,
/// one key prefix).
#[tokio::test]
async fn an_access_point_policy_deny_governs_only_that_access_points_path() {
    let emulator = Emulator::new();
    let bucket = "access-point-denied-bucket";
    let key = "data/report.txt";
    emulator.create_bucket(bucket).await;
    emulator.write_fixture(bucket, key, BODY).await;
    let route = emulator.create_access_point("denying-ap", bucket);
    let session = emulator
        .assumed_role(
            "access-point-reader",
            Some(&document(vec![json!({
                "Effect": "Allow",
                "Action": GET_OBJECT,
                "Resource": [
                    format!("arn:aws:s3:::{bucket}/*"),
                    access_point_object(&route.arn, "*"),
                ],
            })])),
        )
        .await;

    let rows = [
        PolicyRow {
            case: "a Deny selected by the calling role's ARN",
            policy: |session, route| {
                document(vec![json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": GET_OBJECT,
                    "Resource": access_point_object(&route.arn, "*"),
                    "Condition": {"ArnEquals": {"aws:PrincipalArn": session.role_arn.clone()}},
                })])
            },
            denies_the_access_point_path: true,
        },
        PolicyRow {
            case: "a Deny of every principal",
            policy: |_, route| {
                document(vec![json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3:*",
                    "Resource": access_point_object(&route.arn, "*"),
                })])
            },
            denies_the_access_point_path: true,
        },
        PolicyRow {
            case: "a Deny scoped to a key prefix the request is inside of",
            policy: |_, route| {
                document(vec![json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": GET_OBJECT,
                    "Resource": access_point_object(&route.arn, "data/*"),
                })])
            },
            denies_the_access_point_path: true,
        },
        PolicyRow {
            case: "a Deny scoped to a key prefix the request is outside of",
            policy: |_, route| {
                document(vec![json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": GET_OBJECT,
                    "Resource": access_point_object(&route.arn, "archive/*"),
                })])
            },
            denies_the_access_point_path: false,
        },
        PolicyRow {
            case: "a Deny selected by another principal's ARN",
            policy: |_, route| {
                document(vec![json!({
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": GET_OBJECT,
                    "Resource": access_point_object(&route.arn, "*"),
                    "Condition": {"ArnEquals": {
                        "aws:PrincipalArn": format!(
                            "arn:aws:iam::{}:role/another-role",
                            engine::emulator_account_id()
                        ),
                    }},
                })])
            },
            denies_the_access_point_path: false,
        },
        PolicyRow {
            case: "a policy that denies nothing at all",
            policy: |_, route| {
                document(vec![json!({
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": GET_OBJECT,
                    "Resource": access_point_object(&route.arn, "*"),
                })])
            },
            denies_the_access_point_path: false,
        },
    ];

    for row in rows {
        let case = row.case;
        emulator.put_access_point_policy(&route.arn, Some((row.policy)(&session, &route)));
        // The read THROUGH the access point, at its own endpoint — the form an SDK sends.
        let host = format!("{}.s3.{REGION}.amazonaws.com", route.endpoint_label());
        let through = emulator
            .s3_at(&session.access_key_id, &host, "GET", &format!("/{key}"), b"")
            .await;
        if row.denies_the_access_point_path {
            assert_eq!(
                (through.status, through.error_code().as_str()),
                (403, ACCESS_DENIED),
                "case {case:?}: the read through the access point must be refused 403 \
                 {ACCESS_DENIED}, got {} with {}",
                through.status,
                body_text(&through)
            );
            assert_eq!(
                through.error_message(),
                denial_message(
                    &session.session_arn,
                    GET_OBJECT,
                    &access_point_object(&route.arn, key),
                    RESOURCE_DENY
                ),
                "case {case:?}: the refusal must name the access point's own resource and the \
                 document that decided it, got {}",
                body_text(&through)
            );
        } else {
            assert_eq!(
                (through.status, body_text(&through).as_str()),
                (200, String::from_utf8_lossy(BODY).as_ref()),
                "case {case:?}: the read through the access point must be served, got {} with \
                 {}",
                through.status,
                body_text(&through)
            );
        }
        // And the same object by BUCKET NAME, which that document does not govern.
        let direct = emulator
            .s3(
                &session.access_key_id,
                "GET",
                &format!("/{bucket}/{key}"),
                b"",
            )
            .await;
        assert_eq!(
            (direct.status, body_text(&direct).as_str()),
            (200, String::from_utf8_lossy(BODY).as_ref()),
            "case {case:?}: the read by bucket name must be untouched by the access point's \
             policy, got {} with {}",
            direct.status,
            body_text(&direct)
        );
    }

    // Removing the document returns the access point to carrying none, and the read it had
    // been refusing is served again.
    emulator.put_access_point_policy(&route.arn, Some(denies_everyone(&route)));
    emulator.put_access_point_policy(&route.arn, None);
    let host = format!("{}.s3.{REGION}.amazonaws.com", route.endpoint_label());
    let served = emulator
        .s3_at(&session.access_key_id, &host, "GET", &format!("/{key}"), b"")
        .await;
    assert_eq!(
        (served.status, body_text(&served).as_str()),
        (200, String::from_utf8_lossy(BODY).as_ref()),
        "case a removed document: the read through the access point must be served again, got \
         {} with {}",
        served.status,
        body_text(&served)
    );
}

/// The document every principal is denied by, for the row that removes it again.
fn denies_everyone(route: &AccessPointRoute) -> String {
    document(vec![json!({
        "Effect": "Deny",
        "Principal": "*",
        "Action": "s3:*",
        "Resource": access_point_object(&route.arn, "*"),
    })])
}

/// Rule, third half: the access point's policy and a bucket policy that DELEGATES to its
/// access points are each a grant for the access point path on their own — and neither
/// grants the request addressed to the bucket directly.
///
/// The session holds NO identity-based document at all here, so the only grant a served read
/// can have come from is the document the row installed. The rows differ along where that
/// grant lives: the access point's own policy, and a bucket policy conditioned on the
/// `s3:DataAccessPointAccount` key — which is answered only for a request that really came
/// through an access point.
#[tokio::test]
async fn a_grant_for_the_access_point_path_does_not_grant_the_bucket_path() {
    let emulator = Emulator::new();
    let bucket = "access-point-granted-bucket";
    let key = "shared/report.txt";
    emulator.create_bucket(bucket).await;
    emulator.write_fixture(bucket, key, BODY).await;
    let route = emulator.create_access_point("granting-ap", bucket);
    let session = emulator.assumed_role("access-point-grantee", None).await;
    let host = format!("{}.s3.{REGION}.amazonaws.com", route.endpoint_label());

    // (case, the access point's policy, the bucket's policy)
    let rows: [(&str, Option<String>, Option<String>); 2] = [
        (
            "the grant is the access point's own policy",
            Some(document(vec![json!({
                "Effect": "Allow",
                "Principal": {"AWS": session.role_arn.clone()},
                "Action": GET_OBJECT,
                "Resource": access_point_object(&route.arn, "*"),
            })])),
            None,
        ),
        (
            "the grant is a bucket policy delegating to its access points",
            None,
            Some(document(vec![json!({
                "Effect": "Allow",
                "Principal": {"AWS": "*"},
                "Action": "*",
                "Resource": [
                    format!("arn:aws:s3:::{bucket}"),
                    format!("arn:aws:s3:::{bucket}/*"),
                ],
                "Condition": {"StringEquals": {
                    "s3:DataAccessPointAccount": engine::emulator_account_id(),
                }},
            })])),
        ),
    ];

    for (case, access_point_policy, bucket_policy) in rows {
        emulator.put_access_point_policy(&route.arn, access_point_policy);
        if let Some(bucket_policy) = &bucket_policy {
            emulator.put_bucket_policy(bucket, bucket_policy).await;
        }
        let through = emulator
            .s3_at(&session.access_key_id, &host, "GET", &format!("/{key}"), b"")
            .await;
        assert_eq!(
            (through.status, body_text(&through).as_str()),
            (200, String::from_utf8_lossy(BODY).as_ref()),
            "case {case:?}: the read through the access point must be granted, got {} with {}",
            through.status,
            body_text(&through)
        );
        // The same read addressed to the BUCKET has no grant at all: the access point's
        // policy is not in force on it, and the delegating statement's condition is answered
        // only for a request that came through an access point.
        let direct = emulator
            .s3(
                &session.access_key_id,
                "GET",
                &format!("/{bucket}/{key}"),
                b"",
            )
            .await;
        assert_eq!(
            (direct.status, direct.error_code().as_str()),
            (403, ACCESS_DENIED),
            "case {case:?}: the read by bucket name must be refused 403 {ACCESS_DENIED}, got \
             {} with {}",
            direct.status,
            body_text(&direct)
        );
        assert_eq!(
            direct.error_message(),
            denial_message(
                &session.session_arn,
                GET_OBJECT,
                &format!("arn:aws:s3:::{bucket}/{key}"),
                &no_grant(GET_OBJECT)
            ),
            "case {case:?}: the refusal must name the bucket's own resource and the absent \
             grant, got {}",
            body_text(&direct)
        );
    }
}
