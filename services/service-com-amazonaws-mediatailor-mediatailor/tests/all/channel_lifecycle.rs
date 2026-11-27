// SPDX-License-Identifier: CC0-1.0

//! The channel create-and-read slice, end to end over HTTP.
//!
//! Every assertion here goes through the router: a request is built, dispatched
//! to the mounted service, and the JSON response read back — so what is checked
//! is the emulator's wire behaviour, not a handler's return value.
//!
//! The rule under test is the one the slice implements: a channel is a resource
//! persisted under its `ChannelName`, so `CreateChannel` mints it ONCE — with
//! the ARN, the per-output playback URLs, the creation and last-modified
//! instants and the `STOPPED` state the service derives — and `DescribeChannel`
//! reports that same stored record member for member, while a second create
//! under a name the account already holds is refused and changes nothing. The
//! table spans the class of channels: both playback modes, every tier
//! (including the defaulted one), one and several outputs, the HLS/DASH/both
//! playlist forms, audiences present and absent, time-shift at each published
//! bound, tag counts from none to several, and several forms of name.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_response_projection::expected_hls_playlist_settings;

/// The region this suite signs for, so the ARNs and playback hosts it asserts
/// are the ones the request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A SigV4 `Authorization` header scoped to `mediatailor` in [`REGION`]: it is
/// what routes the request to this service and what the handler reads the ARN's
/// region from. The signature is never verified here (this router runs with
/// request authentication off).
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/us-east-1/mediatailor/aws4_request, SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// `TimeShiftConfiguration$MaxTimeDelaySeconds`' published bounds, from the
/// member's own documentation: "The minimum allowed maximum time delay is 0
/// seconds, and the maximum allowed maximum time delay is 21600 seconds (6
/// hours)."
pub(crate) const MIN_TIME_SHIFT_SECONDS: i64 = 0;
pub(crate) const MAX_TIME_SHIFT_SECONDS: i64 = 21_600;

/// The refusal every route that addresses a stored MediaTailor resource answers
/// a name or an ARN the account holds no such resource under with: HTTP 404
/// under the error code `NotFoundException`. The model declares no error on
/// those operations, so the status and the code are the service's recorded
/// behaviour, and they are what tells "this resource is gone" apart from the
/// one client error the model DOES declare (`BadRequestException`, HTTP 400,
/// "A request contains unexpected data").
pub(crate) const NOT_FOUND_STATUS: u16 = 404;
pub(crate) const NOT_FOUND_CODE: &str = "NotFoundException";

/// The sentence a channel route answers a channel miss with. It is entirely
/// FIXED text: the requested `ChannelName` is not interpolated into it.
pub(crate) const CHANNEL_NOT_FOUND_MESSAGE: &str = "The specified channel doesn't exist.";

/// The sentence the tagging routes answer an ARN that addresses no resource
/// with — fixed text too, and distinct from the channel sentence.
pub(crate) const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The member this service's one MODELLED error carries its text in:
/// `BadRequestException$Message`, as the shape declares it in this crate's
/// `smithy-model/` document (no `@jsonName`, so the restJson1 codec keys it by
/// that member name).
pub(crate) const MODELLED_ERROR_MESSAGE_MEMBER: &str = "Message";

/// The member the error shapes this service SYNTHESISES carry their text in —
/// the not-found envelope and the scaffold's not-implemented signal, neither of
/// which the model declares — and the member the framework's own pre-handler
/// refusal writes.
pub(crate) const SYNTHETIC_ERROR_MESSAGE_MEMBER: &str = "message";

/// One dispatched request's answer.
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves an error shape from.
    pub(crate) error_code: String,
    /// The parsed JSON body, or `Value::Null` for an empty one.
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn get(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    pub(crate) fn string(&self, pointer: &str) -> String {
        self.get(pointer).as_str().unwrap_or_default().to_string()
    }

    /// The refusal's text, read under whichever key the error shape that
    /// answered spells it.
    ///
    /// The restJson1 codec keys a member by its `@jsonName` or else by the
    /// member name the model declares, so the key is a property of the error
    /// SHAPE: this service's one modelled error spells its member `Message` and
    /// declares no `@jsonName` ([`MODELLED_ERROR_MESSAGE_MEMBER`]), while the
    /// shapes this service synthesises for the failures its model declares none
    /// for — its not-found envelope, the scaffold's not-implemented signal — and
    /// the framework's own pre-handler refusal spell it
    /// [`SYNTHETIC_ERROR_MESSAGE_MEMBER`]. An error body carries exactly one of
    /// the two (the `error_envelope` suite pins which, per shape), so reading
    /// the one that is there reads the refusal's text whichever refused.
    pub(crate) fn message(&self) -> String {
        match self
            .get(&format!("/{MODELLED_ERROR_MESSAGE_MEMBER}"))
            .as_str()
        {
            Some(text) => text.to_string(),
            None => self.string(&format!("/{SYNTHETIC_ERROR_MESSAGE_MEMBER}")),
        }
    }

    /// The `requestId` the error body names its response by, or the empty
    /// string when the body carries none.
    pub(crate) fn request_id(&self) -> String {
        self.string("/requestId")
    }
}

/// The emulator under test: this one service mounted on the real router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SINGLE engine the mounted service reads and writes its state
    /// through, kept so a suite can seed a resource no operation writes yet.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediatailor_mediatailor::EmulatedService::new(engine.clone()),
        );
        Self { router, engine }
    }

    /// The engine this emulator's service was constructed from — the same store
    /// every request of it reads.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    pub(crate) async fn request(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.mediatailor.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
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
            error_code,
            body,
        }
    }

    /// `CreateChannel`: `ChannelName` is the `@httpLabel`, every other member
    /// travels in the body.
    pub(crate) async fn create(&self, name: &str, request: &Value) -> Reply {
        self.request(
            "POST",
            &format!("/channel/{}", encode(name)),
            Some(request.clone()),
        )
        .await
    }

    pub(crate) async fn describe(&self, name: &str) -> Reply {
        self.request("GET", &format!("/channel/{}", encode(name)), None)
            .await
    }

    /// `DeleteChannel`: `ChannelName` is the `@httpLabel` and the operation
    /// carries no request body.
    pub(crate) async fn delete(&self, name: &str) -> Reply {
        self.request("DELETE", &format!("/channel/{}", encode(name)), None)
            .await
    }

    pub(crate) async fn list_tags(&self, arn: &str) -> Reply {
        self.request("GET", &format!("/tags/{}", encode(arn)), None)
            .await
    }

    /// `ListAlerts`: the resource's ARN and the page members all travel in the
    /// query string.
    pub(crate) async fn list_alerts(&self, arn: &str) -> Reply {
        self.request(
            "GET",
            &format!("/alerts?resourceArn={}", encode(arn)),
            None,
        )
        .await
    }

    pub(crate) async fn tag(&self, arn: &str, tags: Value) -> Reply {
        self.request(
            "POST",
            &format!("/tags/{}", encode(arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    pub(crate) async fn untag(&self, arn: &str, keys: &[&str]) -> Reply {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", encode(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.request("DELETE", &format!("/tags/{}?{query}", encode(arn)), None)
            .await
    }
}

/// Percent-encode a value bound to a URI path segment or query parameter, the
/// way an AWS SDK does: everything outside the RFC 3986 unreserved set is
/// escaped, so an ARN's `:` and `/` stay inside one path segment.
pub(crate) fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

pub(crate) fn expected_arn(name: &str) -> String {
    format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}")
}

/// The channels the table spans: each row differs from the others in its
/// playback mode, its tier, how many outputs it has and in which playlist form,
/// whether it carries audiences or a time-shift configuration, how many tags it
/// has, and the form of its name.
pub(crate) fn table() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "a LOOP channel with one HLS output and no optional member",
            "loop-minimal",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{
                    "ManifestName": "index",
                    "SourceGroup": "default",
                    "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                }],
            }),
        ),
        (
            "a LINEAR channel with its required filler slate, STANDARD tier and one tag",
            "linear.standard_1",
            json!({
                "PlaybackMode": "LINEAR",
                "Tier": "STANDARD",
                "FillerSlate": {
                    "SourceLocationName": "slate-location",
                    "VodSourceName": "slate-source",
                },
                "Outputs": [{
                    "ManifestName": "main",
                    "SourceGroup": "group-a",
                    "HlsPlaylistSettings": {
                        "ManifestWindowSeconds": 3600,
                        "AdMarkupType": ["DATERANGE"],
                    },
                }],
                "tags": { "Env": "prod" },
            }),
        ),
        (
            "a DASH-only output, the tier stated explicitly, three tags",
            "dash-only",
            json!({
                "PlaybackMode": "LOOP",
                "Tier": "BASIC",
                "Outputs": [{
                    "ManifestName": "manifest",
                    "SourceGroup": "group-b",
                    "DashPlaylistSettings": {
                        "ManifestWindowSeconds": 60,
                        "MinBufferTimeSeconds": 2,
                        "MinUpdatePeriodSeconds": 2,
                        "SuggestedPresentationDelaySeconds": 60,
                    },
                }],
                "tags": { "Env": "dev", "Team": "video", "Cost-Center": "1234" },
            }),
        ),
        (
            "three outputs: HLS, DASH and one carrying both playlist forms",
            "a",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [
                    {
                        "ManifestName": "hls",
                        "SourceGroup": "group-a",
                        "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    },
                    {
                        "ManifestName": "dash",
                        "SourceGroup": "group-a",
                        "DashPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    },
                    {
                        "ManifestName": "both",
                        "SourceGroup": "group-c",
                        "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                        "DashPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    },
                ],
                "Audiences": ["en", "fr", "de"],
            }),
        ),
        (
            "time-shifted viewing at the published minimum, with audiences",
            "time-shift-min",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "TimeShiftConfiguration": { "MaxTimeDelaySeconds": MIN_TIME_SHIFT_SECONDS },
                "Audiences": ["en"],
            }),
        ),
        (
            "time-shifted viewing at the published maximum and a 64-character name",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "TimeShiftConfiguration": { "MaxTimeDelaySeconds": MAX_TIME_SHIFT_SECONDS },
                "tags": { "k": "v" },
            }),
        ),
    ]
}

/// The tier a channel whose request names none is created in: `BASIC` is the
/// tier MediaTailor assigns unless `STANDARD` is asked for.
const DEFAULT_TIER: &str = "BASIC";

/// The sentence MediaTailor refuses a create under a name already in use with.
/// It is fixed text: it names neither the requested channel nor any other value
/// of the request or of the stored channel.
pub(crate) const CHANNEL_ALREADY_EXISTS_MESSAGE: &str =
    "This channel already exists. Create a unique channel and resubmit your request.";

#[tokio::test]
async fn create_persists_every_member_and_describe_reports_it() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut playback_urls: BTreeSet<String> = BTreeSet::new();

    for (case, name, request) in table() {
        let created = emulator.create(name, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateChannel to answer 200, got {} {:?}",
            created.status, created.body
        );

        // The identity of the stored resource.
        assert_eq!(
            created.string("/ChannelName"),
            name,
            "case {case:?}: expected ChannelName {name:?}, got {:?}",
            created.get("/ChannelName")
        );
        assert_eq!(
            created.string("/Arn"),
            expected_arn(name),
            "case {case:?}: expected the channel's own ARN, got {:?}",
            created.get("/Arn")
        );
        assert!(
            arns.insert(created.string("/Arn")),
            "case {case:?}: every channel has its own ARN, but {:?} was minted twice",
            created.get("/Arn")
        );

        // A channel is created stopped: nothing plays until StartChannel runs.
        assert_eq!(
            created.string("/ChannelState"),
            "STOPPED",
            "case {case:?}: expected a created channel to be STOPPED, got {:?}",
            created.get("/ChannelState")
        );

        // The tier the request asked for, or the default when it asked for none.
        let expected_tier = request
            .get("Tier")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_TIER);
        assert_eq!(
            created.string("/Tier"),
            expected_tier,
            "case {case:?}: expected Tier {expected_tier:?}, got {:?}",
            created.get("/Tier")
        );

        // Both timestamps are the one instant the channel was created at.
        let creation = created.get("/CreationTime").as_f64();
        let modified = created.get("/LastModifiedTime").as_f64();
        assert!(
            creation.is_some() && creation == modified,
            "case {case:?}: expected CreationTime and LastModifiedTime to be the same \
             instant, got {:?} and {:?}",
            created.get("/CreationTime"),
            created.get("/LastModifiedTime")
        );

        // Every output is reported with the members the request carried plus the
        // playback URLs the service derives, whose manifest extension follows the
        // playlist form the output asked for.
        let requested_outputs = request["Outputs"].as_array().expect("the row lists outputs");
        let reported = created
            .get("/Outputs")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            requested_outputs.len(),
            "case {case:?}: expected {} outputs, got {:?}",
            requested_outputs.len(),
            created.get("/Outputs")
        );
        for (index, requested) in requested_outputs.iter().enumerate() {
            let output = &reported[index];
            for member in ["ManifestName", "SourceGroup", "DashPlaylistSettings"] {
                assert_eq!(
                    output.get(member),
                    requested.get(member),
                    "case {case:?}: expected output {index} member {member} to be reported as \
                     sent, got {:?}",
                    output.get(member)
                );
            }
            // The HLS playlist settings carry their own always-present member,
            // so they are reported as the output was STORED with them — the
            // `channel_response_projection` suite owns that rule.
            let expected_hls = expected_hls_playlist_settings(requested.get("HlsPlaylistSettings"));
            assert_eq!(
                output.get("HlsPlaylistSettings").cloned(),
                expected_hls,
                "case {case:?}: expected output {index} HlsPlaylistSettings {expected_hls:?}, \
                 got {:?}",
                output.get("HlsPlaylistSettings")
            );
            let manifest = requested["ManifestName"].as_str().expect("a manifest name");
            let dash_only = requested.get("DashPlaylistSettings").is_some()
                && requested.get("HlsPlaylistSettings").is_none();
            let expected_suffix = if dash_only {
                format!("/v1/dash/{name}/{manifest}.mpd")
            } else {
                format!("/v1/channel/{name}/{manifest}.m3u8")
            };
            let playback = output["PlaybackUrl"].as_str().unwrap_or_default().to_string();
            assert!(
                playback.starts_with("https://")
                    && playback.contains(&format!(".mediatailor.{REGION}.amazonaws.com"))
                    && playback.ends_with(&expected_suffix),
                "case {case:?}: expected output {index} PlaybackUrl on the regional endpoint \
                 ending in {expected_suffix:?}, got {playback:?}"
            );
            let dual_stack = output["DualStackPlaybackUrl"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                dual_stack.contains(&format!(".mediatailor.{REGION}.api.aws"))
                    && dual_stack.ends_with(&expected_suffix),
                "case {case:?}: expected output {index} DualStackPlaybackUrl on the dual-stack \
                 endpoint ending in {expected_suffix:?}, got {dual_stack:?}"
            );
            assert!(
                playback_urls.insert(playback.clone()),
                "case {case:?}: one playback URL addresses one output, but {playback:?} was \
                 minted twice"
            );
        }

        // `Tags` says what tags the channel carries, so it is always reported —
        // as the set the request asked for, or as an empty set. It travels under
        // the member's modelled `jsonName`, `tags`.
        let expected_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            created.get("/tags"),
            &expected_tags,
            "case {case:?}: expected Tags {expected_tags:?}, got {:?}",
            created.get("/tags")
        );

        // The round trip: the read reports the stored record member for member,
        // and adds the log configuration a channel starts with.
        let described = emulator.describe(name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeChannel to answer 200, got {} {:?}",
            described.status, described.body
        );
        for (member, value) in created.body.as_object().expect("a create response object") {
            assert_eq!(
                described.body.get(member),
                Some(value),
                "case {case:?}: expected DescribeChannel to report {member} as CreateChannel \
                 did ({value:?}), got {:?}",
                described.body.get(member)
            );
        }
        assert_eq!(
            described.get("/LogConfiguration/LogTypes"),
            &json!([]),
            "case {case:?}: expected a channel to start with no log type enabled, got {:?}",
            described.get("/LogConfiguration")
        );

        // A read reports; it does not change what the next caller sees.
        let again = emulator.describe(name).await;
        assert_eq!(
            again.body, described.body,
            "case {case:?}: expected a second DescribeChannel to report the same channel, got \
             {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn a_second_create_under_a_stored_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    for (case, name, request) in table() {
        assert_eq!(
            emulator.create(name, &request).await.status,
            200,
            "case {case:?}: the first create must succeed"
        );
        let stored = emulator.describe(name).await.body;

        // A second create under the same name, asking for a different channel
        // altogether: the name identifies one channel, so it is refused.
        let conflicting = json!({
            "PlaybackMode": "LOOP",
            "Tier": "STANDARD",
            "Outputs": [{ "ManifestName": "other", "SourceGroup": "other-group" }],
            "tags": { "Replaced": "yes" },
        });
        let refused = emulator.create(name, &conflicting).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a duplicate CreateChannel to be refused with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            CHANNEL_ALREADY_EXISTS_MESSAGE,
            "case {case:?}: expected the fixed name-collision sentence, got {:?}",
            refused.message()
        );

        // And the channel that is already stored is untouched.
        assert_eq!(
            emulator.describe(name).await.body,
            stored,
            "case {case:?}: expected the refused create to leave the stored channel unchanged"
        );
    }
}

/// The requests a channel cannot be built from, each with the member that makes
/// it impossible. `TimeShiftConfiguration`'s rows are the published bounds of
/// `MaxTimeDelaySeconds` overstepped by one in each direction; the manifest-name
/// rows are values that could address no output of this channel; the LINEAR row
/// is the slate the mode's own documentation requires.
fn refused_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "LINEAR without the filler slate the mode requires",
            json!({
                "PlaybackMode": "LINEAR",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
            }),
        ),
        (
            "an empty manifest name",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "", "SourceGroup": "default" }],
            }),
        ),
        (
            "a manifest name carrying the path separator",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "a/b", "SourceGroup": "default" }],
            }),
        ),
        (
            "two outputs under one manifest name",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [
                    { "ManifestName": "index", "SourceGroup": "group-a" },
                    { "ManifestName": "index", "SourceGroup": "group-b" },
                ],
            }),
        ),
        (
            "a time delay one second below the published minimum",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "TimeShiftConfiguration": { "MaxTimeDelaySeconds": MIN_TIME_SHIFT_SECONDS - 1 },
            }),
        ),
        (
            "a time delay one second above the published maximum",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "TimeShiftConfiguration": { "MaxTimeDelaySeconds": MAX_TIME_SHIFT_SECONDS + 1 },
            }),
        ),
    ]
}

#[tokio::test]
async fn a_channel_that_cannot_be_built_is_refused_and_nothing_is_stored() {
    let emulator = Emulator::new();

    for (case, request) in refused_requests() {
        let name = "refused-channel";
        let refused = emulator.create(name, &request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected CreateChannel to refuse the request with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got an empty message"
        );

        // Nothing was stored, so the read still finds no such channel and the
        // name is still free.
        let missing = emulator.describe(name).await;
        assert_eq!(
            missing.status, NOT_FOUND_STATUS,
            "case {case:?}: expected the refused channel to be absent, got {} {:?}",
            missing.status, missing.body
        );
    }
}

#[tokio::test]
async fn describing_a_channel_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // Created, so the refusal below is about the name and not about an empty store.
    assert_eq!(
        emulator
            .create(
                "stored",
                &json!({
                    "PlaybackMode": "LOOP",
                    "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                })
            )
            .await
            .status,
        200,
        "the channel the other reads are compared against must be created"
    );

    for case in ["absent", "STORED", "stored-2"] {
        let reply = emulator.describe(case).await;
        assert_eq!(
            reply.status, NOT_FOUND_STATUS,
            "case {case:?}: expected DescribeChannel to refuse a channel the account does not \
             hold as not found, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code, NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.message(),
            CHANNEL_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the channel-miss sentence, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_channels_tags_are_one_set_whichever_surface_wrote_them() {
    let emulator = Emulator::new();
    let name = "tagged-channel";
    let created = emulator
        .create(
            name,
            &json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "tags": { "Env": "dev" },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the tagged channel must be created, got {} {:?}",
        created.status, created.body
    );
    let arn = created.string("/Arn");

    // The tags the create carried are the channel's tags on every surface.
    let listed = emulator.list_tags(&arn).await;
    assert_eq!(
        listed.get("/tags"),
        &json!({ "Env": "dev" }),
        "expected ListTagsForResource to report the create's tags, got {:?}",
        listed.body
    );

    // A tag written through the tagging surface shows up on the channel read.
    let tagged = emulator.tag(&arn, json!({ "Team": "video" })).await;
    assert_eq!(
        tagged.status, 204,
        "expected TagResource to answer 204, got {} {:?}",
        tagged.status, tagged.body
    );
    assert_eq!(
        emulator.describe(name).await.get("/tags"),
        &json!({ "Env": "dev", "Team": "video" }),
        "expected DescribeChannel to report the tag TagResource added"
    );

    // And one removed there is gone from the channel read too, while the rest
    // stay.
    assert_eq!(
        emulator.untag(&arn, &["Env"]).await.status,
        204,
        "expected UntagResource to answer 204"
    );
    assert_eq!(
        emulator.describe(name).await.get("/tags"),
        &json!({ "Team": "video" }),
        "expected DescribeChannel to report the remaining tag only"
    );

    // A channel ARN this account holds no channel under names no resource.
    let absent = emulator.list_tags(&expected_arn("absent-channel")).await;
    assert_eq!(
        absent.status, NOT_FOUND_STATUS,
        "expected ListTagsForResource to refuse an ARN naming no channel as not found, got {} {:?}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.message(),
        RESOURCE_NOT_FOUND_MESSAGE,
        "expected the tagging routes' own not-found sentence, got {:?}",
        absent.message()
    );
}
