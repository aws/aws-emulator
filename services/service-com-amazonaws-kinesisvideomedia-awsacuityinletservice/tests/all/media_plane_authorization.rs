// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a call to this media data plane through, driven end to end over the
//! wire against the services that hold the identity and its policies.
//!
//! The rule under test, one level above the recorded case (one role, one deny, one call):
//!
//! > Every route of this media data plane is authorized against the IDENTITY-BASED policies
//! > in force on the principal the request's credential names, on the action
//! > `kinesisvideo:<ApiName>` and the stream the request addresses, BEFORE anything about
//! > that stream is read or written: a call no statement allows (IAM's default deny) and
//! > one an explicit `Deny` covers are both refused `AccessDeniedException` (HTTP 403) with
//! > no media served and none ingested, while a call a statement allows is served. The
//! > account's own published credential, which holds no IAM policies, is the position AWS
//! > account root credentials are in and is served.
//!
//! The rows span what the rule generalises over: the statements a policy can carry (an
//! allow of the exact action, a wildcard allow, an allow of only one of the two actions, an
//! allow scoped to another stream, an explicit deny, an explicit deny beside a wildcard
//! allow, no policy at all) and both routes of the plane — the modelled read route and the
//! producer route beside it. One row also proves the ORDER: a caller IAM refuses is refused
//! even for a stream the registry does not hold, so the decision is taken ahead of
//! everything about the resource.
//!
//! Two namings of one stream are at work, and the rows pin both: a statement's `Resource`
//! element is written as the stream's ARN (the row scoped to this stream's ARN is served),
//! while the refusal names the stream AS THE REQUEST ADDRESSED IT (a row addressing it by
//! name is refused in a sentence carrying that name and not the ARN).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice as media;
use smithy_server_runtime::MultiServiceRouter;

use crate::put_media_ingest::{cluster, ingest_headers, Container};

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The host a data-plane call arrives on.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The emulator's own published long-term access key: the account credential, which holds
/// no IAM policies.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The code and status AWS refuses a call IAM does not permit with — the pair recorded
/// behaviour shows, which no Kinesis Video media model declares a shape for.
const DENIED_CODE: &str = "AccessDeniedException";
const DENIED_STATUS: u16 = 403;

/// The stream every row addresses, and another one a policy can be scoped to instead.
const STREAM: &str = "authz-stream";
const OTHER_STREAM: &str = "authz-other-stream";

/// A name the registry never held, for the row that proves the decision comes first.
const NEVER_CREATED: &str = "authz-never-created";

/// The tag name every chunk `GetMedia` serves carries in its Matroska `Tags` block, so a
/// body holding none of it served no chunk.
const MKV_TAG_MARKER: &[u8] = b"AWS_KINESISVIDEO_";

/// The router every case drives: the control plane that creates a stream, this media data
/// plane, and the two services that own the identity a request speaks for (STS) and the
/// policies in force on it (IAM) — all on ONE engine.
///
/// Its own harness rather than the one `control_plane_stream_reads` shares: that one mounts
/// two services and signs every call with the account's credential, while every call here is
/// signed with the credential of the ROW's own principal and two more services have to be
/// mounted for those credentials to exist at all.
struct Harness {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(media::EmulatedService::new(Arc::clone(&engine)));
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
        Self { engine, router }
    }

    /// The SigV4 credential scope a request signed with `access_key_id` carries. Its
    /// service field is the routing key; this router verifies no signature, so the
    /// credential is read for its key id alone — which is what the gate resolves an
    /// identity from.
    fn authorization(access_key_id: &str, signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20261031/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// POST one restJson1 call to a Kinesis Video plane, signed with `access_key_id`.
    async fn call(&self, access_key_id: &str, path: &str, body: &Value) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                Self::authorization(access_key_id, "kinesisvideo"),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.router.dispatch(request).await
    }

    /// `GetMedia` from the earliest chunk of the stream `addressing` names, signed with
    /// `access_key_id`.
    async fn get_media(&self, access_key_id: &str, addressing: &Value) -> http::Response<Bytes> {
        let mut body = addressing.as_object().cloned().unwrap_or_default();
        body.insert(
            "StartSelector".to_string(),
            json!({ "StartSelectorType": "EARLIEST" }),
        );
        self.call(access_key_id, "/getMedia", &Value::Object(body))
            .await
    }

    /// POST one ingest session for `stream_name`, signed with `access_key_id`: one Cluster
    /// of media, as a producer submits it.
    async fn put_media(
        &self,
        access_key_id: &str,
        stream_name: &str,
        producer_start: SystemTime,
    ) -> http::Response<Bytes> {
        let headers = ingest_headers(
            ("x-amzn-stream-name", stream_name.to_string()),
            "RELATIVE",
            producer_start,
        );
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT_HOST}/putMedia"))
            .header("host", ENDPOINT_HOST)
            .header(
                "authorization",
                Self::authorization(access_key_id, "kinesisvideo"),
            );
        for (name, value) in &headers {
            request = request.header(*name, value.clone());
        }
        let body = Container::of(vec![cluster(0, 2, 33, 0xD4)]).bytes();
        self.router
            .dispatch(
                request
                    .body(Bytes::from(body))
                    .expect("a well-formed request"),
            )
            .await
    }

    /// One awsQuery call to IAM or STS, always with the account's own credential:
    /// `signing_name` picks the service, `version` its API version.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encoded(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                Self::authorization(ACCOUNT_KEY, signing_name),
            )
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the access key id of the session — the credential
    /// a caller then signs its media requests with.
    async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let account = engine::EMULATOR_ACCOUNT_ID;
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{account}:root") },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
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
            assert_eq!(
                status, 200,
                "setup: putting the inline policy of {role_name:?} must succeed, got \
                 {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!("arn:aws:iam::{account}:role/{role_name}"),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("setup: AssumeRole must answer an AccessKeyId, got {body}"))
    }

    /// Create `name` through the control plane and answer the `StreamARN` it minted.
    async fn create_stream(&self, name: &str) -> String {
        let response = self
            .call(
                ACCOUNT_KEY,
                "/createStream",
                &json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "setup: creating {name:?} must succeed, got {} ({})",
            status(&response),
            error_code(&response)
        );
        body(&response)
            .get("StreamARN")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("setup: no StreamARN for {name:?}"))
            .to_string()
    }

    /// How many fragments this service's media log holds for the stream `arn` names.
    fn stored_fragments(&self, arn: &str) -> usize {
        let state = media::state::service_state(&*self.engine).expect("the media store");
        let store = state.lock();
        store.log_by_arn(arn).map_or(0, |log| log.fragments.len())
    }
}

/// One form field, percent-encoded the way an awsQuery body carries it.
fn form_encoded(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>` element of an awsQuery XML response.
fn xml_element(document: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = document.find(&open)? + open.len();
    let end = document[start..].find(&close)? + start;
    Some(document[start..end].to_string())
}

/// An identity-based policy with one statement.
fn policy(effect: &str, actions: &[&str], resources: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Action": actions,
            "Resource": resources,
        }],
    })
    .to_string()
}

fn status(response: &http::Response<Bytes>) -> u16 {
    response.status().as_u16()
}

fn error_code(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn body(response: &http::Response<Bytes>) -> Value {
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

fn content_type(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of an error answer.
fn message(response: &http::Response<Bytes>) -> String {
    let parsed = body(response);
    parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row: the policy a principal holds, and what each route owes it.
struct Row {
    case: &'static str,
    role: &'static str,
    /// The inline policy the role carries; `None` for a principal the account grants
    /// nothing.
    policy: Option<String>,
    /// The status the modelled read route owes this caller.
    read_status: u16,
    /// The status the producer route owes it.
    ingest_status: u16,
}

/// RULE: both routes of this media plane are served only to a caller IAM permits, and a
/// caller it refuses is answered `AccessDeniedException` with nothing served and nothing
/// ingested.
#[tokio::test]
async fn a_media_call_is_served_only_to_a_caller_iam_permits() {
    let harness = Harness::new();
    // Both streams exist, and both hold media: what a row varies is the POLICY, never
    // whether the resource is there, so a refusal is never a disguised not-found.
    let arn = harness.create_stream(STREAM).await;
    let other_arn = harness.create_stream(OTHER_STREAM).await;
    for name in [STREAM, OTHER_STREAM] {
        let response = harness
            .put_media(ACCOUNT_KEY, name, producer_start(0))
            .await;
        assert_eq!(
            status(&response),
            200,
            "setup: seeding {name:?} must succeed, got {} ({})",
            status(&response),
            error_code(&response)
        );
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "a policy allowing both actions on this stream",
            role: "both-actions",
            policy: Some(policy(
                "Allow",
                &["kinesisvideo:GetMedia", "kinesisvideo:PutMedia"],
                &[&arn],
            )),
            read_status: 200,
            ingest_status: 200,
        },
        Row {
            case: "a wildcard allow of every action on every resource",
            role: "wildcard",
            policy: Some(policy("Allow", &["kinesisvideo:*"], &["*"])),
            read_status: 200,
            ingest_status: 200,
        },
        Row {
            case: "a policy allowing only the read action",
            role: "read-only",
            policy: Some(policy("Allow", &["kinesisvideo:GetMedia"], &["*"])),
            read_status: 200,
            ingest_status: DENIED_STATUS,
        },
        Row {
            case: "a policy allowing only the producer action",
            role: "write-only",
            policy: Some(policy("Allow", &["kinesisvideo:PutMedia"], &["*"])),
            read_status: DENIED_STATUS,
            ingest_status: 200,
        },
        Row {
            case: "an allow scoped to another stream",
            role: "other-stream",
            policy: Some(policy("Allow", &["kinesisvideo:*"], &[&other_arn])),
            read_status: DENIED_STATUS,
            ingest_status: DENIED_STATUS,
        },
        Row {
            case: "an explicit deny beside no allow",
            role: "explicit-deny",
            policy: Some(policy("Deny", &["kinesisvideo:*"], &["*"])),
            read_status: DENIED_STATUS,
            ingest_status: DENIED_STATUS,
        },
        Row {
            case: "an explicit deny of the read beside an allow of every action",
            role: "deny-wins",
            policy: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        { "Effect": "Allow", "Action": "kinesisvideo:*", "Resource": "*" },
                        {
                            "Effect": "Deny",
                            "Action": "kinesisvideo:GetMedia",
                            "Resource": arn,
                        },
                    ],
                })
                .to_string(),
            ),
            read_status: DENIED_STATUS,
            ingest_status: 200,
        },
        Row {
            case: "no policy at all, which is IAM's default deny",
            role: "no-policy",
            policy: None,
            read_status: DENIED_STATUS,
            ingest_status: DENIED_STATUS,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let key = harness
            .assumed_role_key(row.role, row.policy.as_deref())
            .await;

        // The modelled read route.
        let response = harness
            .get_media(&key, &json!({ "StreamName": STREAM }))
            .await;
        assert_eq!(
            status(&response),
            row.read_status,
            "case {case:?}: GetMedia expected HTTP {}, got {} ({}) {:?}",
            row.read_status,
            status(&response),
            error_code(&response),
            body(&response)
        );
        if row.read_status == DENIED_STATUS {
            assert_eq!(
                error_code(&response),
                DENIED_CODE,
                "case {case:?}: expected {DENIED_CODE}, got {:?}",
                error_code(&response)
            );
            // The sentence a Kinesis Video media API words this refusal in: the principal,
            // the action, and the stream AS THE REQUEST ADDRESSED IT — one sentence for
            // every rule the decision can rest on, so an explicit deny and IAM's default
            // deny are worded alike and neither names a policy.
            let answered = message(&response);
            let expected_tail =
                format!(" is not authorized to perform: kinesisvideo:GetMedia on resource: \
                          {STREAM}");
            assert!(
                answered.starts_with("User: arn:") && answered.ends_with(&expected_tail),
                "case {case:?}: expected a refusal naming a principal and ending with \
                 {expected_tail:?}, got {answered:?}"
            );
            assert!(
                !answered.contains(&arn),
                "case {case:?}: the refusal names the stream as the request addressed it, so \
                 it must not carry the ARN {arn:?}, got {answered:?}"
            );
            // Nothing was served to a caller that was refused: no chunk framing — the MKV
            // tag block every served chunk carries — and not the stream's media content
            // type.
            assert!(
                !response
                    .body()
                    .windows(MKV_TAG_MARKER.len())
                    .any(|window| window == MKV_TAG_MARKER),
                "case {case:?}: a refused read must serve no chunk, but its body carries the \
                 chunk framing"
            );
            assert_ne!(
                content_type(&response),
                media::state::DEFAULT_CONTENT_TYPE,
                "case {case:?}: a refused read must not answer as the stream's media"
            );
        } else {
            assert!(
                !response.body().is_empty(),
                "case {case:?}: a permitted read must answer the stream's media, got an empty \
                 payload"
            );
        }

        // The producer route beside it, judged on its own action.
        let before = harness.stored_fragments(&arn);
        let response = harness
            .put_media(&key, STREAM, producer_start(index + 1))
            .await;
        assert_eq!(
            status(&response),
            row.ingest_status,
            "case {case:?}: PutMedia expected HTTP {}, got {} ({})",
            row.ingest_status,
            status(&response),
            error_code(&response)
        );
        let after = harness.stored_fragments(&arn);
        if row.ingest_status == DENIED_STATUS {
            assert_eq!(
                error_code(&response),
                DENIED_CODE,
                "case {case:?}: expected {DENIED_CODE}, got {:?}",
                error_code(&response)
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused ingest must persist nothing, but the log went from \
                 {before} to {after} fragment(s)"
            );
            let answered = message(&response);
            assert!(
                answered.ends_with(&format!(
                    " is not authorized to perform: kinesisvideo:PutMedia on resource: {STREAM}"
                )),
                "case {case:?}: expected the refusal to name the producer action and the \
                 stream, got {answered:?}"
            );
        } else {
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: a permitted ingest must persist its Cluster, but the log went \
                 from {before} to {after} fragment(s)"
            );
        }
    }

    // The account's own published credential holds no IAM policies — the position AWS
    // account root credentials are in — and is served on both routes.
    let read = harness
        .get_media(ACCOUNT_KEY, &json!({ "StreamName": STREAM }))
        .await;
    assert_eq!(
        status(&read),
        200,
        "case \"the account's own credential\": GetMedia expected HTTP 200, got {} ({})",
        status(&read),
        error_code(&read)
    );
    let ingested = harness
        .put_media(ACCOUNT_KEY, STREAM, producer_start(rows.len() + 1))
        .await;
    assert_eq!(
        status(&ingested),
        200,
        "case \"the account's own credential\": PutMedia expected HTTP 200, got {} ({})",
        status(&ingested),
        error_code(&ingested)
    );
}

/// RULE: the decision is taken BEFORE anything about the stream is resolved, so a caller
/// IAM refuses is refused whether or not the stream it addressed exists — it learns nothing
/// about the resource.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_before_the_stream_is_resolved() {
    let harness = Harness::new();
    let arn = harness.create_stream(STREAM).await;
    let never_created_arn = arn.replace(STREAM, NEVER_CREATED);

    // One principal the account grants nothing, and one it explicitly denies: both of IAM's
    // refusals must come ahead of the registry's answer.
    let default_denied = harness.assumed_role_key("ordering-no-policy", None).await;
    let explicitly_denied = harness
        .assumed_role_key(
            "ordering-explicit-deny",
            Some(&policy("Deny", &["kinesisvideo:*"], &["*"])),
        )
        .await;

    // (case, the credential, the addressing, the stream as the request addresses it)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "no policy, a stream that exists",
            &default_denied,
            json!({ "StreamName": STREAM }),
            STREAM.to_string(),
        ),
        (
            "no policy, a name nothing created",
            &default_denied,
            json!({ "StreamName": NEVER_CREATED }),
            NEVER_CREATED.to_string(),
        ),
        (
            "no policy, an ARN the registry never held",
            &default_denied,
            json!({ "StreamARN": never_created_arn.clone() }),
            never_created_arn.clone(),
        ),
        (
            "an explicit deny, a stream that exists",
            &explicitly_denied,
            json!({ "StreamName": STREAM }),
            STREAM.to_string(),
        ),
        (
            "an explicit deny, the ARN of a stream that exists",
            &explicitly_denied,
            json!({ "StreamARN": arn.clone() }),
            arn.clone(),
        ),
        (
            "an explicit deny, a name nothing created",
            &explicitly_denied,
            json!({ "StreamName": NEVER_CREATED }),
            NEVER_CREATED.to_string(),
        ),
    ];

    for (case, key, addressing, addressed_as) in &rows {
        let response = harness.get_media(key, addressing).await;
        assert_eq!(
            status(&response),
            DENIED_STATUS,
            "case {case:?}: expected HTTP {DENIED_STATUS}, got {} ({})",
            status(&response),
            error_code(&response)
        );
        assert_eq!(
            error_code(&response),
            DENIED_CODE,
            "case {case:?}: expected {DENIED_CODE}, got {:?}",
            error_code(&response)
        );
        // The refusal names the stream as the request addressed it, whichever identifier
        // form that was, and says nothing about whether it exists.
        let answered = message(&response);
        let expected_tail = format!(
            " is not authorized to perform: kinesisvideo:GetMedia on resource: {addressed_as}"
        );
        assert!(
            answered.ends_with(&expected_tail),
            "case {case:?}: expected the refusal to end with {expected_tail:?}, got {answered:?}"
        );
    }

    // And the account's own credential still tells the two apart, so the rows above are
    // about the caller and not about this suite's addressing.
    let found = harness
        .get_media(ACCOUNT_KEY, &json!({ "StreamName": STREAM }))
        .await;
    assert_eq!(
        status(&found),
        200,
        "case \"the account reading the stream that exists\": expected HTTP 200, got {} ({})",
        status(&found),
        error_code(&found)
    );
    let absent = harness
        .get_media(ACCOUNT_KEY, &json!({ "StreamName": NEVER_CREATED }))
        .await;
    assert_eq!(
        status(&absent),
        404,
        "case \"the account reading a name nothing created\": expected HTTP 404, got {} ({})",
        status(&absent),
        error_code(&absent)
    );
}

/// The producer start timestamp of the `index`th ingest session: whole seconds apart, so
/// each session's fragment lands on its own producer instant.
fn producer_start(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_000_000 + (index as u64) * 10_000)
}
