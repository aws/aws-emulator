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
//!   the upload the URL names — whatever endpoint host the URL was minted on, including
//!   one whose name resolves no service at all — and the plane refuses a write that is
//!   late, that omits or contradicts a published header, that uses another method, that
//!   names another knowledge base's path, or that carries more bytes than this endpoint
//!   accepts;
//! * a start against a knowledge base this account does not hold is
//!   `ResourceNotFoundException` (HTTP 404), and one against a knowledge base whose type
//!   is not `CUSTOM` is `ValidationException` (HTTP 400) — the operation's own
//!   documentation: "You can only upload content to a knowledge base of type CUSTOM." —
//!   stating the type the request pointed at by the name this service uses for it
//!   internally, read from the STORED knowledge base, for every modelled type that is not
//!   `CUSTOM` and in both label forms;
//! * the written bodies this endpoint holds are bounded in AGGREGATE as well as one by
//!   one, so a caller cannot hold the product of the per-body bound and the number of
//!   upload slots in the emulator's heap.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::state::{
    self, DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES, MAX_CONTENT_UPLOAD_BYTES,
    MAX_PENDING_UPLOAD_BYTES,
};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every modelled request in this suite is signed for.
const REGION: &str = "eu-west-1";

/// The endpoint host the modelled requests address, and so the host the upload URLs this
/// suite follows are minted on.
const ENDPOINT_HOST: &str = "wisdom.eu-west-1.amazonaws.com";

/// An endpoint host whose first label names NO service — what an emulator reached over
/// loopback looks like. A URL minted on it is still followable, because the upload id in
/// its path is what routes the request.
const LOOPBACK_HOST: &str = "127.0.0.1:9297";

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

/// Every value of the modelled `KnowledgeBaseType` enum
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")`) that is NOT `CUSTOM`, so
/// the refusal table spans the whole catalog of types content cannot be uploaded to.
const NON_CUSTOM_KNOWLEDGE_BASE_TYPES: [&str; 4] = [
    "EXTERNAL",
    "QUICK_RESPONSES",
    "MESSAGE_TEMPLATES",
    "MANAGED",
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

/// The header a SIGNED write must carry for its signature to verify, and its value: a
/// query-signed URL is minted before the body exists, so the payload is signed as
/// unsigned. Written out here, as the other header names are, so the assertion pins what
/// a client has to send.
const PAYLOAD_HEADER: &str = "x-amz-content-sha256";
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

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

    /// A modelled request, signed for this endpoint's `wisdom` signing name and
    /// addressed to `host` (no `Host` header at all when it is `None`, as an HTTP/2
    /// request has none).
    async fn send_with_host(
        &self,
        host: Option<&str>,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization);
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
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

    /// A modelled request, signed for this endpoint's `wisdom` signing name and
    /// addressed to `host`.
    async fn send_to(&self, host: &str, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.send_with_host(Some(host), method, uri, body).await
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.send_to(ENDPOINT_HOST, method, uri, body).await
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
        // A knowledge base whose type names a source outside the service cannot be
        // created without one: the model documents `sourceConfiguration` as the source of
        // "EXTERNAL or Managed" knowledge bases
        // (`smithy_query(query="doc",shape="CreateKnowledgeBaseRequest",member="sourceConfiguration")`),
        // and declares one union member per source
        // (`smithy_query(query="shape",shape="SourceConfiguration")`).
        match knowledge_base_type {
            "EXTERNAL" => {
                body["sourceConfiguration"] = json!({
                    "appIntegrations": {
                        "appIntegrationArn": format!(
                            "arn:aws:app-integrations:{REGION}:{account}:data-integration/src",
                            account = engine::EMULATOR_ACCOUNT_ID
                        )
                    }
                });
            }
            "MANAGED" => {
                body["sourceConfiguration"] = json!({
                    "managedSourceConfiguration": {
                        "webCrawlerConfiguration": {
                            "urlConfiguration": {
                                "seedUrls": [{ "url": "https://example.test/docs" }]
                            }
                        }
                    }
                });
            }
            _ => {}
        }
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

    /// Start one upload against the knowledge base `label` names, addressed to `host`.
    async fn start_upload_at(
        &self,
        host: &str,
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
        self.send_to(
            host,
            "POST",
            &format!("/knowledgeBases/{encoded}/upload"),
            Some(body),
        )
        .await
    }

    async fn start_upload(
        &self,
        label: &str,
        content_type: &str,
        time_to_live: Option<i64>,
    ) -> Reply {
        self.start_upload_at(ENDPOINT_HOST, label, content_type, time_to_live)
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
        .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// Assert `url` is a URL this emulator SIGNED for `path`: an absolute `https` URL whose
/// path is the plane's own, carrying the SigV4 query signature it was minted with — the
/// shape AWS's own presigned locator for these members has.
///
/// The credential scope names the Region the request was signed for and this endpoint's
/// signing name, the expiry is the one the call asked for, and the signature is a SHA-256
/// hex digest. Nothing of the REQUEST's `Host` reaches the URL, which is what makes the
/// locator one this emulator issued rather than one a caller chose.
pub(crate) fn assert_signed_plane_url(
    case: &str,
    url: &str,
    path: &str,
    expires_secs: u64,
    region: &str,
) {
    let (address, query) = url
        .split_once('?')
        .unwrap_or_else(|| panic!("case {case:?}: the url carries no signature at all: {url}"));
    assert!(
        address.starts_with("https://") && address.ends_with(path),
        "case {case:?}: the url must address {path} over https, got {url}"
    );
    let fields: BTreeMap<&str, &str> = query
        .split('&')
        .filter_map(|field| field.split_once('='))
        .collect();
    assert_eq!(
        fields.get("X-Amz-Algorithm").copied(),
        Some("AWS4-HMAC-SHA256"),
        "case {case:?}: the url must be SigV4 query-signed, got {url}"
    );
    let scope = format!("%2F{region}%2Fwisdom%2Faws4_request");
    assert!(
        fields
            .get("X-Amz-Credential")
            .is_some_and(|credential| credential.ends_with(&scope)),
        "case {case:?}: the credential scope must name {region} and this endpoint's \
         signing name, got {url}"
    );
    assert_eq!(
        fields.get("X-Amz-Expires").copied(),
        Some(expires_secs.to_string().as_str()),
        "case {case:?}: the signature must expire after the {expires_secs}s asked for, got \
         {url}"
    );
    assert!(
        fields.get("X-Amz-Date").is_some_and(|date| !date.is_empty()),
        "case {case:?}: the url must name the instant it was signed at, got {url}"
    );
    assert!(
        fields.get("X-Amz-Signature").is_some_and(|signature| {
            signature.len() == 64 && signature.bytes().all(|byte| byte.is_ascii_hexdigit())
        }),
        "case {case:?}: the url must carry a SHA-256 signature, got {url}"
    );
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
    /// The endpoint host the start is addressed to, and so the host of the URL it mints.
    host: &'static str,
    /// The asset the `PUT` to the URL carries.
    asset: Vec<u8>,
}

/// The table: one row per axis the rules generalise over — every modelled
/// `contentType`, the time to live (absent, both published bounds, and values between),
/// whether the knowledge base is encrypted, the label form the start addresses it by,
/// the endpoint host the URL is minted on, and the size and shape of the asset uploaded.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            label: "plain text, no time to live named, unencrypted, by id",
            content_type: CONTENT_TYPES[0],
            time_to_live: None,
            kms_key_id: None,
            by_arn: false,
            host: ENDPOINT_HOST,
            asset: b"the body of an article".to_vec(),
        },
        StartCase {
            label: "html at the shortest legal time to live, by ARN",
            content_type: CONTENT_TYPES[1],
            time_to_live: Some(TIME_TO_LIVE_MIN),
            kms_key_id: None,
            by_arn: true,
            host: ENDPOINT_HOST,
            asset: b"<p>an article</p>".to_vec(),
        },
        StartCase {
            label: "csv at the longest legal time to live, encrypted, by id",
            content_type: CONTENT_TYPES[2],
            time_to_live: Some(TIME_TO_LIVE_MAX),
            kms_key_id: Some("arn:aws:kms:eu-west-1:111122223333:key/abc"),
            by_arn: false,
            host: ENDPOINT_HOST,
            asset: b"a,b,c\n1,2,3\n".to_vec(),
        },
        StartCase {
            label: "pdf, encrypted, by ARN, binary asset",
            content_type: CONTENT_TYPES[3],
            time_to_live: Some(30),
            kms_key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab"),
            by_arn: true,
            host: ENDPOINT_HOST,
            asset: vec![0u8, 1, 2, 250, 251, 255],
        },
        StartCase {
            label: "a word document minted on a loopback endpoint",
            content_type: CONTENT_TYPES[4],
            time_to_live: None,
            kms_key_id: None,
            by_arn: false,
            host: LOOPBACK_HOST,
            asset: b"PK\x03\x04 and the rest".to_vec(),
        },
        StartCase {
            label: "a salesforce wisdom-json asset",
            content_type: CONTENT_TYPES[5],
            time_to_live: Some(5),
            kms_key_id: None,
            by_arn: false,
            host: ENDPOINT_HOST,
            asset: br#"{"source":"salesforce"}"#.to_vec(),
        },
        StartCase {
            label: "a servicenow wisdom-json asset, encrypted, on a loopback endpoint",
            content_type: CONTENT_TYPES[6],
            time_to_live: Some(TIME_TO_LIVE_MAX - 1),
            kms_key_id: Some("alias/wisdom"),
            by_arn: false,
            host: LOOPBACK_HOST,
            asset: br#"{"source":"servicenow"}"#.to_vec(),
        },
        StartCase {
            label: "a zendesk wisdom-json asset, by ARN, empty body",
            content_type: CONTENT_TYPES[7],
            time_to_live: None,
            kms_key_id: None,
            by_arn: true,
            host: ENDPOINT_HOST,
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
            .given_knowledge_base(label, &format!("kb-{index}"), "CUSTOM", case.kms_key_id)
            .await;
        let addressed = if case.by_arn {
            knowledge_base_arn.as_str()
        } else {
            knowledge_base_id.as_str()
        };

        let before = SystemTime::now();
        let reply = emulator
            .start_upload_at(case.host, addressed, case.content_type, case.time_to_live)
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

        // The URL names THIS upload under THIS knowledge base — whichever label form the
        // start addressed — and is SIGNED by this emulator, so following it presents a
        // credential this emulator issued.
        let url = reply
            .string("/url")
            .unwrap_or_else(|| panic!("case {label:?}: no url in {reply:?}"));
        let minutes = case
            .time_to_live
            .unwrap_or(DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES as i64);
        assert_signed_plane_url(
            label,
            &url,
            &format!("/knowledgeBases/{knowledge_base_id}/uploads/{upload_id}"),
            (minutes * 60) as u64,
            REGION,
        );
        assert!(
            !url.contains(case.host),
            "case {label:?}: the url must name the emulator's own base rather than the \
             host the start was addressed to, got {url}"
        );

        // `urlExpiry` is the call's instant plus the time to live asked for, and this
        // emulator's documented default for a request that named none.
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

        // `headersToInclude` carries the media type the upload was started for, the
        // encryption headers only for a knowledge base that names a KMS key, and the
        // unsigned-payload marker the SIGNATURE of a write that sends a body requires.
        let mut expected_headers: BTreeMap<String, String> = BTreeMap::new();
        expected_headers.insert(
            CONTENT_TYPE_HEADER.to_string(),
            case.content_type.to_string(),
        );
        expected_headers.insert(
            PAYLOAD_HEADER.to_string(),
            UNSIGNED_PAYLOAD.to_string(),
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
            .start_upload_at(case.host, addressed, case.content_type, case.time_to_live)
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
}

#[tokio::test]
async fn a_start_against_a_knowledge_base_that_cannot_take_content_is_refused() {
    let emulator = Emulator::new();
    let (custom_id, _) = emulator
        .given_knowledge_base("refusals", "kb-custom", "CUSTOM", None)
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
    let mut rows: Vec<(String, String, u16, &str, Option<String>)> = vec![
        (
            "a CUSTOM knowledge base this account holds".to_string(),
            custom_id,
            200,
            "",
            None,
        ),
        (
            "a knowledge base this account no longer holds".to_string(),
            deleted_id,
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an id this account holds no knowledge base under".to_string(),
            ABSENT_UUID.to_string(),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an ARN of this account that names no knowledge base".to_string(),
            this_account_arn,
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an ARN of another account".to_string(),
            other_account_arn,
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "a label outside the modelled UuidOrArn pattern".to_string(),
            "not-a-knowledge-base".to_string(),
            400,
            "ValidationException",
            None,
        ),
    ];
    // Every modelled knowledge-base type content cannot be uploaded to, by id and by
    // ARN: the type of the STORED record is what the refusal reads, not the label form,
    // and the refusal names that type the way this service names it internally. The
    // expected sentence is WRITTEN OUT per type rather than taken from the
    // implementation, so each row pins the template and the type name the wire carries.
    let stated_sentences: [(&str, &str); 4] = [
        (
            "EXTERNAL",
            "Unsupported ContentType for KnowledgeBaseType: external",
        ),
        (
            "QUICK_RESPONSES",
            "Unsupported ContentType for KnowledgeBaseType: quick_replies",
        ),
        (
            "MESSAGE_TEMPLATES",
            "Unsupported ContentType for KnowledgeBaseType: message_templates",
        ),
        (
            "MANAGED",
            "Unsupported ContentType for KnowledgeBaseType: managed",
        ),
    ];
    for (index, knowledge_base_type) in NON_CUSTOM_KNOWLEDGE_BASE_TYPES.into_iter().enumerate() {
        let (id, arn) = emulator
            .given_knowledge_base(
                knowledge_base_type,
                &format!("kb-{knowledge_base_type}-{index}"),
                knowledge_base_type,
                None,
            )
            .await;
        let stated = stated_sentences
            .iter()
            .find(|(declared, _)| *declared == knowledge_base_type)
            .map(|(_, sentence)| (*sentence).to_string());
        assert!(
            stated.is_some(),
            "case {knowledge_base_type:?}: the table must state the sentence this type is \
             refused with"
        );
        rows.push((
            format!("a {knowledge_base_type} knowledge base, by id"),
            id,
            400,
            "ValidationException",
            stated.clone(),
        ));
        rows.push((
            format!("a {knowledge_base_type} knowledge base, by ARN"),
            arn,
            400,
            "ValidationException",
            stated,
        ));
    }
    // The one type whose internal name is not its modelled value, asserted again on its
    // own row so the renamed token is unmissable.
    rows.push((
        "a QUICK_RESPONSES knowledge base states its internal type name".to_string(),
        emulator
            .given_knowledge_base("token", "kb-quick-token", "QUICK_RESPONSES", None)
            .await
            .0,
        400,
        "ValidationException",
        Some("Unsupported ContentType for KnowledgeBaseType: quick_replies".to_string()),
    ));

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

/// The URL a start hands out names the EMULATOR's own base, whatever `Host` the start
/// carried: the locator is signed for that base, so nothing a caller puts in the request
/// can choose where the URL it is handed points.
#[tokio::test]
async fn the_url_names_the_emulators_own_base_whatever_host_the_request_carries() {
    let emulator = Emulator::new();
    let (knowledge_base_id, _) = emulator
        .given_knowledge_base("endpoint fallback", "kb-fallback", "CUSTOM", None)
        .await;

    // (case, the Host header sent) — a request that carries none, one whose header is not
    // a host at all, and one that names this endpoint's published host: none of them may
    // reach the URL this service reports.
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("no Host header at all", None),
        ("a Host header that is not a host", Some("not a host")),
        ("an empty Host header", Some("")),
        ("the published endpoint host", Some(ENDPOINT_HOST)),
        ("a loopback host and port", Some(LOOPBACK_HOST)),
    ];

    for (case, host) in rows {
        let reply = emulator
            .send_with_host(
                host,
                "POST",
                &format!("/knowledgeBases/{knowledge_base_id}/upload"),
                Some(json!({ "contentType": "text/plain" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the start must succeed, got {reply:?}"
        );
        let upload_id = reply
            .string("/uploadId")
            .unwrap_or_else(|| panic!("case {case:?}: no uploadId in {reply:?}"));
        let url = reply
            .string("/url")
            .unwrap_or_else(|| panic!("case {case:?}: no url in {reply:?}"));
        assert_signed_plane_url(
            case,
            &url,
            &format!("/knowledgeBases/{knowledge_base_id}/uploads/{upload_id}"),
            DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES * 60,
            REGION,
        );
        assert!(
            host.is_none_or(|host| host.is_empty() || !url.contains(host)),
            "case {case:?}: nothing of the request's Host may reach the url, got {url}"
        );
        // And it is still followable: the upload id in the path is what routes the
        // write back to the store that minted it.
        let headers = headers_of(&reply, case);
        let put = emulator.follow("PUT", &url, &headers, b"an asset").await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the fallback URL must still be followable, got {put:?}"
        );
        assert_eq!(
            emulator.stored_body(&upload_id).as_deref(),
            Some(b"an asset".as_slice()),
            "case {case:?}: the upload must hold the bytes that were PUT"
        );
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
