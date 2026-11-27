// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the attachment slice: `StartAttachmentUpload`,
//! `CompleteAttachmentUpload`, `GetAttachment` and the pre-signed plane both of them
//! publish URLs into.
//!
//! The rule under test: one attachment is a STORED resource of one chat with a lifecycle —
//! started (`IN_PROGRESS`), confirmed (`APPROVED`), then downloadable — and every URL these
//! operations report is a pre-signed address of this emulator that really serves the file
//! that was uploaded to it. So each case drives the whole path over the wire, through the
//! same `MultiServiceRouter` the production router dispatches with: the control-plane call,
//! the `PUT` to the published target, the read back, and the `GET` of the download URL.
//!
//! Whether a chat may share attachments at all is a rule of its own, asserted at the end of
//! this suite: a chat file is kept in the storage the Amazon Connect instance the contact
//! belongs to is configured with, so a chat of an instance configured with none is refused
//! every attachment operation (403 `AccessDeniedException`), and the answer follows that
//! configuration as it stands at the time of the call.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_connectparticipant_amazonconnectparticipantservicelambda as participant;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host the modelled requests address.
const HOST: &str = "participant.connect.us-east-1.amazonaws.com";

/// A SigV4 credential scope naming this service's signing name, which is what the router
/// resolves a request's target service from.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/\
                             execute-api/aws4_request, SignedHeaders=host, Signature=00";

/// A harness with just this service mounted on one shared engine.
fn router() -> Harness {
    router_and_engine().0
}

/// [`router`], plus the shared engine behind it — the store the component that starts a chat
/// writes its session into, and the one every handler reads.
fn router_and_engine() -> (Harness, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(participant::EmulatedService::new(engine.clone()));
    (
        Harness {
            router,
            engine: engine.clone(),
        },
        engine,
    )
}

/// The router plus the shared engine: a participant token is issued where a chat is
/// STARTED, so a test that connects one registers the chat in that store first
/// ([`Harness::start_chat`]), exactly as `StartChatContact` does.
///
/// Derefs to the router, so a request is dispatched through the harness itself.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl std::ops::Deref for Harness {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &MultiServiceRouter {
        &self.router
    }
}

impl Harness {
    /// Register the chat a `StartChatContact` would have started for `participant_token`,
    /// and answer its contact id. Registering one token twice keeps the session the first
    /// call registered, because a participant token names one participant of one contact.
    ///
    /// The chat is one of an Amazon Connect instance that IS configured with somewhere to
    /// keep an attached file, which is what lets its participant share attachments at all
    /// (see [`Harness::set_attachment_sharing`]).
    fn start_chat(&self, participant_token: &str) -> String {
        self.start_chat_sharing(participant_token, true)
    }

    /// [`Harness::start_chat`], for a chat whose instance is configured with somewhere to
    /// keep an attached file only when `attachments_shared`.
    fn start_chat_sharing(&self, participant_token: &str, attachments_shared: bool) -> String {
        let store = store(&self.engine);
        let mut state = store.lock();
        if let Some(contact_id) = state.participant_tokens.get(participant_token) {
            return contact_id.clone();
        }
        let mut session = participant::state::ChatSession::started(
            participant::state::new_id(),
            participant::state::new_id(),
            Some("sim-customer".to_string()),
            std::time::SystemTime::now() + participant::state::CONNECTION_TOKEN_TTL,
        );
        session.attachments_shared = attachments_shared;
        let contact_id = session.contact_id.clone();
        state.register_session(participant_token.to_string(), session);
        contact_id
    }

    /// Write whether the Amazon Connect instance the chat `contact_id` belongs to is
    /// configured with somewhere to keep an attached file, exactly as the component that
    /// associates (or drops) that storage publishes it onto this chat's session.
    fn set_attachment_sharing(&self, contact_id: &str, shared: bool) {
        let store = store(&self.engine);
        let mut state = store.lock();
        state
            .sessions
            .get_mut(contact_id)
            .expect("the registered chat session")
            .attachments_shared = shared;
    }

    /// How many attachments this store holds for `contact_id`.
    fn attachments_held(&self, contact_id: &str) -> usize {
        store(&self.engine).lock().attachment_count(contact_id)
    }
}

/// This service's store in the shared engine.
fn store(
    engine: &Arc<engine::InMemoryEngine>,
) -> ::engine::ServiceState<
    participant::state::ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState,
> {
    participant::state::service_state(&**engine).expect("the connectparticipant store")
}

/// POST `path` with an optional `X-Amz-Bearer` token, returning the status and the decoded
/// JSON body (`Value::Null` for an empty body).
async fn post(
    router: &MultiServiceRouter,
    path: &str,
    bearer: Option<&str>,
    body: &Value,
) -> (u16, Value) {
    let (status, _, decoded) = post_answer(router, path, bearer, body).await;
    (status, decoded)
}

/// [`post`], also answering the `x-amzn-errortype` header a refusal names its error shape
/// in (the empty string when the response carries none).
async fn post_answer(
    router: &MultiServiceRouter,
    path: &str,
    bearer: Option<&str>,
    body: &Value,
) -> (u16, String, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION);
    if let Some(bearer) = bearer {
        request = request.header("x-amz-bearer", bearer);
    }
    let request = request
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, error_type, decoded)
}

/// What a request to a published attachment URL answered: the status, the body, and the
/// `content-type` and `content-disposition` it was served under.
struct Fetched {
    status: u16,
    body: Vec<u8>,
    content_type: Option<String>,
    content_disposition: Option<String>,
}

impl Fetched {
    /// The body decoded as JSON, for a refusal of the attachment plane.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// Follow a URL this emulator published, exactly as a client holding it would: the URL as
/// signed, carrying no `Authorization` header of its own, plus `headers`.
async fn fetch(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Fetched {
    let mut request = http::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let request = request
        .body(Bytes::from(body))
        .expect("build the request to the published URL");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    Fetched {
        status,
        body: response.into_body().to_vec(),
        content_type,
        content_disposition,
    }
}

/// The `__type` discriminator of a restJson1 error body.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert `value` is an `ISO8601Datetime` / `Instant`: `yyyy-MM-ddThh:mm:ss.SSSZ`, the form
/// every timestamp in this model documents (e.g. `2019-11-08T02:41:28.172Z`). Checked here
/// independently of the service's own formatter.
fn assert_iso8601_millis(case: &str, value: &str) {
    assert_eq!(
        value.len(),
        24,
        "case {case:?}: expected a 24-character yyyy-MM-ddThh:mm:ss.SSSZ timestamp, got {value:?}"
    );
    let chars: Vec<char> = value.chars().collect();
    for (at, expected) in [
        (4, '-'),
        (7, '-'),
        (10, 'T'),
        (13, ':'),
        (16, ':'),
        (19, '.'),
        (23, 'Z'),
    ] {
        assert_eq!(
            chars[at], expected,
            "case {case:?}: expected {expected:?} at index {at} of {value:?}, got {:?}",
            chars[at]
        );
    }
    for at in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22] {
        assert!(
            chars[at].is_ascii_digit(),
            "case {case:?}: expected a digit at index {at} of {value:?}, got {:?}",
            chars[at]
        );
    }
}

/// The value of query parameter `name` in `url`, still percent-encoded.
fn query_param(url: &str, name: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

/// Start the chat `participant_token` belongs to, open a connection for it and return its
/// `ConnectionToken`.
async fn connect(harness: &Harness, participant_token: &str) -> String {
    harness.start_chat(participant_token);
    open_connection(harness, participant_token).await
}

/// [`connect`], for a chat whose Amazon Connect instance is configured with somewhere to
/// keep an attached file only when `attachments_shared`.
async fn connect_sharing(
    harness: &Harness,
    participant_token: &str,
    attachments_shared: bool,
) -> String {
    harness.start_chat_sharing(participant_token, attachments_shared);
    open_connection(harness, participant_token).await
}

/// Open a connection for a chat already registered, answering the `ConnectionToken` every
/// later call authenticates with.
async fn open_connection(harness: &Harness, participant_token: &str) -> String {
    let (status, body) = post(
        harness,
        "/participant/connection",
        Some(participant_token),
        &json!({"Type": ["CONNECTION_CREDENTIALS"]}),
    )
    .await;
    assert_eq!(status, 200, "CreateParticipantConnection: got {body:?}");
    body["ConnectionCredentials"]["ConnectionToken"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateParticipantConnection returned no ConnectionToken: {body:?}"))
        .to_string()
}

/// One started upload: the id the attachment is named by, and the published target.
struct Started {
    attachment_id: String,
    url: String,
    url_expiry: String,
    headers: Vec<(String, String)>,
}

/// `StartAttachmentUpload` for one file, asserting it was served, and reading the published
/// target out of the response.
async fn start(
    router: &MultiServiceRouter,
    connection_token: &str,
    case: &str,
    name: &str,
    content_type: &str,
    size: usize,
    client_token: &str,
) -> Started {
    let (status, body) = start_raw(
        router,
        connection_token,
        name,
        content_type,
        size as i64,
        client_token,
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: StartAttachmentUpload got {body:?}");
    Started {
        attachment_id: body["AttachmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no AttachmentId in {body:?}"))
            .to_string(),
        url: body["UploadMetadata"]["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no UploadMetadata.Url in {body:?}"))
            .to_string(),
        url_expiry: body["UploadMetadata"]["UrlExpiry"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no UploadMetadata.UrlExpiry in {body:?}"))
            .to_string(),
        headers: body["UploadMetadata"]["HeadersToInclude"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: no HeadersToInclude in {body:?}"))
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
    }
}

/// `StartAttachmentUpload` as sent, without asserting the outcome.
async fn start_raw(
    router: &MultiServiceRouter,
    connection_token: &str,
    name: &str,
    content_type: &str,
    size: i64,
    client_token: &str,
) -> (u16, Value) {
    post(
        router,
        "/participant/start-attachment-upload",
        Some(connection_token),
        &json!({
            "AttachmentName": name,
            "AttachmentSizeInBytes": size,
            "ContentType": content_type,
            "ClientToken": client_token,
        }),
    )
    .await
}

/// `CompleteAttachmentUpload` for one attachment.
async fn complete(
    router: &MultiServiceRouter,
    connection_token: &str,
    attachment_id: &str,
    client_token: &str,
) -> (u16, Value) {
    post(
        router,
        "/participant/complete-attachment-upload",
        Some(connection_token),
        &json!({"AttachmentIds": [attachment_id], "ClientToken": client_token}),
    )
    .await
}

/// `GetAttachment` for one attachment, with an optional `UrlExpiryInSeconds`.
async fn get_attachment(
    router: &MultiServiceRouter,
    connection_token: &str,
    attachment_id: &str,
    url_expiry_in_seconds: Option<i64>,
) -> (u16, Value) {
    let mut body = json!({"AttachmentId": attachment_id});
    if let Some(seconds) = url_expiry_in_seconds {
        body["UrlExpiryInSeconds"] = json!(seconds);
    }
    post(router, "/participant/attachment", Some(connection_token), &body).await
}

/// `GetAttachment` for one attachment, with an optional `UrlContentDisposition`.
async fn get_attachment_presented(
    router: &MultiServiceRouter,
    connection_token: &str,
    attachment_id: &str,
    url_content_disposition: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"AttachmentId": attachment_id});
    if let Some(disposition) = url_content_disposition {
        body["UrlContentDisposition"] = json!(disposition);
    }
    post(router, "/participant/attachment", Some(connection_token), &body).await
}

/// Start, upload and confirm one file, answering the attachment it minted: the state every
/// case that reads a file back needs, driven over the wire exactly as a client drives it.
async fn stored_attachment(
    router: &MultiServiceRouter,
    connection_token: &str,
    file: &File,
    client_token: &str,
) -> Started {
    let started = start(
        router,
        connection_token,
        file.case,
        file.name,
        file.content_type,
        file.size,
        client_token,
    )
    .await;
    let uploaded = fetch(router, "PUT", &started.url, &started.headers, file.bytes()).await;
    assert_eq!(
        uploaded.status,
        200,
        "case {:?}: the upload to the published target got {:?}",
        file.case,
        uploaded.json()
    );
    let (status, body) = complete(
        router,
        connection_token,
        &started.attachment_id,
        &format!("{client_token}-complete"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CompleteAttachmentUpload got {body:?}",
        file.case
    );
    started
}

/// One file an upload is started for. The rows differ along what the rule generalises over:
/// the media type, the name, and the size of the file.
struct File {
    case: &'static str,
    name: &'static str,
    content_type: &'static str,
    /// The bytes the file holds, built by repeating `fill` `size` times, so a case can be
    /// as large as it likes without a literal that long.
    fill: u8,
    size: usize,
}

/// The files every table in this suite is driven over.
const FILES: &[File] = &[
    File {
        case: "one byte of text",
        name: "note.txt",
        content_type: "text/plain",
        fill: b'a',
        size: 1,
    },
    File {
        case: "a small csv",
        name: "Quarterly Report, final.csv",
        content_type: "text/csv",
        fill: b',',
        size: 512,
    },
    File {
        case: "a binary document",
        name: "scan.pdf",
        content_type: "application/pdf",
        fill: 0x7f,
        size: 4096,
    },
    File {
        case: "an opaque stream",
        name: "capture.bin",
        content_type: "application/octet-stream",
        fill: 0x00,
        size: 65_536,
    },
];

impl File {
    /// The bytes this file carries.
    fn bytes(&self) -> Vec<u8> {
        vec![self.fill; self.size]
    }
}

#[tokio::test]
async fn an_upload_publishes_a_presigned_target_for_the_file_it_declares() {
    let router = router();
    let connection_token = connect(&router, "participant-upload-targets").await;

    let mut seen_ids: Vec<String> = Vec::new();
    for (index, file) in FILES.iter().enumerate() {
        let started = start(
            &router,
            &connection_token,
            file.case,
            file.name,
            file.content_type,
            file.size,
            &format!("start-{index}"),
        )
        .await;

        assert!(
            !seen_ids.contains(&started.attachment_id),
            "case {:?}: expected a fresh AttachmentId, got the already-issued {:?}",
            file.case,
            started.attachment_id
        );
        seen_ids.push(started.attachment_id.clone());

        // The target is an address of THIS emulator, pre-signed, naming this attachment's
        // own object path.
        assert!(
            started.url.starts_with("https://"),
            "case {:?}: expected an https upload URL, got {:?}",
            file.case,
            started.url
        );
        assert!(
            started.url.contains("/participant/attachments/"),
            "case {:?}: expected the attachment object path in {:?}",
            file.case,
            started.url
        );
        assert!(
            started.url.contains(&started.attachment_id),
            "case {:?}: expected the upload URL to name attachment {:?}, got {:?}",
            file.case,
            started.attachment_id,
            started.url
        );
        for parameter in ["X-Amz-Credential", "X-Amz-Signature", "X-Amz-Expires"] {
            assert!(
                query_param(&started.url, parameter).is_some(),
                "case {:?}: expected a pre-signed {parameter} on {:?}",
                file.case,
                started.url
            );
        }
        // The upload target carries no window parameter of its own, so it is signed for the
        // longest window `UrlExpiryInSeconds` allows (300 seconds).
        assert_eq!(
            query_param(&started.url, "X-Amz-Expires").as_deref(),
            Some("300"),
            "case {:?}: expected the published upload window on {:?}",
            file.case,
            started.url
        );
        assert_iso8601_millis(file.case, &started.url_expiry);

        // "The headers to be provided while uploading the file to the URL": the media type
        // the signature bound, and the length of the declared file.
        let header = |name: &str| {
            started
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            header("content-type").as_deref(),
            Some(file.content_type),
            "case {:?}: expected HeadersToInclude to bind the declared ContentType, got {:?}",
            file.case,
            started.headers
        );
        assert_eq!(
            header("content-length").as_deref(),
            Some(file.size.to_string().as_str()),
            "case {:?}: expected HeadersToInclude to carry the declared size, got {:?}",
            file.case,
            started.headers
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_replays_the_attachment_the_first_call_minted() {
    let router = router();
    let connection_token = connect(&router, "participant-idempotent-upload").await;

    for file in FILES {
        let first = start(
            &router,
            &connection_token,
            file.case,
            file.name,
            file.content_type,
            file.size,
            "one-and-only-token",
        )
        .await;
        let second = start(
            &router,
            &connection_token,
            file.case,
            file.name,
            file.content_type,
            file.size,
            "one-and-only-token",
        )
        .await;
        assert_eq!(
            (second.attachment_id.as_str(), second.url.as_str()),
            (first.attachment_id.as_str(), first.url.as_str()),
            "case {:?}: expected the repeat to replay the first attachment and its target",
            file.case
        );
    }
}

#[tokio::test]
async fn the_file_uploaded_to_the_published_target_is_the_file_downloaded_back() {
    let router = router();
    let connection_token = connect(&router, "participant-attachment-lifecycle").await;

    for (index, file) in FILES.iter().enumerate() {
        let started = start(
            &router,
            &connection_token,
            file.case,
            file.name,
            file.content_type,
            file.size,
            &format!("lifecycle-start-{index}"),
        )
        .await;

        // Before the upload is confirmed there is no download at all.
        let (status, body) =
            get_attachment(&router, &connection_token, &started.attachment_id, None).await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected GetAttachment before completion to be refused, got {body:?}",
            file.case
        );

        // The file goes to the published target with the headers that were reported.
        let uploaded = fetch(
            &router,
            "PUT",
            &started.url,
            &started.headers,
            file.bytes(),
        )
        .await;
        assert_eq!(
            uploaded.status,
            200,
            "case {:?}: expected the upload to be stored, got {:?}",
            file.case,
            uploaded.json()
        );

        let (status, body) = complete(
            &router,
            &connection_token,
            &started.attachment_id,
            &format!("lifecycle-complete-{index}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CompleteAttachmentUpload got {body:?}",
            file.case
        );

        // The read back reports the stored size and a pre-signed download address.
        let (status, body) =
            get_attachment(&router, &connection_token, &started.attachment_id, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetAttachment got {body:?}",
            file.case
        );
        assert_eq!(
            body["AttachmentSizeInBytes"].as_u64(),
            Some(file.size as u64),
            "case {:?}: expected the declared size, got {body:?}",
            file.case
        );
        let download_url = body["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Url in {body:?}", file.case))
            .to_string();
        assert_iso8601_millis(
            file.case,
            body["UrlExpiry"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no UrlExpiry in {body:?}", file.case)),
        );

        // And that address serves the very bytes that were uploaded.
        let downloaded = fetch(&router, "GET", &download_url, &[], Vec::new()).await;
        assert_eq!(
            downloaded.status,
            200,
            "case {:?}: expected the download to be served, got {:?}",
            file.case,
            downloaded.json()
        );
        assert_eq!(
            downloaded.body,
            file.bytes(),
            "case {:?}: expected the uploaded bytes back, got {} bytes",
            file.case,
            downloaded.body.len()
        );
        assert_eq!(
            downloaded.content_type.as_deref(),
            Some(file.content_type),
            "case {:?}: expected the declared media type on the download",
            file.case
        );

        // A `HEAD` reports the object without its bytes.
        let headed = fetch(&router, "HEAD", &download_url, &[], Vec::new()).await;
        assert_eq!(
            (headed.status, headed.body.len()),
            (200, 0),
            "case {:?}: expected HEAD to report the object with no body",
            file.case
        );

        // The confirmed attachment is part of the conversation, so the transcript carries it.
        let (status, transcript) = post(
            &router,
            "/participant/transcript",
            Some(&connection_token),
            &json!({"MaxResults": 100}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetTranscript got {transcript:?}",
            file.case
        );
        let item = transcript["Transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no Transcript in {transcript:?}", file.case))
            .iter()
            .find(|item| {
                item["Attachments"][0]["AttachmentId"].as_str() == Some(&started.attachment_id)
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: no transcript item for attachment {:?} in {transcript:?}",
                    file.case, started.attachment_id
                )
            })
            .clone();
        assert_eq!(
            (
                item["Type"].as_str(),
                item["Attachments"][0]["AttachmentName"].as_str(),
                item["Attachments"][0]["ContentType"].as_str(),
                item["Attachments"][0]["Status"].as_str(),
            ),
            (
                Some("ATTACHMENT"),
                Some(file.name),
                Some(file.content_type),
                Some("APPROVED"),
            ),
            "case {:?}: unexpected transcript item {item:?}",
            file.case
        );
    }
}

#[tokio::test]
async fn confirming_one_upload_twice_conflicts() {
    let router = router();
    let connection_token = connect(&router, "participant-complete-twice").await;

    for (index, file) in FILES.iter().enumerate() {
        let started = start(
            &router,
            &connection_token,
            file.case,
            file.name,
            file.content_type,
            file.size,
            &format!("twice-start-{index}"),
        )
        .await;
        let (status, body) = complete(
            &router,
            &connection_token,
            &started.attachment_id,
            &format!("twice-first-{index}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first confirmation got {body:?}",
            file.case
        );

        // A second confirmation of the same attachment, as its own request.
        let (status, body) = complete(
            &router,
            &connection_token,
            &started.attachment_id,
            &format!("twice-second-{index}"),
        )
        .await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (409, "ConflictException"),
            "case {:?}: expected the second confirmation to conflict, got {body:?}",
            file.case
        );

        // While a REPEAT of the first request — the same client token — is the same
        // confirmation, so it is answered as the first one was.
        let (status, body) = complete(
            &router,
            &connection_token,
            &started.attachment_id,
            &format!("twice-first-{index}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the repeated confirmation to replay, got {body:?}",
            file.case
        );
    }
}

#[tokio::test]
async fn an_attachment_is_reachable_only_through_its_own_chats_connection() {
    let router = router();
    let mine = connect(&router, "participant-owner").await;
    let other = connect(&router, "participant-bystander").await;

    for (index, file) in FILES.iter().enumerate() {
        let started = start(
            &router,
            &mine,
            file.case,
            file.name,
            file.content_type,
            file.size,
            &format!("scope-start-{index}"),
        )
        .await;
        let (status, body) = complete(
            &router,
            &mine,
            &started.attachment_id,
            &format!("scope-complete-{index}"),
        )
        .await;
        assert_eq!(status, 200, "case {:?}: confirmation got {body:?}", file.case);

        // Another chat's participant cannot read it, and cannot confirm it either: an
        // attachment of one contact is not an attachment of another's.
        let (status, body) = get_attachment(&router, &other, &started.attachment_id, None).await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected another chat's GetAttachment to be refused, got {body:?}",
            file.case
        );
        let (status, body) = complete(
            &router,
            &other,
            &started.attachment_id,
            &format!("scope-foreign-{index}"),
        )
        .await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected another chat's confirmation to be refused, got {body:?}",
            file.case
        );

        // And a connection token that authenticates nobody is denied outright.
        let unknown = format!("not-a-connection-token-{index}");
        let (status, body) =
            start_raw(&router, &unknown, file.name, file.content_type, 1, "denied").await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {:?}: expected StartAttachmentUpload to deny an unknown token, got {body:?}",
            file.case
        );
        let (status, body) = complete(&router, &unknown, &started.attachment_id, "denied").await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {:?}: expected CompleteAttachmentUpload to deny an unknown token, got {body:?}",
            file.case
        );
        let (status, body) = get_attachment(&router, &unknown, &started.attachment_id, None).await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {:?}: expected GetAttachment to deny an unknown token, got {body:?}",
            file.case
        );
    }
}

#[tokio::test]
async fn an_attachment_nobody_minted_is_refused() {
    let router = router();
    let connection_token = connect(&router, "participant-unknown-attachment").await;

    // `ArtifactId` is `len 1..256`, so each of these satisfies the model and names no
    // attachment this chat holds.
    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-an-id",
        &"a".repeat(256),
    ] {
        let (status, body) = get_attachment(&router, &connection_token, id, None).await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {id:?}: expected GetAttachment to refuse an unknown attachment, got {body:?}"
        );
        let (status, body) = complete(&router, &connection_token, id, "unknown").await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {id:?}: expected CompleteAttachmentUpload to refuse an unknown attachment, \
             got {body:?}"
        );
    }
}

#[tokio::test]
async fn the_download_window_is_the_one_the_caller_asked_for() {
    let router = router();
    let connection_token = connect(&router, "participant-download-window").await;
    let started = start(
        &router,
        &connection_token,
        "download window",
        "note.txt",
        "text/plain",
        1,
        "window-start",
    )
    .await;
    let (status, body) = complete(
        &router,
        &connection_token,
        &started.attachment_id,
        "window-complete",
    )
    .await;
    assert_eq!(status, 200, "CompleteAttachmentUpload got {body:?}");

    // `UrlExpiryInSeconds` is modelled `range 5..300`: both published bounds, a value
    // inside them, and the absent member, which takes the longest window the range allows.
    for (case, requested, signed_for) in [
        ("the published minimum", Some(5), "5"),
        ("inside the range", Some(60), "60"),
        ("the published maximum", Some(300), "300"),
        ("no window asked for", None, "300"),
    ] {
        let (status, body) =
            get_attachment(&router, &connection_token, &started.attachment_id, requested).await;
        assert_eq!(status, 200, "case {case:?}: GetAttachment got {body:?}");
        let url = body["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Url in {body:?}"));
        assert_eq!(
            query_param(url, "X-Amz-Expires").as_deref(),
            Some(signed_for),
            "case {case:?}: expected the download URL signed for {signed_for} seconds, got {url:?}"
        );
    }

    // One past each bound is outside the modelled range, so the request is refused.
    for (case, requested) in [("one below the minimum", 4), ("one above the maximum", 301)] {
        let (status, body) = get_attachment(
            &router,
            &connection_token,
            &started.attachment_id,
            Some(requested),
        )
        .await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected UrlExpiryInSeconds {requested} to be refused, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_attachment_larger_than_this_emulator_holds_is_refused() {
    let router = router();
    let connection_token = connect(&router, "participant-attachment-size").await;
    let max = participant::state::MAX_ATTACHMENT_BYTES;

    // The bound itself is accepted — nothing is uploaded for it here, so only the
    // declaration is judged — and one byte past it is refused.
    let (status, body) = start_raw(
        &router,
        &connection_token,
        "at the bound.bin",
        "application/octet-stream",
        max,
        "size-at-bound",
    )
    .await;
    assert_eq!(status, 200, "case \"at the bound\": got {body:?}");

    let (status, body) = start_raw(
        &router,
        &connection_token,
        "past the bound.bin",
        "application/octet-stream",
        max + 1,
        "size-past-bound",
    )
    .await;
    assert_eq!(
        (status, error_type(&body).as_str()),
        (400, "ValidationException"),
        "case \"one past the bound\": expected a refusal, got {body:?}"
    );

    // And zero is outside `AttachmentSizeInBytes`' own modelled range of 1..
    let (status, body) = start_raw(
        &router,
        &connection_token,
        "nothing.bin",
        "application/octet-stream",
        0,
        "size-zero",
    )
    .await;
    assert_eq!(
        (status, error_type(&body).as_str()),
        (400, "ValidationException"),
        "case \"zero bytes\": expected a refusal, got {body:?}"
    );
}

#[tokio::test]
async fn one_contact_holds_at_most_its_attachment_quota() {
    let router = router();
    let connection_token = connect(&router, "participant-attachment-quota").await;
    let quota = participant::state::MAX_ATTACHMENTS_PER_CONTACT;

    for index in 0..quota {
        let (status, body) = start_raw(
            &router,
            &connection_token,
            "note.txt",
            "text/plain",
            1,
            &format!("quota-{index}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"attachment {index} of the quota\": got {body:?}"
        );
    }
    let (status, body) = start_raw(
        &router,
        &connection_token,
        "note.txt",
        "text/plain",
        1,
        "quota-past",
    )
    .await;
    assert_eq!(
        (status, error_type(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"one past the quota\": expected a refusal, got {body:?}"
    );
}

#[tokio::test]
async fn a_content_type_that_is_not_a_media_type_is_refused() {
    let router = router();
    let connection_token = connect(&router, "participant-content-type").await;

    // Each of these satisfies `ContentType`'s modelled `len 1..255` and none of them
    // describes a MIME file type the upload target could declare: a header value carries no
    // control characters.
    for (case, content_type) in [
        ("a newline", "text/plain\nx: y"),
        ("a carriage return", "text/plain\r"),
        ("a NUL", "text/\0plain"),
    ] {
        let (status, body) = start_raw(
            &router,
            &connection_token,
            "note.txt",
            content_type,
            1,
            &format!("content-type-{case}"),
        )
        .await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the ContentType to be refused, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_custom_bot_participant_is_denied_the_attachments_of_its_own_chat() {
    let (router, engine) = router_and_engine();

    // `PARTICIPANT_ROLE_VALUES`: every role a session's participant can hold. A chat's human
    // and system participants read their attachments; `GetAttachment` documents CUSTOM_BOT as
    // "not permitted to access attachments customers may upload".
    for (index, (role, expected_status, expected_error)) in [
        ("CUSTOMER", 200, None),
        ("AGENT", 200, None),
        ("SUPERVISOR", 200, None),
        ("SYSTEM", 200, None),
        ("CUSTOM_BOT", 403, Some("AccessDeniedException")),
    ]
    .iter()
    .enumerate()
    {
        let participant_token = format!("participant-role-{index}");
        let connection_token = connect(&router, &participant_token).await;
        let started = start(
            &router,
            &connection_token,
            role,
            "note.txt",
            "text/plain",
            1,
            &format!("role-start-{index}"),
        )
        .await;
        let (status, body) = complete(
            &router,
            &connection_token,
            &started.attachment_id,
            &format!("role-complete-{index}"),
        )
        .await;
        assert_eq!(status, 200, "case {role:?}: confirmation got {body:?}");

        // The role is the session's, written by the component that started the chat.
        let contact_id = store(&engine)
            .lock()
            .participant_tokens
            .get(&participant_token)
            .cloned()
            .unwrap_or_else(|| panic!("case {role:?}: no session for {participant_token:?}"));
        store(&engine)
            .lock()
            .sessions
            .get_mut(&contact_id)
            .unwrap_or_else(|| panic!("case {role:?}: no session for {contact_id:?}"))
            .participant_role = (*role).to_string();

        let (status, body) =
            get_attachment(&router, &connection_token, &started.attachment_id, None).await;
        assert_eq!(
            status, *expected_status,
            "case {role:?}: expected HTTP {expected_status} from GetAttachment, got {body:?}"
        );
        match expected_error {
            Some(error) => assert_eq!(
                error_type(&body).as_str(),
                *error,
                "case {role:?}: expected {error}, got {body:?}"
            ),
            None => assert!(
                body["Url"].as_str().is_some(),
                "case {role:?}: expected a download URL, got {body:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_repeated_client_token_naming_another_attachment_is_judged_on_its_own() {
    let router = router();
    let connection_token = connect(&router, "participant-replay-scope").await;
    let first = start(
        &router,
        &connection_token,
        "the confirmed attachment",
        "note.txt",
        "text/plain",
        1,
        "scope-start-first",
    )
    .await;
    let second = start(
        &router,
        &connection_token,
        "the other attachment",
        "other.txt",
        "text/plain",
        1,
        "scope-start-second",
    )
    .await;

    let (status, body) = complete(&router, &connection_token, &first.attachment_id, "one-token").await;
    assert_eq!(status, 200, "the first confirmation got {body:?}");

    // The same client token, naming ANOTHER attachment: one token confirms one upload, so
    // this is its own request and the attachment it names is really confirmed — not answered
    // with the stored success of the first one.
    let (status, body) =
        complete(&router, &connection_token, &second.attachment_id, "one-token").await;
    assert_eq!(
        status, 200,
        "case \"another attachment under the same token\": got {body:?}"
    );
    let (status, body) =
        get_attachment(&router, &connection_token, &second.attachment_id, None).await;
    assert_eq!(
        status, 200,
        "case \"the other attachment after its own confirmation\": got {body:?}"
    );

    // And the first attachment is still confirmed, so confirming it again conflicts.
    let (status, body) = complete(&router, &connection_token, &first.attachment_id, "one-token").await;
    assert_eq!(
        (status, error_type(&body).as_str()),
        (409, "ConflictException"),
        "case \"the first attachment again\": got {body:?}"
    );
}

#[tokio::test]
async fn a_download_is_served_only_inside_the_window_that_was_announced_for_it() {
    let (router, engine) = router_and_engine();
    let connection_token = connect(&router, "participant-download-window-plane").await;
    let file = &FILES[0];
    let started = start(
        &router,
        &connection_token,
        file.case,
        file.name,
        file.content_type,
        file.size,
        "window-plane-start",
    )
    .await;
    let uploaded = fetch(&router, "PUT", &started.url, &started.headers, file.bytes()).await;
    assert_eq!(
        uploaded.status,
        200,
        "the upload got {:?}",
        uploaded.json()
    );
    let (status, body) = complete(
        &router,
        &connection_token,
        &started.attachment_id,
        "window-plane-complete",
    )
    .await;
    assert_eq!(status, 200, "CompleteAttachmentUpload got {body:?}");

    // Inside the window `GetAttachment` announced, the file is served.
    let (status, body) =
        get_attachment(&router, &connection_token, &started.attachment_id, Some(300)).await;
    assert_eq!(status, 200, "GetAttachment got {body:?}");
    let download_url = body["Url"]
        .as_str()
        .unwrap_or_else(|| panic!("no Url in {body:?}"))
        .to_string();
    let served = fetch(&router, "GET", &download_url, &[], Vec::new()).await;
    assert_eq!(
        (served.status, served.body.len()),
        (200, file.size),
        "case \"inside the announced window\": got {:?}",
        served.json()
    );

    // Past it, the same URL is refused the way a pre-signed URL whose signature has expired
    // is. The window is moved into the past in the store rather than waited out, so the case
    // is decided by the rule and not by how long the test takes.
    store(&engine)
        .lock()
        .attachments
        .get_mut(&started.attachment_id)
        .unwrap_or_else(|| panic!("no attachment {:?}", started.attachment_id))
        .download_url_expiry = Some(std::time::SystemTime::UNIX_EPOCH);
    let expired = fetch(&router, "GET", &download_url, &[], Vec::new()).await;
    assert_eq!(
        (
            expired.status,
            expired.json()["code"].as_str(),
            expired.json()["message"].as_str()
        ),
        (403, Some("AccessDenied"), Some("Request has expired")),
        "case \"past the announced window\": got {:?}",
        expired.json()
    );

    // And an attachment confirmed with nothing ever uploaded to its target names no object,
    // inside its announced window or not: no bytes are invented for it.
    let empty = start(
        &router,
        &connection_token,
        "a confirmation with no upload",
        "empty.txt",
        "text/plain",
        1,
        "window-plane-empty-start",
    )
    .await;
    let (status, body) = complete(
        &router,
        &connection_token,
        &empty.attachment_id,
        "window-plane-empty-complete",
    )
    .await;
    assert_eq!(status, 200, "CompleteAttachmentUpload got {body:?}");
    let (status, body) =
        get_attachment(&router, &connection_token, &empty.attachment_id, None).await;
    assert_eq!(status, 200, "GetAttachment got {body:?}");
    let url = body["Url"]
        .as_str()
        .unwrap_or_else(|| panic!("no Url in {body:?}"))
        .to_string();
    let nothing = fetch(&router, "GET", &url, &[], Vec::new()).await;
    assert_eq!(
        (nothing.status, nothing.json()["code"].as_str()),
        (404, Some("NoSuchKey")),
        "case \"a confirmed attachment with no file\": got {:?}",
        nothing.json()
    );
}

#[tokio::test]
async fn the_published_target_takes_only_the_upload_it_was_minted_for() {
    let router = router();
    let connection_token = connect(&router, "participant-upload-plane").await;
    let file = &FILES[1];

    let started = start(
        &router,
        &connection_token,
        file.case,
        file.name,
        file.content_type,
        file.size,
        "plane-start",
    )
    .await;

    // No download URL has been reported for this attachment, so no request carries a window
    // for it: the only call that announces one is `GetAttachment`.
    let unannounced = fetch(&router, "GET", &started.url, &[], Vec::new()).await;
    assert_eq!(
        (unannounced.status, unannounced.json()["code"].as_str()),
        (403, Some("AccessDenied")),
        "case \"a download no URL was reported for\": got {:?}",
        unannounced.json()
    );

    // A body of another length than the attachment declared is refused on the declared
    // length and on the length that arrived.
    for (case, body, expected_code) in [
        ("one byte short", file.bytes()[1..].to_vec(), "IncompleteBody"),
        (
            "one byte long",
            [file.bytes(), vec![file.fill]].concat(),
            "EntityTooLarge",
        ),
    ] {
        let headers: Vec<(String, String)> = vec![
            ("content-type".to_string(), file.content_type.to_string()),
            ("content-length".to_string(), body.len().to_string()),
        ];
        let refused = fetch(&router, "PUT", &started.url, &headers, body).await;
        assert_eq!(
            (refused.status, refused.json()["code"].as_str()),
            (400, Some(expected_code)),
            "case {case:?}: got {:?}",
            refused.json()
        );
    }

    // An upload declaring no length at all is refused rather than taken on trust.
    let undeclared = fetch(
        &router,
        "PUT",
        &started.url,
        &[("content-type".to_string(), file.content_type.to_string())],
        file.bytes(),
    )
    .await;
    assert_eq!(
        (undeclared.status, undeclared.json()["code"].as_str()),
        (411, Some("MissingContentLength")),
        "case \"no declared length\": got {:?}",
        undeclared.json()
    );

    // A method neither published URL describes is refused as the method it is.
    let posted = fetch(&router, "POST", &started.url, &[], Vec::new()).await;
    assert_eq!(
        (posted.status, posted.json()["code"].as_str()),
        (405, Some("MethodNotAllowed")),
        "case \"a method the target does not describe\": got {:?}",
        posted.json()
    );

    // The upload itself, then a second one after it was confirmed: the target described one
    // upload, and the confirmation closed it.
    let stored = fetch(
        &router,
        "PUT",
        &started.url,
        &started.headers,
        file.bytes(),
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "case \"the upload\": got {:?}",
        stored.json()
    );
    let (status, body) = complete(
        &router,
        &connection_token,
        &started.attachment_id,
        "plane-complete",
    )
    .await;
    assert_eq!(status, 200, "CompleteAttachmentUpload got {body:?}");
    let after = fetch(
        &router,
        "PUT",
        &started.url,
        &started.headers,
        file.bytes(),
    )
    .await;
    assert_eq!(
        (after.status, after.json()["code"].as_str()),
        (403, Some("AccessDenied")),
        "case \"an upload after the confirmation\": got {:?}",
        after.json()
    );

    // And a path naming an attachment of nobody's chat names no object.
    let elsewhere = started.url.replace(&started.attachment_id, "00000000-0000-4000-8000-000000000000");
    let unknown = fetch(&router, "GET", &elsewhere, &[], Vec::new()).await;
    assert_eq!(
        (unknown.status, unknown.json()["code"].as_str()),
        (404, Some("NoSuchKey")),
        "case \"an attachment nobody minted\": got {:?}",
        unknown.json()
    );
}

/// One attachment call: the modelled path and a body satisfying every modelled constraint,
/// so the only thing a refusal of it can be about is the rule under test.
struct AttachmentCall {
    case: &'static str,
    path: &'static str,
    body: Value,
}

/// The three attachment operations of one chat, each with a request a sharing chat
/// answers. `attachment_id` is an id of the form this service mints, so a refusal cannot
/// be the "no such attachment" validation either.
fn attachment_calls(attachment_id: &str) -> [AttachmentCall; 3] {
    [
        AttachmentCall {
            case: "StartAttachmentUpload",
            path: "/participant/start-attachment-upload",
            body: json!({
                "AttachmentName": "notes.txt",
                "AttachmentSizeInBytes": 11,
                "ContentType": "text/plain",
                "ClientToken": "sharing-start",
            }),
        },
        AttachmentCall {
            case: "CompleteAttachmentUpload",
            path: "/participant/complete-attachment-upload",
            body: json!({"AttachmentIds": [attachment_id], "ClientToken": "sharing-complete"}),
        },
        AttachmentCall {
            case: "GetAttachment",
            path: "/participant/attachment",
            body: json!({"AttachmentId": attachment_id}),
        },
    ]
}

/// Assert `(status, error_type, body)` is this service's access-denied refusal: HTTP 403,
/// the modelled error shape named in the `x-amzn-errortype` header and in the body, and
/// the fixed access-denied text under the wire `message` key.
fn assert_access_denied(case: &str, status: u16, header_error_type: &str, body: &Value) {
    assert_eq!(
        status, 403,
        "case {case:?}: expected HTTP 403, got {status} with body {body:?}"
    );
    assert_eq!(
        header_error_type, "AccessDeniedException",
        "case {case:?}: expected the x-amzn-errortype header to name AccessDeniedException, \
         got {header_error_type:?}"
    );
    assert_eq!(
        error_type(body),
        "AccessDeniedException",
        "case {case:?}: expected the body to name AccessDeniedException, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(participant::state::ACCESS_DENIED_MESSAGE),
        "case {case:?}: expected the message {:?}, got {body:?}",
        participant::state::ACCESS_DENIED_MESSAGE
    );
}

#[tokio::test]
async fn a_chat_of_an_instance_with_no_attachment_storage_shares_no_files() {
    // The rule: a chat file is kept in the storage the Amazon Connect instance the contact
    // belongs to is configured with, so a participant of a chat whose instance is
    // configured with none shares no files — every attachment operation is refused the
    // modelled `AccessDeniedException` — while a participant of a chat whose instance IS
    // configured for it is served. Two chats in one store, differing in that alone: the
    // connection token is not what the rows differ in, since both authenticate a
    // connected participant of a live chat.
    let harness = router();
    let denied_contact = harness.start_chat_sharing("denied-participant-token", false);
    let denied_connection = connect_sharing(&harness, "denied-participant-token", false).await;
    let served_connection = connect_sharing(&harness, "served-participant-token", true).await;

    let unknown_attachment = "00000000-0000-4000-8000-000000000000";
    let calls = attachment_calls(unknown_attachment);
    for call in &calls {
        let (status, error_type, body) =
            post_answer(&harness, call.path, Some(&denied_connection), &call.body).await;
        assert_access_denied(
            &format!("{} for a chat of an instance with no attachment storage", call.case),
            status,
            &error_type,
            &body,
        );
    }
    assert_eq!(
        harness.attachments_held(&denied_contact),
        0,
        "case \"the refused chat\": expected the refusals to mint no attachment, got {} held",
        harness.attachments_held(&denied_contact)
    );

    // The same upload, from the chat whose instance IS configured for attachments, is
    // served: the rule is the instance's configuration, not the request.
    let (status, _, body) = post_answer(
        &harness,
        "/participant/start-attachment-upload",
        Some(&served_connection),
        &calls[0].body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"StartAttachmentUpload for a chat of a configured instance\": got {body:?}"
    );
    assert!(
        body["AttachmentId"].as_str().is_some_and(|id| !id.is_empty()),
        "case \"StartAttachmentUpload for a chat of a configured instance\": expected an \
         AttachmentId, got {body:?}"
    );
}

#[tokio::test]
async fn an_instance_that_stops_sharing_attachments_stops_serving_its_chats_attachments() {
    // What the attachment operations serve is what the Amazon Connect instance is
    // configured with as it stands, not a verdict the chat was started with: the same
    // live chat, with the same connection token and the same stored attachment, is
    // served, then refused, then served again as its instance is configured, unconfigured
    // and configured anew — each published onto this chat's session the way the
    // association does.
    let harness = router();
    let contact_id = harness.start_chat_sharing("revoked-participant-token", true);
    let connection = connect_sharing(&harness, "revoked-participant-token", true).await;

    let started = start(
        &harness,
        &connection,
        "a chat of a configured instance",
        "notes.txt",
        "text/plain",
        11,
        "revoked-start",
    )
    .await;
    let (status, body) = complete(
        &harness,
        &connection,
        &started.attachment_id,
        "revoked-complete",
    )
    .await;
    assert_eq!(status, 200, "CompleteAttachmentUpload got {body:?}");

    harness.set_attachment_sharing(&contact_id, false);
    let calls = attachment_calls(&started.attachment_id);
    for call in &calls {
        let (status, error_type, body) =
            post_answer(&harness, call.path, Some(&connection), &call.body).await;
        assert_access_denied(
            &format!("{} after the instance stopped sharing attachments", call.case),
            status,
            &error_type,
            &body,
        );
    }

    harness.set_attachment_sharing(&contact_id, true);
    let (status, body) = get_attachment(&harness, &connection, &started.attachment_id, None).await;
    assert_eq!(
        status, 200,
        "case \"the stored attachment once its instance shares attachments again\": got {body:?}"
    );
    assert_eq!(
        body["AttachmentSizeInBytes"].as_u64(),
        Some(11),
        "case \"the stored attachment once its instance shares attachments again\": expected \
         the declared size 11, got {body:?}"
    );
}

/// Every `ContentDisposition` a download may be asked for, with the `Content-Disposition` the
/// file is then served under, plus the absent member — which the model documents as
/// downloading the file. The values are `CONTENT_DISPOSITION_VALUES`, every value the member
/// declares.
const DISPOSITIONS: &[(&str, Option<&str>, &str)] = &[
    ("no disposition asked for", None, "attachment"),
    ("the downloading disposition", Some("ATTACHMENT"), "attachment"),
    ("the displaying disposition", Some("INLINE"), "inline"),
];

#[tokio::test]
async fn a_download_presents_the_file_the_way_the_request_asked_for() {
    // The rule: the pre-signed download URL `GetAttachment` reports carries the disposition
    // the request's `UrlContentDisposition` names — downloading the file when it names none —
    // and the request that follows that URL is served the file under exactly that
    // `Content-Disposition`, with its bytes and its media type unchanged.
    let router = router();
    let connection_token = connect(&router, "participant-content-disposition").await;

    // Driven over the whole file table as well as the disposition: how a file is presented is
    // independent of its media type, its name and its size.
    for (index, file) in FILES.iter().enumerate() {
        let started = stored_attachment(
            &router,
            &connection_token,
            file,
            &format!("disposition-{index}"),
        )
        .await;

        for (case, requested, expected) in DISPOSITIONS {
            let case = format!("{} / {case}", file.case);
            let (status, body) = get_attachment_presented(
                &router,
                &connection_token,
                &started.attachment_id,
                *requested,
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: GetAttachment got {body:?}");
            let url = body["Url"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Url in {body:?}"))
                .to_string();
            assert_eq!(
                query_param(&url, "response-content-disposition").as_deref(),
                Some(*expected),
                "case {case:?}: expected the published URL to carry the disposition \
                 {expected:?}, got {url:?}"
            );

            // A `HEAD` reports the same presentation as the `GET` that carries the bytes.
            for method in ["GET", "HEAD"] {
                let fetched = fetch(&router, method, &url, &[], Vec::new()).await;
                assert_eq!(
                    fetched.status,
                    200,
                    "case {case:?}: {method} of the published URL got {:?}",
                    fetched.json()
                );
                assert_eq!(
                    fetched.content_disposition.as_deref(),
                    Some(*expected),
                    "case {case:?}: expected {method} to serve the file as {expected:?}, got {:?}",
                    fetched.content_disposition
                );
                assert_eq!(
                    fetched.content_type.as_deref(),
                    Some(file.content_type),
                    "case {case:?}: expected the stored media type {:?} from {method}, got {:?}",
                    file.content_type,
                    fetched.content_type
                );
                let expected_body = if method == "GET" {
                    file.bytes()
                } else {
                    Vec::new()
                };
                assert_eq!(
                    fetched.body.len(),
                    expected_body.len(),
                    "case {case:?}: expected {} bytes from {method}, got {}",
                    expected_body.len(),
                    fetched.body.len()
                );
                assert!(
                    fetched.body == expected_body,
                    "case {case:?}: the bytes {method} served are not the bytes that were \
                     uploaded"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_disposition_the_member_does_not_declare_is_refused() {
    // The same rule from the other side: only a value `ContentDisposition` declares names a
    // presentation, so anything else is refused — the modelled value set of the member — and
    // no URL is published for it. Each of these satisfies the member's own shape (a string)
    // and is not one of its two values.
    let router = router();
    let connection_token = connect(&router, "participant-content-disposition-refusal").await;
    let started = stored_attachment(
        &router,
        &connection_token,
        &FILES[0],
        "disposition-refusal",
    )
    .await;

    for requested in [
        "inline",
        "Attachment",
        "DOWNLOAD",
        "ATTACHMENT ",
        "attachment; filename=\"note.txt\"",
        "",
    ] {
        let (status, body) = get_attachment_presented(
            &router,
            &connection_token,
            &started.attachment_id,
            Some(requested),
        )
        .await;
        assert_eq!(
            (status, error_type(&body).as_str()),
            (400, "ValidationException"),
            "case {requested:?}: expected GetAttachment to refuse a disposition the member \
             does not declare, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("urlContentDisposition") && message.contains("enum value set"),
            "case {requested:?}: expected the refusal to report the member's value set, got \
             {message:?}"
        );
    }
}

/// The base URL the authenticating harness below is reached at, so every URL this service
/// publishes there leads back to it.
const BASE_URL: &str = "https://participant.connect.us-east-1.amazonaws.com";

/// The Region every signed call of that harness is signed for, the one [`HOST`] names.
const REGION: &str = "us-east-1";

/// This service's SigV4 signing name, which the emulator verifies a request to it under.
const SIGNING_NAME: &str = "execute-api";

/// The query field a pre-signed URL carries its own signature in.
const SIGNATURE_FIELD: &str = "X-Amz-Signature";

/// A harness that VERIFIES every request signature — the `--auth on` path a developer runs the
/// emulator on — so a request to a published attachment URL is held to the credential that
/// URL carries.
fn authenticating_harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.set_url_base(
        smithy_server_runtime::EndpointUrl::parse(BASE_URL)
            .expect("the harness base URL must parse"),
    );
    router.register_service(participant::EmulatedService::new(engine.clone()));
    Harness { router, engine }
}

/// One modelled call, SigV4-signed with the keys this emulator process publishes — what an
/// authenticating router verifies a request against.
async fn signed_post(
    router: &MultiServiceRouter,
    path: &str,
    bearer: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("{BASE_URL}{path}"))
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("x-amz-bearer", bearer)
        .body(serde_json::to_vec(body).expect("serialize the request body"))
        .expect("build the request");
    let keys = ::emulator_server::request_authentication::SigV4Authenticator::for_this_process();
    ::service_common::in_process_sigv4::sign_in_process_request_as(
        &mut request,
        SIGNING_NAME,
        REGION,
        &keys.access_key_id,
        &keys.secret_access_key,
    )
    .await
    .expect("the test request must sign");
    let (parts, body) = request.into_parts();
    let response = router
        .dispatch(http::Request::from_parts(parts, Bytes::from(body)))
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// The request target of `url` — its path with the query behind it — which is what a caller
/// following a published URL sends.
fn target_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.find('/')
        .map_or_else(|| "/".to_string(), |index| rest[index..].to_string())
}

/// `url` with its whole query string dropped: the path alone, which is no credential.
fn without_query(url: &str) -> String {
    format!(
        "{BASE_URL}{}",
        target_of(url)
            .split_once('?')
            .map_or_else(|| target_of(url), |(path, _)| path.to_string())
    )
}

/// `url` with query field `name` removed altogether, every other pair left byte-identical.
fn without_field(url: &str, name: &str) -> String {
    let target = target_of(url);
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| pair.split_once('=').map(|(field, _)| field) != Some(name))
        .collect();
    format!("{BASE_URL}{path}?{}", kept.join("&"))
}

/// `url` with the first digit of its signature rewritten: still a signature of the same form,
/// and not this request's.
fn with_tampered_signature(url: &str) -> String {
    let target = target_of(url);
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((field, value)) if field == SIGNATURE_FIELD => {
                let rewritten: String = value
                    .chars()
                    .enumerate()
                    .map(|(index, digit)| match (index, digit) {
                        (0, '0') => '1',
                        (0, _) => '0',
                        (_, kept) => kept,
                    })
                    .collect();
                format!("{field}={rewritten}")
            }
            _ => pair.to_string(),
        })
        .collect();
    format!("{BASE_URL}{path}?{}", pairs.join("&"))
}

#[tokio::test]
async fn a_published_attachment_url_is_served_only_to_the_signature_it_carries() {
    // The rule: an address an operation hands the caller to follow later is a PRE-SIGNED one
    // — its credential travels in the query string rather than in an `Authorization` header —
    // so the plane serving it declares no scheme of its own and the emulator holds a request
    // to that signature: the address exactly as published serves the file, and one whose
    // signature is absent or does not verify over the request presented serves nothing. A
    // path alone is never a credential, however unguessable it looks.
    let harness = authenticating_harness();
    let participant_token = "participant-presigned-plane";
    harness.start_chat(participant_token);
    let (status, body) = signed_post(
        &harness,
        "/participant/connection",
        participant_token,
        &json!({"Type": ["CONNECTION_CREDENTIALS"]}),
    )
    .await;
    assert_eq!(status, 200, "CreateParticipantConnection got {body:?}");
    let connection_token = body["ConnectionCredentials"]["ConnectionToken"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionToken in {body:?}"))
        .to_string();

    let file = &FILES[1];
    let (status, body) = signed_post(
        &harness,
        "/participant/start-attachment-upload",
        &connection_token,
        &json!({
            "AttachmentName": file.name,
            "AttachmentSizeInBytes": file.size as i64,
            "ContentType": file.content_type,
            "ClientToken": "presigned-plane",
        }),
    )
    .await;
    assert_eq!(status, 200, "StartAttachmentUpload got {body:?}");
    let attachment_id = body["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {body:?}"))
        .to_string();
    let upload_url = body["UploadMetadata"]["Url"]
        .as_str()
        .unwrap_or_else(|| panic!("no UploadMetadata.Url in {body:?}"))
        .to_string();
    let upload_headers: Vec<(String, String)> = body["UploadMetadata"]["HeadersToInclude"]
        .as_object()
        .unwrap_or_else(|| panic!("no HeadersToInclude in {body:?}"))
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        upload_url.contains(SIGNATURE_FIELD),
        "the published upload target must be pre-signed, got {upload_url:?}"
    );

    // An upload presented at an address whose signature is absent or does not verify stores
    // nothing: the rows differ along HOW the presented address differs from the published one.
    // An address carrying NO credential at all names no service either — the router resolves
    // which service serves a request from the credential it presents — so that row is answered
    // by the router, ahead of this service, and the others by the signature check.
    for (case, address, expected) in [
        ("the whole query dropped", without_query(&upload_url), 404),
        (
            "the signature field dropped",
            without_field(&upload_url, SIGNATURE_FIELD),
            403,
        ),
        (
            "one digit of the signature rewritten",
            with_tampered_signature(&upload_url),
            403,
        ),
    ] {
        let refused = fetch(&harness, "PUT", &address, &upload_headers, file.bytes()).await;
        assert_eq!(
            refused.status, expected,
            "case {case:?}: expected the upload to be refused with HTTP {expected}, got {:?}",
            refused.json()
        );
    }

    // The address exactly as published is the one that stores the file.
    let stored = fetch(
        &harness,
        "PUT",
        &upload_url,
        &upload_headers,
        file.bytes(),
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "the published upload target must take the file, got {:?}",
        stored.json()
    );

    let (status, body) = signed_post(
        &harness,
        "/participant/complete-attachment-upload",
        &connection_token,
        &json!({"AttachmentIds": [attachment_id], "ClientToken": "presigned-plane-complete"}),
    )
    .await;
    assert_eq!(status, 200, "CompleteAttachmentUpload got {body:?}");

    let (status, body) = signed_post(
        &harness,
        "/participant/attachment",
        &connection_token,
        &json!({"AttachmentId": attachment_id, "UrlExpiryInSeconds": 300}),
    )
    .await;
    assert_eq!(status, 200, "GetAttachment got {body:?}");
    let download_url = body["Url"]
        .as_str()
        .unwrap_or_else(|| panic!("no Url in {body:?}"))
        .to_string();

    // The same three rows for the download, each of which must serve no stored byte.
    for (case, address, expected) in [
        ("the whole query dropped", without_query(&download_url), 404),
        (
            "the signature field dropped",
            without_field(&download_url, SIGNATURE_FIELD),
            403,
        ),
        (
            "one digit of the signature rewritten",
            with_tampered_signature(&download_url),
            403,
        ),
    ] {
        let refused = fetch(&harness, "GET", &address, &[], Vec::new()).await;
        assert_eq!(
            refused.status, expected,
            "case {case:?}: expected the download to be refused with HTTP {expected}, got {:?}",
            refused.json()
        );
        assert!(
            refused.body != file.bytes(),
            "case {case:?}: a refused download must serve no stored byte"
        );
    }

    // And the published address itself serves the file that was uploaded.
    let served = fetch(&harness, "GET", &download_url, &[], Vec::new()).await;
    assert_eq!(
        served.status,
        200,
        "the published download URL must serve the file, got {:?}",
        served.json()
    );
    assert!(
        served.body == file.bytes(),
        "the published download URL must serve the {} uploaded bytes, got {}",
        file.size,
        served.body.len()
    );
}
