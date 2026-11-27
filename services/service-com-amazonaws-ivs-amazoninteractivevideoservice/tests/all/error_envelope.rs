// SPDX-License-Identifier: CC0-1.0

//! The envelope every IVS response carries, driven end-to-end over the router the
//! production entrypoint serves.
//!
//! Three rules:
//!
//! 1. A refused call answers a JSON error document stating the error's code under `Code`,
//!    its text under `Message` and the id of the request it answers under `RequestID`,
//!    with the same code in `x-amzn-errortype`, the same text in `x-amzn-errormessage` and
//!    under the `message` key an SDK error parser reads, and the same id in
//!    `x-amzn-requestid` — whatever the error CLASS and whichever channel the failure
//!    travels (a constraint refused at the request boundary, a modelled failure a handler
//!    states, a code the operation's model does not declare, or the scaffold's 501).
//! 2. Every response — a success included — names the request it answers, and no two
//!    responses name the same one.
//! 3. A refusal's dotted member path names each step the way the model names the value at
//!    it, and names an element of a list by its ZERO-based position: the element's own
//!    shape where the model gives the element a shape, the list member's own name where it
//!    does not, and never the synthetic `member` the Smithy list shape names its member by.
//!
//! Each table's rows differ along what its rule generalises over (the operation, the error
//! class, the member, the position in the list, the kind of list), never by renaming one
//! case.
//!
//! The `GetStreamSession` rows carry the refusal for a `streamId` that is not an id this
//! deployment issued; `live_stream_reads.rs` is where that rule is asserted over the whole
//! class of stream ids, against the sessions this store really holds.

use std::collections::BTreeMap;
use std::sync::Arc;

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ED25519};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService;

/// The region this suite signs for, and the account every emulator ARN carries.
const REGION: &str = "us-west-2";
const ACCOUNT: &str = "000000000000";

/// The three members of the error document a client of this service reads a failure out
/// of, and the three response headers carrying the same three facts.
const CODE_KEY: &str = "Code";
const MESSAGE_KEY: &str = "Message";
const REQUEST_ID_KEY: &str = "RequestID";
const SDK_MESSAGE_KEY: &str = "message";
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";
const ERROR_MESSAGE_HEADER: &str = "x-amzn-errormessage";
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One response, as a client sees it.
struct Answer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Value,
}

impl Answer {
    /// A body member as text, or `"<absent>"` so a failing assertion says which member
    /// was missing rather than panicking inside the reader.
    fn member(&self, key: &str) -> &str {
        self.body
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
    }

    /// A response header, or `"<absent>"`.
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("<absent>", String::as_str)
    }
}

/// One emulator holding only this service, driven through the router that serves
/// production traffic.
struct Ivs {
    router: MultiServiceRouter,
}

impl Ivs {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// One request over the wire, signed with a credential scope naming `ivs` in
    /// [`REGION`] as an SDK would.
    async fn call(&self, method: &str, path: &str, body: Option<&str>) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivs.{REGION}.amazonaws.com{path}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.unwrap_or_default().to_owned()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers: BTreeMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
        };
        Answer {
            status,
            headers,
            body,
        }
    }

    /// One of the operations the model binds to `POST /<Operation>`.
    async fn post(&self, operation: &str, body: &Value) -> Answer {
        self.call("POST", &format!("/{operation}"), Some(&body.to_string()))
            .await
    }

    /// Create a channel and return the ARN the service minted for it.
    async fn create_channel(&self, body: &Value) -> String {
        let answer = self.post("CreateChannel", body).await;
        assert_eq!(
            answer.status, 200,
            "CreateChannel {body}: expected 200, got {} {}",
            answer.status, answer.body
        );
        answer.body["channel"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A well-formed ARN of `kind` naming a resource this store never minted.
fn absent_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{kind}/{id}")
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label, so a
/// whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A freshly generated public key in the `PUBLIC KEY` PEM block an import carries.
///
/// Generated rather than hard-coded, so what the suite imports is a real public key.
fn public_key_pem(algorithm: &'static rcgen::SignatureAlgorithm) -> String {
    let der = KeyPair::generate_for(algorithm)
        .expect("the test key pair is generated")
        .public_key_der();
    pem::encode(&pem::Pem::new("PUBLIC KEY", der))
}

/// The service's one not-found sentence.
fn not_found(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// The service's one sentence for an operation on a channel that is not ingesting.
fn offline(channel_arn: &str) -> String {
    format!("Channel: {channel_arn} is not currently online")
}

/// The sentence a `@pattern` refusal states.
fn pattern_refusal(operation: &str, path: &str, pattern: &str) -> String {
    format!("{operation}.{path} does not match the required pattern {pattern}")
}

/// The service's one sentence for a `streamId` that is not a stream id it ever issued,
/// read from the one place this crate's suites state it (`live_stream_reads.rs`, where the
/// rule itself is asserted), so the two cannot disagree about the text.
fn invalid_stream_id() -> String {
    crate::live_stream_reads::INVALID_STREAM_ID.to_string()
}

/// A value satisfying the modelled `StreamId` (`@length 26..26`, `@pattern
/// ^st-[a-zA-Z0-9]+$`) that no stream of this emulator carries — the same reader the rule's
/// own suite builds its rows with.
fn unissued_stream_id(body: &str) -> String {
    crate::live_stream_reads::unissued_stream_id(body)
}

/// Whether `id` takes the fixed-width base64 token form every response of this endpoint
/// names the request it answers by.
///
/// The form itself is read by `request_id_token_form`'s `token_form_violation`, the one
/// place this crate's suites state it, so this reader and that suite cannot disagree about
/// what the wire contract is.
pub(crate) fn is_request_id_shaped(id: &str) -> bool {
    crate::request_id_token_form::token_form_violation(id).is_none()
}

/// Rule 1 over ONE refusal: the document's three members, the three headers carrying the
/// same three facts, and the `message` key an SDK error parser reads.
///
/// `expected_message` is `None` for a failure whose text no message rule of this service
/// fixes (the scaffold's 501), where the requirement is only that a text is stated.
fn assert_stated_failure(
    case: &str,
    answer: &Answer,
    expected_status: u16,
    expected_code: &str,
    expected_message: Option<&str>,
) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {} {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.member(CODE_KEY),
        expected_code,
        "case {case:?}: expected the document's {CODE_KEY} to be {expected_code:?}, got {}",
        answer.body
    );
    match expected_message {
        Some(expected) => assert_eq!(
            answer.member(MESSAGE_KEY),
            expected,
            "case {case:?}: expected the document's {MESSAGE_KEY} to follow the service's \
             message rule, got {}",
            answer.body
        ),
        None => assert!(
            answer.member(MESSAGE_KEY) != "<absent>" && !answer.member(MESSAGE_KEY).is_empty(),
            "case {case:?}: expected a stated {MESSAGE_KEY}, got {}",
            answer.body
        ),
    }
    // The text an SDK error parser reads is the same text, not a second rendering of it.
    assert_eq!(
        answer.member(SDK_MESSAGE_KEY),
        answer.member(MESSAGE_KEY),
        "case {case:?}: expected the SDK {SDK_MESSAGE_KEY} key to carry the stated text, got {}",
        answer.body
    );
    let request_id = answer.member(REQUEST_ID_KEY);
    assert!(
        is_request_id_shaped(request_id),
        "case {case:?}: expected the document's {REQUEST_ID_KEY} to take the \
         16-character base64 token form, got {request_id:?} in {}",
        answer.body
    );
    assert_eq!(
        answer.header(ERROR_TYPE_HEADER),
        expected_code,
        "case {case:?}: expected {ERROR_TYPE_HEADER} to name the error, got {:?}",
        answer.headers
    );
    assert_eq!(
        answer.header(ERROR_MESSAGE_HEADER),
        answer.member(MESSAGE_KEY),
        "case {case:?}: expected {ERROR_MESSAGE_HEADER} to carry the stated text, got {:?}",
        answer.headers
    );
    assert_eq!(
        answer.header(REQUEST_ID_HEADER),
        request_id,
        "case {case:?}: expected {REQUEST_ID_HEADER} to name the same request as the \
         document, got {:?}",
        answer.headers
    );
    // The model's own detail member is not what a client reads: the text travels under
    // the keys above, so the modelled `exceptionMessage` is nowhere in the body.
    assert!(
        answer.body.get("exceptionMessage").is_none(),
        "case {case:?}: the text travels under the document's keys, not the model's own \
         member name, got {}",
        answer.body
    );
}

/// One row of rule 1's table: the call to make, and the envelope the service must answer
/// it with — the status, the code, and the text its message rule fixes (`None` where no
/// rule of this service fixes one).
type Refusal = (
    &'static str,
    &'static str,
    Value,
    u16,
    &'static str,
    Option<String>,
);

/// Rule 1: every class of failure this service answers states its code, its text and the
/// request it answers — one row per (operation, error class) pair the service can reach.
#[tokio::test]
async fn every_refused_call_states_its_code_its_text_and_the_request_it_answers() {
    let ivs = Ivs::new();
    // A channel that exists and is idle; creating it also mints its one stream key, so a
    // second `CreateStreamKey` meets the per-channel quota.
    let idle = ivs.create_channel(&json!({ "name": "envelope" })).await;
    let absent_channel = absent_arn("channel", "neverCreated");
    let absent_key_pair = absent_arn("playback-key", "neverImported");
    // An imported key, so importing the same material again is a conflict.
    let imported = public_key_pem(&PKCS_ECDSA_P256_SHA256);
    let import = ivs
        .post(
            "ImportPlaybackKeyPair",
            &json!({ "publicKeyMaterial": imported }),
        )
        .await;
    assert_eq!(
        import.status, 200,
        "the fixture key pair must import, got {} {}",
        import.status, import.body
    );
    let not_a_pem = "not a pem at all";
    let wrong_algorithm = public_key_pem(&PKCS_ED25519);
    let unusable_material =
        "Request member: publicKeyMaterial failed to satisfy the constraints: could not \
         decode as a PEM file";

    let cases: Vec<Refusal> = vec![
        (
            "a constraint refused at the request boundary",
            "CreateChannel",
            json!({ "name": "not a legal name!" }),
            400,
            "ValidationException",
            Some(pattern_refusal(
                "CreateChannel",
                "name",
                "^[a-zA-Z0-9-_]*$",
            )),
        ),
        (
            "a member the handler refuses: material that is no PEM block",
            "ImportPlaybackKeyPair",
            json!({ "publicKeyMaterial": not_a_pem }),
            400,
            "ValidationException",
            Some(unusable_material.to_string()),
        ),
        (
            "a member the handler refuses: a PEM block holding another algorithm's key",
            "ImportPlaybackKeyPair",
            json!({ "publicKeyMaterial": wrong_algorithm }),
            400,
            "ValidationException",
            Some(unusable_material.to_string()),
        ),
        (
            "a quota: a second stream key on a channel at its one-key limit",
            "CreateStreamKey",
            json!({ "channelArn": idle }),
            402,
            "ServiceQuotaExceededException",
            Some("Resource: stream-key quota exceeded limit: 1".to_string()),
        ),
        (
            "an offline channel, read through GetStream",
            "GetStream",
            json!({ "channelArn": idle }),
            404,
            "ChannelNotBroadcasting",
            Some(offline(&idle)),
        ),
        (
            "an offline channel, acted on through StopStream",
            "StopStream",
            json!({ "channelArn": idle }),
            404,
            "ChannelNotBroadcasting",
            Some(offline(&idle)),
        ),
        (
            "an offline channel, acted on through PutMetadata",
            "PutMetadata",
            json!({ "channelArn": idle, "metadata": "cue" }),
            404,
            "ChannelNotBroadcasting",
            Some(offline(&idle)),
        ),
        (
            "a stream id this deployment never issued, read through GetStreamSession",
            "GetStreamSession",
            json!({ "channelArn": idle,
                    "streamId": unissued_stream_id("00000000000000000000000") }),
            400,
            "ValidationException",
            Some(invalid_stream_id()),
        ),
        (
            "a stream id outside the alphabet this service's ids are written in",
            "GetStreamSession",
            json!({ "channelArn": idle,
                    "streamId": unissued_stream_id("notARealStreamIdValue01") }),
            400,
            "ValidationException",
            Some(invalid_stream_id()),
        ),
        (
            "an unissued stream id on a channel that was never created: the id is judged \
             first",
            "GetStreamSession",
            json!({ "channelArn": absent_channel,
                    "streamId": unissued_stream_id("0123456789abcdef0123456") }),
            400,
            "ValidationException",
            Some(invalid_stream_id()),
        ),
        (
            "a channel that was never created, named by StartViewerSessionRevocation",
            "StartViewerSessionRevocation",
            json!({ "channelArn": absent_channel, "viewerId": "viewer-1" }),
            404,
            "ResourceNotFoundException",
            Some(not_found(&absent_channel)),
        ),
        (
            "a channel that was never created, read through GetChannel",
            "GetChannel",
            json!({ "arn": absent_channel }),
            404,
            "ResourceNotFoundException",
            Some(not_found(&absent_channel)),
        ),
        (
            "a key pair that was never imported, read through GetPlaybackKeyPair",
            "GetPlaybackKeyPair",
            json!({ "arn": absent_key_pair }),
            404,
            "ResourceNotFoundException",
            Some(not_found(&absent_key_pair)),
        ),
        (
            "material this service already holds, imported again",
            "ImportPlaybackKeyPair",
            json!({ "publicKeyMaterial": imported }),
            409,
            "ConflictException",
            Some(
                "A playback-key resource already exists with the given publicKeyMaterial"
                    .to_string(),
            ),
        ),
        (
            "an operation that is still scaffolded",
            "UpdateAdConfiguration",
            json!({ "arn": absent_arn("ad-configuration", "neverCreated") }),
            501,
            "NotImplemented",
            None,
        ),
    ];

    for (case, operation, request, status, code, message) in cases {
        let answer = ivs.post(operation, &request).await;
        assert_stated_failure(case, &answer, status, code, message.as_deref());
    }

    // The tagging surface answers a code the operations' own model does not declare, and
    // it travels in the same envelope: the one failure of this service that does.
    let tagging = ivs
        .call(
            "GET",
            &format!("/tags/{}", encode_path_label(&absent_channel)),
            None,
        )
        .await;
    assert_stated_failure(
        "an ARN naming nothing, read through the tagging surface",
        &tagging,
        404,
        "NotFoundException",
        Some("Resource was not found"),
    );
}

/// Rule 2: every response names the request it answers, and no two name the same one.
///
/// The rows span what a response can be: a success with a body, an empty 204 success, and
/// a refusal, each repeated so the ids are compared across calls of the same kind too.
#[tokio::test]
async fn every_response_names_one_request_and_no_two_name_the_same() {
    let ivs = Ivs::new();
    let mut named: Vec<(String, String)> = Vec::new();

    for round in 0..3 {
        let created = ivs
            .post("CreateChannel", &json!({ "name": format!("ided-{round}") }))
            .await;
        assert_eq!(
            created.status, 200,
            "round {round}: expected the create to succeed, got {} {}",
            created.status, created.body
        );
        let arn = created.body["channel"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = ivs.post("GetChannel", &json!({ "arn": arn })).await;
        let refused = ivs.post("GetChannel", &json!({ "arn": "not-an-arn" })).await;
        let deleted = ivs.post("DeleteChannel", &json!({ "arn": arn })).await;
        assert_eq!(
            deleted.status, 204,
            "round {round}: expected the delete to answer 204, got {} {}",
            deleted.status, deleted.body
        );
        for (kind, answer) in [
            ("create", created),
            ("read", read),
            ("refusal", refused),
            ("delete", deleted),
        ] {
            let id = answer.header(REQUEST_ID_HEADER).to_string();
            assert!(
                is_request_id_shaped(&id),
                "round {round} {kind}: expected {REQUEST_ID_HEADER} to take the \
                 16-character base64 token form, got {id:?} in {:?}",
                answer.headers
            );
            named.push((format!("round {round} {kind}"), id));
        }
    }

    for (left, (case, id)) in named.iter().enumerate() {
        for (other_case, other_id) in named.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "case {case:?} and case {other_case:?}: two responses must not name the \
                 same request, both named {id:?}"
            );
        }
    }
}

/// Rule 3: a refusal inside a list names the element's own shape and its zero-based
/// position, and a step the rule does not touch keeps the model's name for it.
///
/// The rows span the lists this service's inputs carry — a list of structures on two
/// operations, at three positions, and a list of scalars whose elements the model gives no
/// shape of their own — beside the two shapes of path the rule leaves alone: a member of
/// the request, and a member of a structure nested two deep in it. The element shapes come
/// from the model (`smithy_query(query="shape",
/// shape="MediaTailorPlaybackConfigurationsList,\
/// BatchStartViewerSessionRevocationViewerSessionList")`: the first list's member targets
/// `MediaTailorPlaybackConfiguration`, the second's
/// `BatchStartViewerSessionRevocationViewerSession`, and
/// `PlaybackRestrictionPolicyAllowedCountryList`'s member targets a string).
#[tokio::test]
async fn a_refusal_inside_a_list_names_the_element_shape_and_its_zero_based_position() {
    let ivs = Ivs::new();
    let media_tailor_pattern =
        "^arn:aws:mediatailor:[a-z0-9-]+:[0-9]+:playbackConfiguration/[a-zA-Z0-9-]+$";
    let channel_pattern = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$";
    let playback_configuration = |arn: &str| json!({ "playbackConfigurationArn": arn });
    let good_playback_configuration = playback_configuration(
        "arn:aws:mediatailor:us-west-2:000000000000:playbackConfiguration/live",
    );
    let viewer_session = |channel_arn: &str| {
        json!({ "channelArn": channel_arn, "viewerId": "viewer-1" })
    };
    let good_viewer_session = viewer_session(&absent_arn("channel", "abcdef123456"));

    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "the only element of a list of structures",
            "CreateAdConfiguration",
            json!({ "name": "ads",
                    "mediaTailorPlaybackConfigurations": [playback_configuration("not-an-arn")] }),
            pattern_refusal(
                "CreateAdConfiguration",
                "mediaTailorPlaybackConfiguration.0.playbackConfigurationArn",
                media_tailor_pattern,
            ),
        ),
        (
            "the third element of a list of structures",
            "CreateAdConfiguration",
            json!({ "name": "ads",
                    "mediaTailorPlaybackConfigurations": [
                        good_playback_configuration,
                        good_playback_configuration,
                        playback_configuration("still-not-an-arn")] }),
            pattern_refusal(
                "CreateAdConfiguration",
                "mediaTailorPlaybackConfiguration.2.playbackConfigurationArn",
                media_tailor_pattern,
            ),
        ),
        (
            "the second element of the same list on another operation",
            "UpdateAdConfiguration",
            json!({ "arn": absent_arn("ad-configuration", "abcdef123456"),
                    "mediaTailorPlaybackConfigurations": [
                        good_playback_configuration,
                        playback_configuration("nor-is-this")] }),
            pattern_refusal(
                "UpdateAdConfiguration",
                "mediaTailorPlaybackConfiguration.1.playbackConfigurationArn",
                media_tailor_pattern,
            ),
        ),
        (
            "an element of a list of structures of another shape",
            "BatchStartViewerSessionRevocation",
            json!({ "viewerSessions": [good_viewer_session, viewer_session("not-an-arn")] }),
            pattern_refusal(
                "BatchStartViewerSessionRevocation",
                "batchStartViewerSessionRevocationViewerSession.1.channelArn",
                channel_pattern,
            ),
        ),
        (
            "an element of a list of SCALARS, which the model gives no shape of its own",
            "CreatePlaybackRestrictionPolicy",
            json!({ "name": "policy", "allowedCountries": ["US", "USA"] }),
            "CreatePlaybackRestrictionPolicy.allowedCountries.1 is required to have a \
             maximum length of 2, found 3"
                .to_string(),
        ),
        (
            "a member of the request, which the rule leaves alone",
            "CreateChannel",
            json!({ "name": "not a legal name!" }),
            pattern_refusal("CreateChannel", "name", "^[a-zA-Z0-9-_]*$"),
        ),
        (
            "a member nested two structures deep, which the rule leaves alone",
            "CreateRecordingConfiguration",
            json!({ "name": "rec",
                    "destinationConfiguration": { "s3": { "bucketName": "Not_A_Bucket" } } }),
            pattern_refusal(
                "CreateRecordingConfiguration",
                "destinationConfiguration.s3.bucketName",
                "^[a-z0-9-.]+$",
            ),
        ),
    ];

    for (case, operation, request, message) in cases {
        let answer = ivs.post(operation, &request).await;
        assert_stated_failure(
            case,
            &answer,
            400,
            "ValidationException",
            Some(message.as_str()),
        );
    }
}
