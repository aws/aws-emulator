// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartContentUpload` and the upload plane the URL it hands out
//! is served by.
//!
//! Every assertion goes over the wire: a request is built and dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts. The store is read directly in
//! exactly one place — the bytes a `PUT` to an upload URL persisted — because this model
//! binds no operation that reads an upload back: `CreateContent` consumes the
//! `uploadId` and reports the CONTENT it finalized, not the upload.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a start against a `CUSTOM` knowledge base this account holds PERSISTS an upload and
//!   reports it: an `uploadId` no other upload shares, a `url` that names that upload
//!   under that knowledge base at the endpoint the caller reached, a `urlExpiry` that is
//!   the call's instant plus the time to live the caller asked for (this emulator's
//!   default when it asked for none), and `headersToInclude` carrying the media type the
//!   upload was started for plus — only for a knowledge base encrypted under a
//!   customer-managed key — the encryption headers that key requires;
//! * the URL is FOLLOWABLE: an unsigned `PUT` carrying those headers stores the asset in
//!   the upload the URL names, and the plane refuses one that is late, that omits or
//!   contradicts a published header, that uses another method, that names another
//!   knowledge base's path, or that carries more bytes than this endpoint accepts;
//! * a start against a knowledge base this account does not hold is
//!   `ResourceNotFoundException` (HTTP 404), and one against a knowledge base whose type
//!   is not `CUSTOM` is `ValidationException` (HTTP 400) — the operation's own
//!   documentation: "You can only upload content to a knowledge base of type CUSTOM." —
//!   stating the type the request pointed at by the name this service uses for it
//!   internally, read from the STORED knowledge base, for every modelled type that is not
//!   `CUSTOM` and in both label forms;
//!
//! This view of the shared `wisdom` endpoint answers under its own registration key (see
//! `src/service.rs`), which is what every modelled request below is signed for; the
//! requests that FOLLOW an upload URL are signed for nothing at all, as a presigned URL
//! is followed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::state::{
    self, DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES, MAX_CONTENT_UPLOAD_BYTES,
    MAX_PENDING_UPLOAD_BYTES,
};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every modelled request in this suite is signed for.
const REGION: &str = "eu-west-3";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`).
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// The endpoint host the modelled requests address.
const ENDPOINT_HOST: &str = "com-amazonaws-wisdom-wisdomservice.eu-west-3.amazonaws.com";

/// The base URL this emulator is reached on while the router is not served on a port
/// (`smithy_server_runtime::endpoint::UNSERVED_URL_BASE`), which is the base the URLs this
/// suite follows are presigned against.
const EMULATOR_BASE_URL: &str = "https://emulator.invalid";

/// The SigV4 signing name this endpoint is addressed under, which a presigned URL it hands
/// out is scoped to.
const SIGNING_NAME: &str = "wisdom";

/// Every value the modelled `ContentType` pattern admits
/// (`smithy_query(query="operation",operation="StartContentUpload")`:
/// `^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$`),
/// so the table spans the whole catalog rather than the one type a recording used.
const CONTENT_TYPES: [&str; 8] = [
    "text/plain",
    "text/html",
    "text/csv",
    "application/pdf",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/x.wisdom-json;source=salesforce",
    "application/x.wisdom-json;source=servicenow",
    "application/x.wisdom-json;source=zendesk",
];

/// The published bounds of `presignedUrlTimeToLive`, in minutes
/// (`smithy_query(query="operation",operation="StartContentUpload")`: `range 1..60`).
const TIME_TO_LIVE_MIN: i64 = 1;
const TIME_TO_LIVE_MAX: i64 = 60;

/// The published bounds of `UploadId` (`len 1..1200`).
const UPLOAD_ID_MIN_LEN: usize = 1;
const UPLOAD_ID_MAX_LEN: usize = 1200;

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The header names the upload plane publishes, written out here rather than imported,
/// so the assertions pin the wire names.
const CONTENT_TYPE_HEADER: &str = "content-type";
const SSE_HEADER: &str = "x-amz-server-side-encryption";
const SSE_KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn string(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// A modelled request, signed for this service's registration key.
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", ENDPOINT_HOST)
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };
        self.dispatch(request).await
    }

    /// Follow a URL this service handed out exactly as a client does: `method` against
    /// the absolute URL, with NO credential of any kind.
    async fn follow(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &[u8],
    ) -> Reply {
        let mut builder = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("build request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> Reply {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Create a knowledge base and answer its `knowledgeBaseId` and ARN.
    async fn given_knowledge_base(
        &self,
        case: &str,
        name: &str,
        knowledge_base_type: &str,
        kms_key_id: Option<&str>,
    ) -> (String, String) {
        let mut body = json!({ "name": name, "knowledgeBaseType": knowledge_base_type });
        if let Some(kms_key_id) = kms_key_id {
            body["serverSideEncryptionConfiguration"] = json!({ "kmsKeyId": kms_key_id });
        }
        let reply = self.send("POST", "/knowledgeBases", Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the knowledge base must be created, got {reply:?}"
        );
        let id = reply
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseId in {reply:?}"));
        let arn = reply
            .string("/knowledgeBase/knowledgeBaseArn")
            .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseArn in {reply:?}"));
        (id, arn)
    }

    /// Start one upload against the knowledge base `label` names.
    async fn start_upload(
        &self,
        label: &str,
        content_type: &str,
        time_to_live: Option<i64>,
    ) -> Reply {
        // An ARN label carries `/`, which must not be read as path structure.
        let encoded = label.replace('/', "%2F");
        let mut body = json!({ "contentType": content_type });
        if let Some(minutes) = time_to_live {
            body["presignedUrlTimeToLive"] = json!(minutes);
        }
        self.send(
            "POST",
            &format!("/knowledgeBases/{encoded}/upload"),
            Some(body),
        )
        .await
    }

    /// The bytes the store holds for `upload_id`, or `None` when the upload holds none;
    /// `None` too when the store holds no such upload.
    fn stored_body(&self, upload_id: &str) -> Option<Vec<u8>> {
        let store = state::service_state(&*self.engine).expect("the service state");
        let store = store.lock();
        store.content_upload(upload_id)?.body.clone()
    }

    fn holds_upload(&self, upload_id: &str) -> bool {
        let store = state::service_state(&*self.engine).expect("the service state");
        let store = store.lock();
        store.content_upload(upload_id).is_some()
    }

    /// Backdate one upload's expiry, so the "the URL is no longer usable" branch is
    /// reachable without waiting out a minute of wall clock.
    fn expire_upload(&self, upload_id: &str) {
        let store = state::service_state(&*self.engine).expect("the service state");
        let mut store = store.lock();
        let upload = store
            .content_upload_mut(upload_id)
            .expect("the upload just started");
        upload.url_expiry = SystemTime::now() - Duration::from_secs(1);
    }
}

/// The `headersToInclude` map of a reply, as a sorted map.
fn headers_of(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    let map = reply
        .body
        .pointer("/headersToInclude")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no headersToInclude in {reply:?}"));
    map.iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One row of the start table: what varies between starts.
#[derive(Debug, Clone)]
struct StartCase {
    label: &'static str,
    content_type: &'static str,
    /// `None` exercises the absent member, so the emulator's own default is used.
    time_to_live: Option<i64>,
    /// `Some` makes the knowledge base encrypted under a customer-managed key.
    kms_key_id: Option<&'static str>,
    /// Whether the start addresses the knowledge base by ARN rather than by id: the
    /// model's `UuidOrArn` admits both forms and they must name the same record.
    by_arn: bool,
    /// The asset the `PUT` to the URL carries.
    asset: Vec<u8>,
}

/// The table: one row per axis the rules generalise over — every modelled
/// `contentType`, the time to live (absent, both published bounds, and values between),
/// whether the knowledge base is encrypted, the label form the start addresses it by,
/// and the size and shape of the asset uploaded.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            label: "plain text, no time to live named, unencrypted, by id",
            content_type: CONTENT_TYPES[0],
            time_to_live: None,
            kms_key_id: None,
            by_arn: false,
            asset: b"the body of an article".to_vec(),
        },
        StartCase {
            label: "html at the shortest legal time to live, by ARN",
            content_type: CONTENT_TYPES[1],
            time_to_live: Some(TIME_TO_LIVE_MIN),
            kms_key_id: None,
            by_arn: true,
            asset: b"<p>an article</p>".to_vec(),
        },
        StartCase {
            label: "csv at the longest legal time to live, encrypted, by id",
            content_type: CONTENT_TYPES[2],
            time_to_live: Some(TIME_TO_LIVE_MAX),
            kms_key_id: Some("arn:aws:kms:eu-west-3:111122223333:key/abc"),
            by_arn: false,
            asset: b"a,b,c\n1,2,3\n".to_vec(),
        },
        StartCase {
            label: "pdf, encrypted, by ARN, binary asset",
            content_type: CONTENT_TYPES[3],
            time_to_live: Some(30),
            kms_key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab"),
            by_arn: true,
            asset: vec![0u8, 1, 2, 250, 251, 255],
        },
        StartCase {
            label: "a word document, no time to live named",
            content_type: CONTENT_TYPES[4],
            time_to_live: None,
            kms_key_id: None,
            by_arn: false,
            asset: b"PK\x03\x04 and the rest".to_vec(),
        },
        StartCase {
            label: "a salesforce wisdom-json asset",
            content_type: CONTENT_TYPES[5],
            time_to_live: Some(5),
            kms_key_id: None,
            by_arn: false,
            asset: br#"{"source":"salesforce"}"#.to_vec(),
        },
        StartCase {
            label: "a servicenow wisdom-json asset, encrypted",
            content_type: CONTENT_TYPES[6],
            time_to_live: Some(TIME_TO_LIVE_MAX - 1),
            kms_key_id: Some("alias/wisdom"),
            by_arn: false,
            asset: br#"{"source":"servicenow"}"#.to_vec(),
        },
        StartCase {
            label: "a zendesk wisdom-json asset, by ARN, empty body",
            content_type: CONTENT_TYPES[7],
            time_to_live: None,
            kms_key_id: None,
            by_arn: true,
            asset: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_start_persists_an_upload_and_its_url_can_be_followed() {
    let emulator = Emulator::new();
    let mut handed_out: BTreeMap<String, String> = BTreeMap::new();

    for (index, case) in start_cases().into_iter().enumerate() {
        let label = case.label;
        let (knowledge_base_id, knowledge_base_arn) = emulator
            .given_knowledge_base(
                label,
                &format!("kb-{index}"),
                "CUSTOM",
                case.kms_key_id,
            )
            .await;
        let addressed = if case.by_arn {
            knowledge_base_arn.as_str()
        } else {
            knowledge_base_id.as_str()
        };

        let before = SystemTime::now();
        let reply = emulator
            .start_upload(addressed, case.content_type, case.time_to_live)
            .await;
        let after = SystemTime::now();
        assert_eq!(
            reply.status, 200,
            "case {label:?}: a start against a CUSTOM knowledge base must succeed, \
             got {reply:?}"
        );

        let upload_id = reply
            .string("/uploadId")
            .unwrap_or_else(|| panic!("case {label:?}: no uploadId in {reply:?}"));
        assert!(
            (UPLOAD_ID_MIN_LEN..=UPLOAD_ID_MAX_LEN).contains(&upload_id.len()),
            "case {label:?}: uploadId must satisfy the modelled len 1..1200, got {} \
             characters",
            upload_id.len()
        );
        assert!(
            handed_out
                .insert(upload_id.clone(), label.to_string())
                .is_none(),
            "case {label:?}: uploadId {upload_id:?} was handed out twice"
        );

        // The URL names THIS upload under THIS knowledge base, on the base URL this
        // emulator is reached on, and it is PRESIGNED: the credential a write is held to
        // travels in the URL's own query (SigV4's `X-Amz-*` fields), scoped to the service
        // this endpoint is signed as and good for exactly the window the start published.
        let url = reply
            .string("/url")
            .unwrap_or_else(|| panic!("case {label:?}: no url in {reply:?}"));
        let (address, query) = url
            .split_once('?')
            .unwrap_or_else(|| panic!("case {label:?}: the url carries no signature: {url:?}"));
        assert_eq!(
            address,
            format!("{EMULATOR_BASE_URL}/knowledgeBases/{knowledge_base_id}/uploads/{upload_id}"),
            "case {label:?}: the url must name the upload under its knowledge base"
        );
        let fields: BTreeMap<&str, &str> = query
            .split('&')
            .filter_map(|field| field.split_once('='))
            .collect();
        let seconds = case
            .time_to_live
            .unwrap_or(DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES as i64)
            * 60;
        for (field, expected) in [
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
            ("X-Amz-Expires", seconds.to_string()),
        ] {
            assert_eq!(
                fields.get(field).copied(),
                Some(expected.as_str()),
                "case {label:?}: the presigned url must carry {field}={expected}, got {url:?}"
            );
        }
        assert!(
            fields
                .get("X-Amz-Credential")
                .is_some_and(|credential| credential.contains(&format!("%2F{SIGNING_NAME}%2F"))),
            "case {label:?}: the presigned url must be scoped to {SIGNING_NAME}, got {url:?}"
        );
        assert!(
            fields
                .get("X-Amz-Signature")
                .is_some_and(|signature| !signature.is_empty()),
            "case {label:?}: the presigned url must carry a signature, got {url:?}"
        );

        // `urlExpiry` is the call's instant plus the time to live asked for, and this
        // emulator's documented default for a request that named none.
        let minutes = case
            .time_to_live
            .unwrap_or(DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES as i64);
        let expiry = reply
            .body
            .pointer("/urlExpiry")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: no urlExpiry in {reply:?}"));
        let epoch = |at: SystemTime| {
            at.duration_since(SystemTime::UNIX_EPOCH)
                .expect("after the epoch")
                .as_secs() as f64
        };
        let (lowest, highest) = (
            epoch(before) + (minutes * 60) as f64 - 1.0,
            epoch(after) + (minutes * 60) as f64 + 1.0,
        );
        assert!(
            (lowest..=highest).contains(&expiry),
            "case {label:?}: urlExpiry must be the call's instant plus {minutes} \
             minutes, i.e. within {lowest}..={highest}, got {expiry}"
        );

        // `headersToInclude` carries the media type the upload was started for, and the
        // encryption headers only for a knowledge base that names a KMS key.
        let mut expected_headers: BTreeMap<String, String> = BTreeMap::new();
        expected_headers.insert(
            CONTENT_TYPE_HEADER.to_string(),
            case.content_type.to_string(),
        );
        if let Some(kms_key_id) = case.kms_key_id {
            expected_headers.insert(SSE_HEADER.to_string(), "aws:kms".to_string());
            expected_headers.insert(SSE_KEY_ID_HEADER.to_string(), kms_key_id.to_string());
        }
        let headers = headers_of(&reply, label);
        assert_eq!(
            headers, expected_headers,
            "case {label:?}: headersToInclude must be the headers the upload requires"
        );
        assert_eq!(
            emulator.stored_body(&upload_id),
            None,
            "case {label:?}: a start must not persist an asset of its own"
        );

        // The URL is a promise that it can be FOLLOWED: an unsigned PUT carrying the
        // published headers stores the asset in the upload the URL names.
        let put = emulator.follow("PUT", &url, &headers, &case.asset).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: following the upload URL with the published headers must \
             be accepted, got {put:?}"
        );
        assert_eq!(
            emulator.stored_body(&upload_id).as_deref(),
            Some(case.asset.as_slice()),
            "case {label:?}: the upload must hold exactly the bytes that were PUT"
        );

        // A second start on the same knowledge base is a second upload: its own id and
        // its own URL, and it does not disturb the first.
        let second = emulator
            .start_upload(addressed, case.content_type, case.time_to_live)
            .await;
        let second_id = second
            .string("/uploadId")
            .unwrap_or_else(|| panic!("case {label:?}: no uploadId in {second:?}"));
        assert!(
            second_id != upload_id,
            "case {label:?}: a second start must mint another uploadId, got \
             {second_id:?} twice"
        );
        assert!(
            handed_out
                .insert(second_id.clone(), label.to_string())
                .is_none(),
            "case {label:?}: uploadId {second_id:?} was handed out twice"
        );
        assert_eq!(
            emulator.stored_body(&upload_id).as_deref(),
            Some(case.asset.as_slice()),
            "case {label:?}: a second start must leave the first upload's asset alone"
        );
    }
}

#[tokio::test]
async fn the_upload_plane_refuses_a_write_the_url_does_not_authorise() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator
        .given_knowledge_base("plane refusals", "kb-plane", "CUSTOM", None)
        .await;
    let (other_knowledge_base_id, _) = emulator
        .given_knowledge_base("plane refusals", "kb-other", "CUSTOM", None)
        .await;

    // (case, the method used, how the published headers are changed, the body sent, the
    // expected status, the expected error code)
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        &str,
        &str,
        fn(&mut BTreeMap<String, String>),
        Vec<u8>,
        u16,
        &str,
    )> = vec![
        (
            "a GET of a URL that is written by PUT",
            "GET",
            |_| {},
            Vec::new(),
            405,
            "MethodNotAllowed",
        ),
        (
            "a POST of a URL that is written by PUT",
            "POST",
            |_| {},
            b"an asset".to_vec(),
            405,
            "MethodNotAllowed",
        ),
        (
            "a PUT that omits a published header",
            "PUT",
            |headers| {
                headers.remove(CONTENT_TYPE_HEADER);
            },
            b"an asset".to_vec(),
            403,
            "AccessDenied",
        ),
        (
            "a PUT that contradicts a published header",
            "PUT",
            |headers| {
                headers.insert(CONTENT_TYPE_HEADER.to_string(), "text/csv".to_string());
            },
            b"an asset".to_vec(),
            403,
            "AccessDenied",
        ),
    ];

    for (case, method, amend, body, expected_status, expected_code) in rows {
        let reply = emulator
            .start_upload(&knowledge_base_id, "text/plain", None)
            .await;
        let upload_id = reply
            .string("/uploadId")
            .unwrap_or_else(|| panic!("case {case:?}: no uploadId in {reply:?}"));
        let url = reply
            .string("/url")
            .unwrap_or_else(|| panic!("case {case:?}: no url in {reply:?}"));
        let mut headers = headers_of(&reply, case);
        amend(&mut headers);

        let answer = emulator.follow(method, &url, &headers, &body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, expected_code,
            "case {case:?}: expected the code {expected_code}, got {answer:?}"
        );
        assert_eq!(
            emulator.stored_body(&upload_id),
            None,
            "case {case:?}: a refused write must store nothing"
        );
    }

    // A path that crosses one knowledge base with another's upload names nothing.
    let reply = emulator
        .start_upload(&knowledge_base_id, "text/plain", None)
        .await;
    let upload_id = reply.string("/uploadId").expect("an uploadId");
    let headers = headers_of(&reply, "crossed knowledge base");
    let crossed = format!(
        "https://{ENDPOINT_HOST}/knowledgeBases/{other_knowledge_base_id}/uploads/{upload_id}"
    );
    let answer = emulator
        .follow("PUT", &crossed, &headers, b"an asset")
        .await;
    assert_eq!(
        answer.status, 404,
        "case \"crossed knowledge base\": a path naming another knowledge base must \
         find no upload, got {answer:?}"
    );
    assert_eq!(
        emulator.stored_body(&upload_id),
        None,
        "case \"crossed knowledge base\": a refused write must store nothing"
    );

    // A write after the URL stopped being usable is refused, and the slot is reclaimed.
    let reply = emulator
        .start_upload(&knowledge_base_id, "text/plain", Some(TIME_TO_LIVE_MIN))
        .await;
    let upload_id = reply.string("/uploadId").expect("an uploadId");
    let url = reply.string("/url").expect("a url");
    let headers = headers_of(&reply, "expired url");
    emulator.expire_upload(&upload_id);
    let answer = emulator.follow("PUT", &url, &headers, b"an asset").await;
    assert_eq!(
        answer.status, 403,
        "case \"expired url\": a write after urlExpiry must be refused, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, "AccessDenied",
        "case \"expired url\": expected the code AccessDenied, got {answer:?}"
    );
    assert!(
        !emulator.holds_upload(&upload_id),
        "case \"expired url\": an expired upload must be reclaimed"
    );

    // A body larger than this endpoint accepts is refused rather than stored.
    let reply = emulator
        .start_upload(&knowledge_base_id, "text/plain", None)
        .await;
    let upload_id = reply.string("/uploadId").expect("an uploadId");
    let url = reply.string("/url").expect("a url");
    let headers = headers_of(&reply, "oversized asset");
    let oversized = vec![b'x'; MAX_CONTENT_UPLOAD_BYTES + 1];
    let answer = emulator.follow("PUT", &url, &headers, &oversized).await;
    assert_eq!(
        answer.status, 413,
        "case \"oversized asset\": a body past the endpoint's ceiling must be refused, \
         got {answer:?}"
    );
    assert_eq!(
        emulator.stored_body(&upload_id),
        None,
        "case \"oversized asset\": a refused write must store nothing"
    );
    // The ceiling itself is accepted, so the refusal is a bound and not a ban.
    let at_ceiling = vec![b'x'; MAX_CONTENT_UPLOAD_BYTES];
    let answer = emulator.follow("PUT", &url, &headers, &at_ceiling).await;
    assert_eq!(
        answer.status, 200,
        "case \"asset at the ceiling\": a body at the endpoint's ceiling must be \
         accepted, got {answer:?}"
    );
    assert_eq!(
        emulator.stored_body(&upload_id).map(|body| body.len()),
        Some(MAX_CONTENT_UPLOAD_BYTES),
        "case \"asset at the ceiling\": the whole body must be stored"
    );
}

/// RULE: the written upload bodies this endpoint holds are bounded in AGGREGATE as well
/// as one by one, so what a caller can hold in this process's heap is the ceiling and not
/// the PRODUCT of the per-body bound and the number of upload slots: writes are accepted
/// up to that ceiling, the first byte past it is refused and stores nothing (leaving every
/// body already held as it was), and re-writing an upload the store already holds is
/// accepted even at the ceiling, because that upload's own bytes are replaced rather than
/// added to.
#[tokio::test]
async fn written_upload_bodies_are_bounded_in_aggregate() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator
        .given_knowledge_base("aggregate", "kb-aggregate", "CUSTOM", None)
        .await;
    let full = vec![b'x'; MAX_CONTENT_UPLOAD_BYTES];
    let slots = MAX_PENDING_UPLOAD_BYTES / MAX_CONTENT_UPLOAD_BYTES;

    // Fill the store to exactly the aggregate ceiling, one full-size body at a time.
    let mut held: Option<(String, String, BTreeMap<String, String>)> = None;
    for slot in 0..slots {
        let reply = emulator
            .start_upload(&knowledge_base_id, "text/plain", None)
            .await;
        let upload_id = reply.string("/uploadId").expect("an uploadId");
        let url = reply.string("/url").expect("a url");
        let headers = headers_of(&reply, "aggregate fill");
        let answer = emulator.follow("PUT", &url, &headers, &full).await;
        assert_eq!(
            answer.status, 200,
            "case \"fill slot {slot}\": a write inside the aggregate ceiling must be \
             accepted, got {answer:?}"
        );
        assert_eq!(
            emulator.stored_body(&upload_id).map(|body| body.len()),
            Some(MAX_CONTENT_UPLOAD_BYTES),
            "case \"fill slot {slot}\": the whole body must be stored"
        );
        held = Some((upload_id, url, headers));
    }
    let (held_id, held_url, held_headers) = held.expect("the store has at least one slot");

    // One more byte would put the store past the ceiling, so it is refused and nothing of
    // it is stored — while the bodies already held are untouched.
    let reply = emulator
        .start_upload(&knowledge_base_id, "text/plain", None)
        .await;
    let overflow_id = reply.string("/uploadId").expect("an uploadId");
    let overflow_url = reply.string("/url").expect("a url");
    let overflow_headers = headers_of(&reply, "past the aggregate ceiling");
    let answer = emulator
        .follow("PUT", &overflow_url, &overflow_headers, b"x")
        .await;
    assert_eq!(
        answer.status, 413,
        "case \"past the aggregate ceiling\": a write the store has no room for must be \
         refused, got {answer:?}"
    );
    assert_eq!(
        emulator.stored_body(&overflow_id),
        None,
        "case \"past the aggregate ceiling\": a refused write must store nothing"
    );
    assert_eq!(
        emulator.stored_body(&held_id).map(|body| body.len()),
        Some(MAX_CONTENT_UPLOAD_BYTES),
        "case \"past the aggregate ceiling\": the bodies already held must be untouched"
    );

    // An upload's own bytes are replaced, not added to, so re-writing one the store
    // already holds is accepted at the ceiling.
    let answer = emulator
        .follow("PUT", &held_url, &held_headers, b"a smaller asset")
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"re-write an upload already held\": replacing a stored body must be \
         accepted at the ceiling, got {answer:?}"
    );
    assert_eq!(
        emulator.stored_body(&held_id),
        Some(b"a smaller asset".to_vec()),
        "case \"re-write an upload already held\": the new body must be what is stored"
    );
    // And with that body shrunk, the write refused above fits.
    let answer = emulator
        .follow("PUT", &overflow_url, &overflow_headers, b"x")
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"after the store drained\": a write that now fits must be accepted, got \
         {answer:?}"
    );
    assert_eq!(
        emulator.stored_body(&overflow_id),
        Some(b"x".to_vec()),
        "case \"after the store drained\": the body must be stored"
    );
}

#[tokio::test]
async fn a_start_against_a_knowledge_base_that_cannot_take_content_is_refused() {
    let emulator = Emulator::new();
    let (custom_id, _) = emulator
        .given_knowledge_base("refusals", "kb-custom", "CUSTOM", None)
        .await;
    let (external_id, external_arn) = emulator
        .given_knowledge_base("refusals", "kb-external", "EXTERNAL", None)
        .await;
    let (quick_responses_id, _) = emulator
        .given_knowledge_base("refusals", "kb-quick", "QUICK_RESPONSES", None)
        .await;
    let (deleted_id, _) = emulator
        .given_knowledge_base("refusals", "kb-deleted", "CUSTOM", None)
        .await;
    let delete = emulator
        .send("DELETE", &format!("/knowledgeBases/{deleted_id}"), None)
        .await;
    assert_eq!(
        delete.status, 204,
        "the knowledge base must be deleted, got {delete:?}"
    );

    let other_account_arn =
        format!("arn:aws:wisdom:{REGION}:999988887777:knowledge-base/{ABSENT_UUID}");
    let this_account_arn = format!(
        "arn:aws:wisdom:{REGION}:{account}:knowledge-base/{ABSENT_UUID}",
        account = engine::EMULATOR_ACCOUNT_ID
    );

    // (case, the knowledgeBaseId sent, the expected status, the expected error code, the
    // expected message where this endpoint states a fixed one for the class)
    let rows: Vec<(&str, String, u16, &str, Option<String>)> = vec![
        (
            "a CUSTOM knowledge base this account holds",
            custom_id,
            200,
            "",
            None,
        ),
        (
            "an EXTERNAL knowledge base, by id",
            external_id,
            400,
            "ValidationException",
            // The sentence written out rather than taken from the implementation, so the
            // row pins the template AND the lower-cased type name the wire carries.
            Some("Unsupported ContentType for KnowledgeBaseType: external".to_string()),
        ),
        (
            "an EXTERNAL knowledge base, by ARN",
            external_arn,
            400,
            "ValidationException",
            Some("Unsupported ContentType for KnowledgeBaseType: external".to_string()),
        ),
        (
            "a QUICK_RESPONSES knowledge base",
            quick_responses_id,
            400,
            "ValidationException",
            // The type's INTERNAL name, which is not its modelled value: the refusal
            // names the type the request pointed at the way this endpoint names it.
            Some("Unsupported ContentType for KnowledgeBaseType: quick_replies".to_string()),
        ),
        (
            "a knowledge base this account no longer holds",
            deleted_id,
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an id this account holds no knowledge base under",
            ABSENT_UUID.to_string(),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an ARN of this account that names no knowledge base",
            this_account_arn,
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an ARN of another account",
            other_account_arn,
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "a label outside the modelled UuidOrArn pattern",
            "not-a-knowledge-base".to_string(),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, knowledge_base_id, expected_status, expected_code, expected_message) in rows {
        let reply = emulator
            .start_upload(&knowledge_base_id, "text/plain", None)
            .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {reply:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                reply.error_type, expected_code,
                "case {case:?}: expected the code {expected_code}, got {reply:?}"
            );
        }
        if let Some(expected_message) = expected_message {
            assert_eq!(
                reply.string("/message"),
                Some(expected_message.clone()),
                "case {case:?}: expected the message {expected_message:?}, got {reply:?}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_knowledge_base_forgets_the_uploads_opened_under_it() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator
        .given_knowledge_base("delete reclaims uploads", "kb-del", "CUSTOM", None)
        .await;
    let (kept_id, _) = emulator
        .given_knowledge_base("delete reclaims uploads", "kb-kept", "CUSTOM", None)
        .await;

    let doomed = emulator
        .start_upload(&knowledge_base_id, "text/plain", None)
        .await;
    let doomed_id = doomed.string("/uploadId").expect("an uploadId");
    let doomed_url = doomed.string("/url").expect("a url");
    let doomed_headers = headers_of(&doomed, "delete reclaims uploads");

    let kept = emulator.start_upload(&kept_id, "text/html", None).await;
    let kept_upload_id = kept.string("/uploadId").expect("an uploadId");

    let delete = emulator
        .send(
            "DELETE",
            &format!("/knowledgeBases/{knowledge_base_id}"),
            None,
        )
        .await;
    assert_eq!(
        delete.status, 204,
        "case \"delete reclaims uploads\": the delete must succeed, got {delete:?}"
    );

    assert!(
        !emulator.holds_upload(&doomed_id),
        "case \"delete reclaims uploads\": the deleted knowledge base's upload must be \
         forgotten"
    );
    assert!(
        emulator.holds_upload(&kept_upload_id),
        "case \"delete reclaims uploads\": another knowledge base's upload must be \
         untouched"
    );
    let answer = emulator
        .follow("PUT", &doomed_url, &doomed_headers, b"an asset")
        .await;
    assert_ne!(
        answer.status, 200,
        "case \"delete reclaims uploads\": a URL whose upload is gone must not accept a \
         write, got {answer:?}"
    );
}
