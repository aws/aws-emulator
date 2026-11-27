// SPDX-License-Identifier: CC0-1.0

//! The two data planes behind the URLs this service hands out — the upload destination
//! `StartContentUpload` publishes and the asset location every read of a content reports —
//! and the credential a request following one of them is held to.
//!
//! RULE: a URL this service hands out for a later caller to follow is SigV4 query-presigned
//! against the base URL this emulator is reached on, and a request following it is served as
//! an OPERATION this service names — which is what puts it through the emulator's own front
//! door, where the credential the URL carries is verified before anything is written or
//! served. A plane served from the routing-error seam instead would be reached BEFORE
//! authentication, so an emulator that verifies signatures would still serve a caller
//! presenting none.
//!
//! The suite asserts that rule at both ends:
//!
//! * over a TABLE of published URLs (the upload destination and the asset location, each in
//!   both knowledge-base label forms), that the service NAMES an operation for a request
//!   following the URL and that its routing-error seam answers nothing for it;
//! * end to end on a router that VERIFIES signatures, that a request following such a URL
//!   with no credential at all is refused and writes nothing.
//!
//! The modelled calls that publish the URLs are signed for this view's own registration key
//! (`src/service.rs`), which is how every suite of this crate addresses it.

use std::sync::Arc;

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The region every modelled request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The routing key this service answers under (`src/service.rs`'s `OWN_REGISTRATION_KEY`):
/// the shared `wisdom` endpoint identity is served by the Amazon Q in Connect crate, so this
/// legacy model is addressed by its own key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// The endpoint host the modelled requests address.
const ENDPOINT_HOST: &str = "com-amazonaws-wisdom-wisdomservice.us-west-2.amazonaws.com";

/// The base URL this emulator is reached on while the router is not served on a port
/// (`smithy_server_runtime::endpoint::UNSERVED_URL_BASE`), and its host: what a URL this
/// service presigns is signed for.
const EMULATOR_BASE_URL: &str = "https://emulator.invalid";
const EMULATOR_HOST: &str = "emulator.invalid";

/// The SigV4 signing name this endpoint is verified under, which a presigned URL it hands
/// out is scoped to (`smithy_query(query="service_info")`: sigv4Name `wisdom`).
const SIGNING_NAME: &str = "wisdom";

/// How long the URLs this suite presigns itself stay valid — long enough that no case can
/// race the clock, and well inside the seven days SigV4 allows.
const PRESIGNED_WINDOW_SECS: u64 = 3_600;

/// The media type the uploads below are started for, one value of the modelled `ContentType`
/// catalog (`smithy_query(query="operation",operation="StartContentUpload")`).
const UPLOAD_CONTENT_TYPE: &str = "text/plain";

/// The bytes a follow of the upload URL writes.
const ASSET: &[u8] = b"an asset the plane must not store unsigned";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
    /// The body as it arrived: an asset read answers bytes rather than JSON.
    raw: Bytes,
}

impl Reply {
    fn string(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// This service alone, on a fresh engine, driven in-process through the production router.
/// `authenticating` is what decides whether the router verifies the credential a request
/// carries — the configuration the assembled emulator serves traffic in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    service: EmulatedService<engine::InMemoryEngine>,
}

impl Emulator {
    fn new(authenticating: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(authenticating);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            // A second service over the SAME engine, so the claiming and routing seams can
            // be asked about a request directly — the router hands them a request it owns,
            // and what they decide is a function of the request and this one store.
            service: EmulatedService::new(Arc::clone(&engine)),
            engine,
        }
    }

    /// Turn the router's request authentication on, leaving the store as it is: what the
    /// planes answer must not depend on whether the upload was opened before or after.
    fn authenticates_requests(&mut self) {
        self.router.set_request_authentication(true);
    }

    /// A modelled request, signed for this service's registration key. The signature itself
    /// is not verified while the router's authentication is off, which is how every suite of
    /// this crate drives the modelled surface.
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
        let response = self.router.dispatch(request).await;
        reply_of(response)
    }

    /// Follow `url` the way a client does — the absolute URL as it was handed out, with NO
    /// credential of any kind beyond whatever the URL itself carries.
    async fn follow(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut builder = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("build request");
        let response = self.router.dispatch(request).await;
        reply_of(response)
    }

    /// Create a `CUSTOM` knowledge base and answer its id and ARN.
    async fn given_knowledge_base(&self, case: &str, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": "CUSTOM" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the knowledge base must be created, got {reply:?}"
        );
        (
            reply
                .string("/knowledgeBase/knowledgeBaseId")
                .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseId in {reply:?}")),
            reply
                .string("/knowledgeBase/knowledgeBaseArn")
                .unwrap_or_else(|| panic!("case {case:?}: no knowledgeBaseArn in {reply:?}")),
        )
    }

    /// Start an upload under `addressed` and answer its id and the URL it published.
    async fn given_upload(&self, case: &str, addressed: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                &format!("/knowledgeBases/{}/upload", label(addressed)),
                Some(json!({ "contentType": UPLOAD_CONTENT_TYPE })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the upload must be started, got {reply:?}"
        );
        (
            reply
                .string("/uploadId")
                .unwrap_or_else(|| panic!("case {case:?}: no uploadId in {reply:?}")),
            reply
                .string("/url")
                .unwrap_or_else(|| panic!("case {case:?}: no url in {reply:?}")),
        )
    }

    /// Finalize that upload into a content and answer the asset URL the content reports.
    async fn given_content(&self, case: &str, addressed: &str, upload_id: &str) -> String {
        let reply = self
            .send(
                "POST",
                &format!("/knowledgeBases/{}/contents", label(addressed)),
                Some(json!({ "name": format!("content-{upload_id}"), "uploadId": upload_id })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the content must be created, got {reply:?}"
        );
        reply
            .string("/content/url")
            .unwrap_or_else(|| panic!("case {case:?}: no content url in {reply:?}"))
    }

    /// The bytes the store holds for `upload_id`, read directly: this model binds no
    /// operation that reads an upload back.
    fn stored_body(&self, upload_id: &str) -> Option<Vec<u8>> {
        let state = service_com_amazonaws_wisdom_wisdomservice::state::service_state(&*self.engine)
            .expect("the store is mounted on this engine");
        let store = state.lock();
        store
            .content_upload(upload_id)
            .and_then(|upload| upload.body.clone())
    }
}

/// One dispatched answer, as the assertions read it.
fn reply_of(response: http::Response<Bytes>) -> Reply {
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
        raw,
    }
}

/// Percent-encode a path label the way an AWS SDK encodes a non-greedy `@httpLabel`: an ARN
/// label carries `:` and `/`, which must not be read as path structure.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `url` as the runtime-API request a client following it sends, so the service's own seams
/// can be asked what they make of it.
fn following(method: &str, url: &str) -> RtRequest {
    let mut request = RtRequest::new(SdkBody::empty());
    request.set_method(method).expect("the verb is a verb");
    request.set_uri(url).expect("the url is a uri");
    request
}

/// `method` and `path` as a URL presigned with the credential this process publishes — the
/// same presigner the framework's URL signer mints a destination with
/// (`emulator_server::request_authentication::presign_url`), for the signing name this
/// endpoint is verified under and the base URL this emulator is reached on.
fn presigned(method: &str, path: &str, valid_for_secs: u64) -> String {
    let keys = ::emulator_server::request_authentication::SigV4Authenticator::for_this_process();
    let target = ::emulator_server::request_authentication::PresignTarget {
        endpoint: EMULATOR_BASE_URL,
        method,
        signing_name: SIGNING_NAME,
        region: REGION,
        path,
        query: &[],
        headers: &[],
    };
    ::emulator_server::request_authentication::presign_url(
        &target,
        &keys.access_key_id,
        &keys.secret_access_key,
        std::time::SystemTime::now(),
        valid_for_secs,
    )
    .expect("the plane's own path presigns")
}

/// The headers a presigned write must send: the payload declaration the presigner signs in
/// for a write outside the Amazon S3 family
/// (`emulator_server::request_authentication::presigned_payload_header`), the host it was
/// signed for, and the media type the upload itself requires.
fn write_headers() -> Vec<(&'static str, &'static str)> {
    let mut headers = vec![
        ("host", EMULATOR_HOST),
        ("content-type", UPLOAD_CONTENT_TYPE),
    ];
    headers.extend(::emulator_server::request_authentication::presigned_payload_header(
        SIGNING_NAME,
        "PUT",
    ));
    headers
}

/// RULE, the positive half: on a router that VERIFIES signatures, a correctly presigned
/// follow of either published plane is SERVED — the write stores its bytes and the read
/// answers them — so the destinations this service publishes are usable in the configuration
/// the emulator serves traffic in, not only on one that checks nothing.
#[tokio::test]
async fn a_correctly_presigned_follow_is_served_once_the_router_verifies_signatures() {
    // The modelled calls that open the upload and finalize the content run first: this view
    // of the endpoint answers under its own registration key, which the front door of an
    // authenticating emulator does not verify a credential for (the key a request must be
    // scoped to there is the endpoint's own signing name).
    let mut emulator = Emulator::new(false);
    let (knowledge_base_id, _) = emulator
        .given_knowledge_base("presigned follow", "kb-presigned")
        .await;
    let (upload_id, _) = emulator
        .given_upload("presigned follow", &knowledge_base_id)
        .await;
    let upload_path = format!("/knowledgeBases/{knowledge_base_id}/uploads/{upload_id}");

    emulator.authenticates_requests();

    // The write: presigned for this plane's own path, carrying the headers such a request
    // sends.
    let case = "a presigned PUT of the upload destination";
    let written = emulator
        .follow(
            "PUT",
            &presigned("PUT", &upload_path, PRESIGNED_WINDOW_SECS),
            &write_headers(),
            ASSET,
        )
        .await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: expected the front door to accept a correctly presigned write, \
         got {written:?}"
    );
    assert_eq!(
        emulator.stored_body(&upload_id).as_deref(),
        Some(ASSET),
        "case {case:?}: the upload must hold exactly the bytes that were PUT, got {:?}",
        emulator.stored_body(&upload_id)
    );

    // And the read of the asset those bytes became, over the same front door. The content is
    // finalized with the router's authentication off again, for the reason above.
    emulator.router.set_request_authentication(false);
    let asset_url = emulator
        .given_content("presigned follow", &knowledge_base_id, &upload_id)
        .await;
    let asset_path = asset_url
        .split_once('?')
        .map_or(asset_url.as_str(), |(address, _)| address)
        .trim_start_matches(EMULATOR_BASE_URL)
        .to_string();
    emulator.authenticates_requests();

    let case = "a presigned GET of the content asset";
    let read = emulator
        .follow(
            "GET",
            &presigned("GET", &asset_path, PRESIGNED_WINDOW_SECS),
            &[("host", EMULATOR_HOST)],
            b"",
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected the front door to accept a correctly presigned read, \
         got {read:?}"
    );
    assert_eq!(
        read.raw.as_ref(),
        ASSET,
        "case {case:?}: the asset must be the bytes the upload received, got {:?}",
        String::from_utf8_lossy(&read.raw)
    );
}

/// RULE: for every URL this service publishes, a request following it is served as an
/// operation this service NAMES — never from the routing-error seam, which is reached before
/// authentication — so the front door holds the request to the credential the presigned URL
/// carries.
#[tokio::test]
async fn a_request_following_a_published_url_is_served_as_an_operation_behind_the_front_door() {
    let emulator = Emulator::new(false);

    // The rows differ along which URL is followed and which label form published it: neither
    // decides where the request is served from.
    let mut rows: Vec<(&str, &str, String)> = Vec::new();
    for (index, (case, by_arn)) in [("by id", false), ("by ARN", true)].into_iter().enumerate() {
        let (knowledge_base_id, knowledge_base_arn) = emulator
            .given_knowledge_base(case, &format!("kb-plane-{index}"))
            .await;
        let addressed = if by_arn {
            knowledge_base_arn.as_str()
        } else {
            knowledge_base_id.as_str()
        };
        let (upload_id, upload_url) = emulator.given_upload(case, addressed).await;
        rows.push(("the upload destination", "PUT", upload_url));
        let followed = emulator
            .follow(
                "PUT",
                &rows.last().expect("the row just pushed").2,
                &[("content-type", UPLOAD_CONTENT_TYPE)],
                ASSET,
            )
            .await;
        assert_eq!(
            followed.status, 200,
            "case {case:?}: the published upload URL must be followable, got {followed:?}"
        );
        let asset_url = emulator.given_content(case, addressed, &upload_id).await;
        rows.push(("the content asset location", "GET", asset_url));
    }

    for (plane, method, url) in rows {
        let request = following(method, &url);
        let case = format!("{plane}, followed with {method}");
        assert!(
            emulator.service.override_operation(&request).is_some(),
            "case {case:?}: the service must name the operation that serves {url:?}, so the \
             request is authenticated like every other one"
        );
        assert!(
            emulator.service.override_routing_error(&request).is_none(),
            "case {case:?}: the routing-error seam is reached before authentication, so it \
             must answer nothing for {url:?}"
        );
    }
}

/// RULE: on a router that VERIFIES signatures, a request following a published URL with no
/// credential at all is refused, and nothing it carried is stored.
#[tokio::test]
async fn an_uncredentialed_follow_is_refused_once_the_router_verifies_signatures() {
    let mut emulator = Emulator::new(false);
    let (knowledge_base_id, _) = emulator
        .given_knowledge_base("uncredentialed follow", "kb-unsigned")
        .await;
    let (upload_id, upload_url) = emulator
        .given_upload("uncredentialed follow", &knowledge_base_id)
        .await;
    // The signature the start minted is dropped, which is exactly what a caller presenting
    // no credential sends: the path the URL published, and nothing else.
    let address = upload_url
        .split_once('?')
        .map_or(upload_url.as_str(), |(address, _)| address)
        .to_string();

    emulator.authenticates_requests();
    for (case, method, headers, body) in [
        (
            "an unsigned PUT of the upload destination",
            "PUT",
            vec![("content-type", UPLOAD_CONTENT_TYPE)],
            ASSET,
        ),
        (
            "an unsigned PUT carrying none of the published headers",
            "PUT",
            Vec::new(),
            ASSET,
        ),
        (
            "an unsigned GET of the upload destination",
            "GET",
            Vec::new(),
            b"".as_slice(),
        ),
    ] {
        let followed = emulator.follow(method, &address, &headers, body).await;
        assert_eq!(
            followed.status, 403,
            "case {case:?}: expected the front door to refuse a request carrying no \
             credential with HTTP 403, got {followed:?}"
        );
        assert!(
            followed.error_type.contains("AccessDenied"),
            "case {case:?}: expected the refusal to name the access-denied class, got \
             {:?}",
            followed.error_type
        );
        assert_eq!(
            emulator.stored_body(&upload_id),
            None,
            "case {case:?}: a refused follow must store nothing, got {:?}",
            emulator.stored_body(&upload_id)
        );
    }
}
