// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IVS channel lifecycle, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A create persists; a read answers from what was persisted; a delete makes the
//!    resource observably gone.
//! 2. A member the caller omits takes the default the model documents for it.
//! 3. `preset` follows `type`: `HIGHER_BANDWIDTH_DELIVERY` for the two ADVANCED
//!    types, the empty string for every other type.
//! 4. A list answers every persisted resource that matches its filters, in one
//!    stable order, cut into pages of at most `maxResults` with a `nextToken` that
//!    resumes exactly where the previous page stopped.
//! 5. An update replaces the members the caller sent and keeps the rest.
//! 6. A stream key belongs to exactly one channel and dies with it.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::{
    service_state, ComAmazonawsIvsAmazoninteractivevideoserviceState,
};
use service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region
/// it is addressed to, so every call carries one. The signature itself is never
/// checked here: request authentication is off unless a router turns it on.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The ARN prefix every channel this emulator mints must carry, from the modelled
/// `ChannelArn` pattern `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$` plus
/// the canonical emulator account id.
pub(crate) const CHANNEL_ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:channel/";
/// The same, for the modelled `StreamKeyArn` pattern.
pub(crate) const STREAM_KEY_ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:stream-key/";

/// One IVS emulator over one shared engine, reached the way the router reaches it.
pub(crate) struct Ivs {
    router: MultiServiceRouter,
    /// The engine the service was built on, kept so a suite can read this service's
    /// store for state no operation reports back.
    engine: Arc<engine::InMemoryEngine>,
}

/// One answer from the emulator: the status, the wire error code (the restJson1
/// `__type` discriminator) and the parsed body.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

impl Ivs {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The same emulator, SERVED at `base` — the base URL
    /// `OperationContext::endpoint` reports, which is the data port's own URL for a
    /// router that is served on a port and `UNSERVED_URL_BASE` for one that is not
    /// (what [`Self::new`] leaves it as).
    ///
    /// A suite about the addresses a response announces needs both, because an
    /// address this emulator publishes is read off that base whenever the request
    /// itself addressed one of AWS's own hosts.
    pub(crate) fn served_at(base: &str) -> Self {
        let mut ivs = Self::new();
        ivs.router.set_url_base(
            smithy_server_runtime::EndpointUrl::parse(base)
                .expect("the test endpoint base is a bare https URL"),
        );
        ivs
    }

    /// Read this service's store, the way a handler reaches it.
    ///
    /// For state no operation reports back — a started viewer-session revocation,
    /// say — reading the store is the only way to assert what a call persisted.
    pub(crate) fn with_store<T>(
        &self,
        read: impl FnOnce(&ComAmazonawsIvsAmazoninteractivevideoserviceState) -> T,
    ) -> T {
        let state = service_state(&*self.engine).expect("the IVS store is reachable");
        let store = state.lock();
        read(&store)
    }

    /// Write this service's store, for the state no operation can create yet: an
    /// ingest session bringing a channel live (see `live_stream_reads.rs`) has no
    /// control-plane operation behind it until the filed RTMPS listener lands.
    pub(crate) fn with_store_mut<T>(
        &self,
        write: impl FnOnce(&mut ComAmazonawsIvsAmazoninteractivevideoserviceState) -> T,
    ) -> T {
        let state = service_state(&*self.engine).expect("the IVS store is reachable");
        let mut store = state.lock();
        write(&mut store)
    }

    pub(crate) async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.call_as(operation, body, None, AUTHORIZATION).await
    }

    /// The same call, from a client that ADDRESSED `host` and signed in the credential
    /// scope `authorization` names.
    ///
    /// `host` is `None` for a client that names no host of its own, as [`Self::call`]
    /// does: the request URI then carries this service's published endpoint, which is
    /// what an SDK sends. Both matter to what a response ANNOUNCES as an address — the
    /// host a client reached this emulator at, and the Region its scope names — so a
    /// suite about announced addresses drives the operations through this.
    pub(crate) async fn call_as(
        &self,
        operation: &str,
        body: &Value,
        host: Option<&str>,
        authorization: &str,
    ) -> Reply {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!("https://ivs.us-east-1.amazonaws.com/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", authorization);
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }

    /// Call `operation` and require it to have succeeded, returning the body.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let reply = self.call(operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {operation} {body}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// Create a channel and return the whole `CreateChannel` output.
    pub(crate) async fn create(&self, request: Value) -> Value {
        self.ok("CreateChannel", request).await
    }

    /// Create a channel and return just its ARN.
    pub(crate) async fn create_arn(&self, request: Value) -> String {
        text(&self.create(request).await["channel"], "arn")
    }
}

/// A string member, read as the empty string when absent — which is what a client
/// sees for a member the wire omits.
pub(crate) fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A boolean member, read as `false` when absent: `authorized` and `insecureIngest`
/// are modelled `@default(false)`, so absent and `false` are the same value.
fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Lowercase-hex of `text`, for forging a candidate `nextToken` by hand: the token
/// codec is hex, so this is how a caller would construct a value that decodes
/// cleanly and still is not a boundary this service issued.
pub(crate) fn hex_of(text: &str) -> String {
    text.bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

/// Whether `arn` satisfies the modelled ARN pattern for `prefix`'s resource type:
/// the fixed `arn:aws:ivs:<region>:<account>:<type>/` prefix followed by a non-empty
/// `[a-zA-Z0-9-]+` id, within the modelled 128-character bound.
pub(crate) fn arn_is_well_formed(arn: &str, prefix: &str) -> bool {
    let Some(id) = arn.strip_prefix(prefix) else {
        return false;
    };
    arn.len() <= 128
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Rule 1 and rule 2 together, over a table spanning the create surface: what
/// `CreateChannel` returns is what `GetChannel` answers afterwards, and every member
/// the caller omitted carries the default the model documents.
#[tokio::test]
async fn a_created_channel_is_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: which members are sent at
    // all, the enum value chosen for each, the flags, and the size of `tags`.
    let cases: Vec<(&str, Value)> = vec![
        ("every member defaulted", json!({})),
        ("name only", json!({ "name": "demo-channel" })),
        (
            "the issue's own case",
            json!({ "name": "demo-channel", "type": "STANDARD", "latencyMode": "LOW",
                    "tags": { "env": "test" } }),
        ),
        (
            "basic, normal latency, insecure ingest",
            json!({ "name": "basic_1", "type": "BASIC", "latencyMode": "NORMAL",
                    "insecureIngest": true }),
        ),
        (
            "advanced-sd, authorized, fragmented mp4",
            json!({ "name": "advanced-sd", "type": "ADVANCED_SD", "authorized": true,
                    "containerFormat": "FRAGMENTED_MP4" }),
        ),
        (
            "advanced-hd with an explicit preset and several tags",
            json!({ "name": "advanced-hd", "type": "ADVANCED_HD",
                    "preset": "CONSTRAINED_BANDWIDTH_DELIVERY",
                    "tags": { "env": "prod", "team": "video", "cost.centre": "42" } }),
        ),
        (
            "an empty name, which the modelled length 0..128 allows",
            json!({ "name": "" }),
        ),
        (
            "a 128-character name, the modelled upper bound",
            json!({ "name": "n".repeat(128) }),
        ),
    ];

    for (case, request) in cases {
        let ivs = Ivs::new();
        let created = ivs.create(request.clone()).await;
        let channel = &created["channel"];

        // The identity and endpoints the service mints.
        let arn = text(channel, "arn");
        assert!(
            arn_is_well_formed(&arn, CHANNEL_ARN_PREFIX),
            "case {case:?}: arn must satisfy the modelled ChannelArn pattern, got {arn:?}"
        );
        assert!(
            !text(channel, "ingestEndpoint").is_empty(),
            "case {case:?}: expected a minted ingestEndpoint, got {channel}"
        );
        assert!(
            text(channel, "playbackUrl").contains(&text(channel, "arn")[CHANNEL_ARN_PREFIX.len()..]),
            "case {case:?}: the playbackUrl must name this channel, got {channel}"
        );

        // Echoed-and-stored members, and the model's documented defaults for the
        // members this row left out.
        let expected_name = request
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            text(channel, "name"),
            expected_name,
            "case {case:?}: name"
        );
        assert_eq!(
            text(channel, "type"),
            request.get("type").and_then(Value::as_str).unwrap_or("STANDARD"),
            "case {case:?}: type (model default STANDARD)"
        );
        assert_eq!(
            text(channel, "latencyMode"),
            request.get("latencyMode").and_then(Value::as_str).unwrap_or("LOW"),
            "case {case:?}: latencyMode (model default LOW)"
        );
        assert_eq!(
            text(channel, "containerFormat"),
            request.get("containerFormat").and_then(Value::as_str).unwrap_or("TS"),
            "case {case:?}: containerFormat (model default TS)"
        );
        assert_eq!(
            flag(channel, "authorized"),
            flag(&request, "authorized"),
            "case {case:?}: authorized (model default false)"
        );
        assert_eq!(
            flag(channel, "insecureIngest"),
            flag(&request, "insecureIngest"),
            "case {case:?}: insecureIngest (model default false)"
        );
        assert_eq!(
            text(channel, "recordingConfigurationArn"),
            "",
            "case {case:?}: recordingConfigurationArn defaults to the empty string"
        );
        let expected_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            channel.get("tags").cloned().unwrap_or_else(|| json!({})),
            expected_tags,
            "case {case:?}: tags"
        );

        // The stream key CreateChannel mints alongside the channel.
        let stream_key = &created["streamKey"];
        let key_arn = text(stream_key, "arn");
        assert!(
            arn_is_well_formed(&key_arn, STREAM_KEY_ARN_PREFIX),
            "case {case:?}: the minted stream key's arn must satisfy the modelled \
             StreamKeyArn pattern, got {key_arn:?}"
        );
        assert!(
            !text(stream_key, "value").is_empty(),
            "case {case:?}: expected a minted stream-key value, got {stream_key}"
        );
        assert_eq!(
            text(stream_key, "channelArn"),
            arn,
            "case {case:?}: the minted stream key must name its channel"
        );

        // Rule 1: the read answers from state, identically.
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await;
        assert_eq!(
            fetched["channel"], *channel,
            "case {case:?}: GetChannel must answer the channel CreateChannel persisted"
        );
    }
}

/// Rule 3, over every value the model's `ChannelType` enumeration declares
/// (source: the `ChannelType` enum, values ADVANCED_HD | ADVANCED_SD | BASIC |
/// STANDARD) crossed with `preset` omitted and both values of `TranscodePreset`.
#[tokio::test]
async fn preset_follows_channel_type() {
    let presets = [
        None,
        Some("HIGHER_BANDWIDTH_DELIVERY"),
        Some("CONSTRAINED_BANDWIDTH_DELIVERY"),
    ];
    let ivs = Ivs::new();
    for channel_type in ["ADVANCED_HD", "ADVANCED_SD", "BASIC", "STANDARD"] {
        for requested in presets {
            let mut request = json!({ "name": "preset-case", "type": channel_type });
            if let Some(preset) = requested {
                request["preset"] = json!(preset);
            }
            let expected = match channel_type {
                // The two ADVANCED types are the only ones where preset is
                // selectable; with none chosen the documented default applies.
                "ADVANCED_HD" | "ADVANCED_SD" => requested.unwrap_or("HIGHER_BANDWIDTH_DELIVERY"),
                _ => "",
            };
            let channel = ivs.create(request.clone()).await["channel"].clone();
            assert_eq!(
                text(&channel, "preset"),
                expected,
                "case type={channel_type} preset={requested:?}: expected preset {expected:?}, \
                 got {channel}"
            );
        }
    }
}

/// The one cross-member rule the model states for `containerFormat`: with multitrack
/// input enabled it must be FRAGMENTED_MP4, and anything else is a 400
/// `ValidationException` rather than a stored channel.
#[tokio::test]
async fn multitrack_input_requires_the_fragmented_mp4_container() {
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "multitrack enabled with FRAGMENTED_MP4",
            json!({ "name": "mt-ok", "containerFormat": "FRAGMENTED_MP4",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "ALLOW",
                                                       "maximumResolution": "HD" } }),
            true,
        ),
        (
            "multitrack enabled with TS",
            json!({ "name": "mt-ts", "containerFormat": "TS",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "ALLOW",
                                                       "maximumResolution": "HD" } }),
            false,
        ),
        (
            "multitrack enabled with containerFormat omitted, so the TS default applies",
            json!({ "name": "mt-default",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "REQUIRE",
                                                       "maximumResolution": "FULL_HD" } }),
            false,
        ),
        (
            "multitrack present but disabled, so TS is fine",
            json!({ "name": "mt-off", "containerFormat": "TS",
                    "multitrackInputConfiguration": { "enabled": false } }),
            true,
        ),
        (
            "no multitrack at all with TS",
            json!({ "name": "mt-absent", "containerFormat": "TS" }),
            true,
        ),
    ];

    let ivs = Ivs::new();
    for (case, request, accepted) in cases {
        let reply = ivs.call("CreateChannel", &request).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}

/// Rule 4 for `ListChannels`: every page size from the modelled lower bound up to
/// and past the number of channels returns the WHOLE set exactly once, in one order,
/// and `maxResults` outside the modelled 1..100 range is refused.
#[tokio::test]
async fn list_channels_pages_the_whole_set_without_duplicates_or_omissions() {
    let ivs = Ivs::new();
    let mut all = BTreeSet::new();
    for index in 0..5 {
        all.insert(ivs.create_arn(json!({ "name": format!("chan-{index}") })).await);
    }

    // The unpaged answer is the full set, and fixes the order the pages must follow.
    let unpaged = ivs.ok("ListChannels", json!({})).await;
    let order: Vec<String> = unpaged["channels"]
        .as_array()
        .expect("channels is a list")
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect();
    assert_eq!(
        order.iter().cloned().collect::<BTreeSet<_>>(),
        all,
        "an unpaged ListChannels must answer every persisted channel, got {unpaged}"
    );

    // 1 and 100 are the published bounds of `MaxChannelResults`; 2, 4 and 5 span
    // "smaller than", "does not divide" and "exactly the set".
    for page_size in [1_usize, 2, 4, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token = Value::Null;
        for _ in 0..(all.len() + 2) {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = token.as_str() {
                request["nextToken"] = json!(token);
            }
            let page = ivs.ok("ListChannels", request).await;
            let channels = page["channels"].as_array().expect("channels is a list");
            assert!(
                channels.len() <= page_size,
                "case maxResults={page_size}: a page may hold at most maxResults entries, \
                 got {}",
                channels.len()
            );
            seen.extend(channels.iter().map(|summary| text(summary, "arn")));
            token = page.get("nextToken").cloned().unwrap_or(Value::Null);
            if token.as_str().is_none() {
                break;
            }
        }
        assert!(
            token.as_str().is_none(),
            "case maxResults={page_size}: paging must terminate, still holding {token}"
        );
        assert_eq!(
            seen, order,
            "case maxResults={page_size}: paging must yield the same channels, in the same \
             order, as the unpaged call"
        );
    }

    // One past each published bound, and tokens this service never issued — a
    // non-hex one, one that decodes to text that is not a boundary it could have
    // issued, and one stamped for the OTHER list operation.
    for bad in [0, 101] {
        let reply = ivs.call("ListChannels", &json!({ "maxResults": bad })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults={bad}: outside the modelled range 1..100, expected 400 \
             ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
    let first_arn = order.first().expect("at least one channel").clone();
    let forged = [
        ("not hex at all", "notatokenwemade".to_string()),
        ("hex of text that is no boundary", hex_of("zzz")),
        ("hex of a bare arn, unstamped", hex_of(&first_arn)),
        (
            "a token stamped for ListStreamKeys",
            hex_of(&format!("ivs:1:stream-key:{first_arn}")),
        ),
        (
            "a token of a future format version",
            hex_of(&format!("ivs:2:channel:{first_arn}")),
        ),
    ];
    for (case, token) in forged {
        let reply = ivs
            .call("ListChannels", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case forged nextToken ({case}): expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 4's filter half: each `filterBy*` member narrows the answer to the channels
/// whose corresponding member matches it exactly.
#[tokio::test]
async fn list_channels_narrows_to_the_channels_a_filter_names() {
    let ivs = Ivs::new();
    let alpha = ivs.create_arn(json!({ "name": "alpha" })).await;
    let beta = ivs.create_arn(json!({ "name": "beta" })).await;
    let beta_twin = ivs.create_arn(json!({ "name": "beta" })).await;
    let unnamed = ivs.create_arn(json!({})).await;

    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "filterByName=alpha",
            json!({ "filterByName": "alpha" }),
            BTreeSet::from([alpha.clone()]),
        ),
        (
            "filterByName=beta matches both channels of that name",
            json!({ "filterByName": "beta" }),
            BTreeSet::from([beta.clone(), beta_twin.clone()]),
        ),
        (
            "filterByName= matches the channel created without a name",
            json!({ "filterByName": "" }),
            BTreeSet::from([unnamed.clone()]),
        ),
        (
            "filterByName names nothing persisted",
            json!({ "filterByName": "gamma" }),
            BTreeSet::new(),
        ),
        (
            "filterByRecordingConfigurationArn= matches every channel, none recording",
            json!({ "filterByRecordingConfigurationArn": "" }),
            BTreeSet::from([
                alpha.clone(),
                beta.clone(),
                beta_twin.clone(),
                unnamed.clone(),
            ]),
        ),
    ];

    for (case, request, expected) in cases {
        let listed = ivs.ok("ListChannels", request).await;
        let got: BTreeSet<String> = listed["channels"]
            .as_array()
            .expect("channels is a list")
            .iter()
            .map(|summary| text(summary, "arn"))
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected exactly {expected:?}, got {listed}"
        );
    }
}

/// Rule 5: an update replaces the members the request carried and leaves every other
/// member of the stored channel as it was — and the following read observes it.
#[tokio::test]
async fn update_replaces_what_it_is_sent_and_keeps_the_rest() {
    // Each row: the update body, and the members it is expected to change.
    type UpdateCase = (&'static str, Value, Vec<(&'static str, &'static str)>);
    let cases: Vec<UpdateCase> = vec![
        (
            "rename only",
            json!({ "name": "renamed" }),
            vec![("name", "renamed")],
        ),
        (
            "the issue's own case: rename and authorize",
            json!({ "name": "demo-renamed", "authorized": true }),
            vec![("name", "demo-renamed")],
        ),
        (
            "latency mode only",
            json!({ "latencyMode": "NORMAL" }),
            vec![("latencyMode", "NORMAL")],
        ),
        (
            "type and container format together",
            json!({ "type": "BASIC", "containerFormat": "FRAGMENTED_MP4" }),
            vec![("type", "BASIC"), ("containerFormat", "FRAGMENTED_MP4")],
        ),
        (
            "promotion to an ADVANCED type takes the documented preset",
            json!({ "type": "ADVANCED_HD" }),
            vec![("type", "ADVANCED_HD"), ("preset", "HIGHER_BANDWIDTH_DELIVERY")],
        ),
        (
            "nothing at all",
            json!({}),
            vec![],
        ),
    ];

    for (case, update, changed) in cases {
        let ivs = Ivs::new();
        let before = ivs
            .create(json!({ "name": "before", "type": "STANDARD", "latencyMode": "LOW",
                            "tags": { "env": "test" } }))
            .await["channel"]
            .clone();
        let arn = text(&before, "arn");

        let mut request = update.clone();
        request["arn"] = json!(arn);
        let updated = ivs.ok("UpdateChannel", request).await["channel"].clone();

        for (member, expected) in &changed {
            assert_eq!(
                text(&updated, member),
                *expected,
                "case {case:?}: {member} must become {expected:?}, got {updated}"
            );
        }
        // Everything the row did not name, and that the rules above do not derive
        // from a member it named, is untouched.
        let derived: Vec<&str> = changed.iter().map(|(member, _)| *member).collect();
        for member in ["arn", "name", "type", "latencyMode", "containerFormat", "preset"] {
            if !derived.contains(&member) {
                assert_eq!(
                    text(&updated, member),
                    text(&before, member),
                    "case {case:?}: {member} was not sent, so it must keep its stored value; \
                     got {updated}"
                );
            }
        }
        assert_eq!(
            updated.get("tags"),
            before.get("tags"),
            "case {case:?}: tags are not an UpdateChannel member, so they are untouched"
        );
        assert_eq!(
            text(&updated, "ingestEndpoint"),
            text(&before, "ingestEndpoint"),
            "case {case:?}: an update must not re-mint the channel's ingest endpoint"
        );

        // Rule 1 again: the mutation is in state, not just in the response.
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await;
        assert_eq!(
            fetched["channel"], updated,
            "case {case:?}: GetChannel must observe the updated channel"
        );
    }
}

/// Rule 6: a stream key names exactly one channel, `ListStreamKeys` answers that
/// channel's keys from state, and a channel that is not persisted is a 404 rather
/// than an empty list.
#[tokio::test]
async fn stream_keys_belong_to_the_channel_that_owns_them() {
    let ivs = Ivs::new();
    let first = ivs.create(json!({ "name": "first" })).await;
    let first_arn = text(&first["channel"], "arn");
    let second_arn = ivs.create_arn(json!({ "name": "second" })).await;

    // Rows differ in whether the channel was created with tags, so the key the create
    // minted is checked under more than one shape of request. A channel's stream-key
    // quota is one key (`MAX_STREAM_KEYS_PER_CHANNEL`), which `CreateChannel` has
    // already filled, so a further key cannot be added to a channel at all.
    let cases: Vec<(&str, Value)> = vec![
        ("untagged channel", json!({ "name": "owner" })),
        (
            "channel created with tags",
            json!({ "name": "ownerTagged", "tags": { "purpose": "backup" } }),
        ),
    ];

    for (case, create) in cases {
        let ivs = Ivs::new();
        let created = ivs.create(create).await;
        let owner = text(&created["channel"], "arn");
        let minted = &created["streamKey"];
        let mut expected = BTreeSet::new();
        expected.insert(text(minted, "arn"));
        assert!(
            arn_is_well_formed(&text(minted, "arn"), STREAM_KEY_ARN_PREFIX),
            "case {case:?}: a minted stream-key arn must satisfy the modelled pattern, \
             got {minted}"
        );
        assert_eq!(
            text(minted, "channelArn"),
            owner,
            "case {case:?}: the minted key must name the channel it was created with"
        );

        let refused = ivs
            .call("CreateStreamKey", &json!({ "channelArn": owner }))
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {case:?}: a second key on a channel already at its quota must be \
             refused, got {} {} {}",
            refused.status,
            refused.error_code,
            refused.body
        );

        let listed = ivs.ok("ListStreamKeys", json!({ "channelArn": owner })).await;
        let got: BTreeSet<String> = listed["streamKeys"]
            .as_array()
            .expect("streamKeys is a list")
            .iter()
            .map(|summary| text(summary, "arn"))
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: ListStreamKeys must answer exactly the key CreateChannel \
             minted, and the refused call must have added none, got {listed}"
        );
        for summary in listed["streamKeys"].as_array().expect("a list") {
            assert_eq!(
                text(summary, "channelArn"),
                owner,
                "case {case:?}: every listed key must name this channel, got {summary}"
            );
            assert!(
                summary.get("value").is_none(),
                "case {case:?}: StreamKeySummary declares no value member, got {summary}"
            );
        }
    }

    // A key minted for one channel is never listed under another.
    let keys_of_second = ivs
        .ok("ListStreamKeys", json!({ "channelArn": second_arn }))
        .await;
    let second_keys: BTreeSet<String> = keys_of_second["streamKeys"]
        .as_array()
        .expect("streamKeys is a list")
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect();
    assert!(
        !second_keys.contains(&text(&first["streamKey"], "arn")),
        "case cross-channel: the first channel's key must not be listed under the second, \
         got {keys_of_second}"
    );
    assert_ne!(
        first_arn, second_arn,
        "two CreateChannel calls must not mint the same arn"
    );

    // An unknown channel is a 404 for both the create and the list.
    let absent = format!("{CHANNEL_ARN_PREFIX}doesnotexist");
    for operation in ["CreateStreamKey", "ListStreamKeys"] {
        let reply = ivs
            .call(operation, &json!({ "channelArn": absent }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {operation} on an absent channel: expected 404 ResourceNotFoundException, \
             got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 1's delete half: the channel and its keys are gone from state, the delete
/// answers 204 with an empty body, and every later reference to it is a 404.
#[tokio::test]
async fn delete_makes_the_channel_and_its_keys_observably_gone() {
    let ivs = Ivs::new();
    let doomed = ivs.create(json!({ "name": "doomed" })).await;
    let doomed_arn = text(&doomed["channel"], "arn");
    let doomed_key = text(&doomed["streamKey"], "arn");
    let survivor = ivs.create_arn(json!({ "name": "survivor" })).await;

    let reply = ivs
        .call("DeleteChannel", &json!({ "arn": doomed_arn }))
        .await;
    assert_eq!(
        reply.status, 204,
        "case delete: the model pins HTTP 204, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.body,
        Value::Null,
        "case delete: 204 carries an empty body, got {}",
        reply.body
    );

    // Gone from every read, and a repeat delete is a 404 too.
    for (case, operation, request) in [
        ("GetChannel", "GetChannel", json!({ "arn": doomed_arn })),
        (
            "a second DeleteChannel",
            "DeleteChannel",
            json!({ "arn": doomed_arn }),
        ),
        (
            "ListStreamKeys of the deleted channel",
            "ListStreamKeys",
            json!({ "channelArn": doomed_arn }),
        ),
    ] {
        let reply = ivs.call(operation, &request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    let listed = ivs.ok("ListChannels", json!({})).await;
    let remaining: BTreeSet<String> = listed["channels"]
        .as_array()
        .expect("channels is a list")
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect();
    assert_eq!(
        remaining,
        BTreeSet::from([survivor.clone()]),
        "case delete: ListChannels must answer only the channels still persisted, got {listed}"
    );

    // The survivor's own keys are untouched; the deleted channel's key is not
    // listed anywhere any more.
    let survivor_keys = ivs
        .ok("ListStreamKeys", json!({ "channelArn": survivor }))
        .await;
    let survivor_key_arns: BTreeSet<String> = survivor_keys["streamKeys"]
        .as_array()
        .expect("streamKeys is a list")
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect();
    assert!(
        !survivor_key_arns.contains(&doomed_key),
        "case delete: the deleted channel's stream key must be gone, got {survivor_keys}"
    );
    assert_eq!(
        survivor_key_arns.len(),
        1,
        "case delete: deleting one channel must not touch another channel's keys, got \
         {survivor_keys}"
    );
}

/// How the two classes of bad ARN are answered: one that could name a channel but
/// names none is a 404; one that cannot be a `ChannelArn` at all is refused as a 400
/// before any handler runs.
#[tokio::test]
async fn an_arn_that_names_nothing_is_a_404_and_one_that_is_not_an_arn_is_a_400() {
    let ivs = Ivs::new();
    // Created so the store is not merely empty.
    let live = ivs.create_arn(json!({ "name": "live" })).await;

    let not_found = [
        format!("{CHANNEL_ARN_PREFIX}doesnotexist"),
        "arn:aws:ivs:us-east-1:123456789012:channel/doesnotexist".to_string(),
        "arn:aws:ivs:eu-west-1:000000000000:channel/abcdef123456".to_string(),
    ];
    for arn in not_found {
        let reply = ivs.call("GetChannel", &json!({ "arn": arn })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {arn:?}: a well-formed arn naming no channel is a 404, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    let malformed = [
        "not-an-arn",
        "",
        "arn:aws:ivs:us-east-1:000000000000:stream-key/abcdef123456",
        "arn:aws:ivs:US-EAST-1:000000000000:channel/abcdef123456",
        "arn:aws:ivs:us-east-1:000000000000:channel/has_underscore",
    ];
    for arn in malformed {
        let reply = ivs.call("GetChannel", &json!({ "arn": arn })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {arn:?}: a value outside the modelled ChannelArn pattern is a 400, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // A missing required member is the same refusal class.
    let reply = ivs.call("GetChannel", &json!({})).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "case absent arn: expected 400 ValidationException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );

    // And none of that disturbed the channel that does exist.
    let fetched = ivs.ok("GetChannel", json!({ "arn": live })).await;
    assert_eq!(
        text(&fetched["channel"], "arn"),
        live,
        "a refused request must not disturb a persisted channel, got {fetched}"
    );
}

/// The two flags `UpdateChannel` cannot treat as optional, pinned explicitly.
///
/// `authorized` and `insecureIngest` are modelled `@default(false)`, so the wire
/// cannot distinguish "omitted" from "false": an absent member arrives as `false`
/// and the request's value is therefore the channel's new value. Rows span both
/// flags, both stored values and both sent values.
#[tokio::test]
async fn update_takes_the_default_valued_flags_from_the_request() {
    let cases: Vec<(&str, Value, Value, bool, bool)> = vec![
        (
            "authorized stored true, update carries neither flag",
            json!({ "authorized": true }),
            json!({ "name": "renamed" }),
            false,
            false,
        ),
        (
            "authorized stored true, update carries it again",
            json!({ "authorized": true }),
            json!({ "name": "renamed", "authorized": true }),
            true,
            false,
        ),
        (
            "insecureIngest stored true, update carries neither flag",
            json!({ "insecureIngest": true }),
            json!({}),
            false,
            false,
        ),
        (
            "both stored false, update sets both",
            json!({}),
            json!({ "authorized": true, "insecureIngest": true }),
            true,
            true,
        ),
        (
            "both stored true, update carries both",
            json!({ "authorized": true, "insecureIngest": true }),
            json!({ "authorized": true, "insecureIngest": true }),
            true,
            true,
        ),
    ];

    for (case, create, update, authorized, insecure_ingest) in cases {
        let ivs = Ivs::new();
        let mut create_request = create.clone();
        create_request["name"] = json!("flagged");
        let arn = ivs.create_arn(create_request).await;

        let mut request = update.clone();
        request["arn"] = json!(arn);
        let updated = ivs.ok("UpdateChannel", request).await["channel"].clone();
        assert_eq!(
            flag(&updated, "authorized"),
            authorized,
            "case {case:?}: authorized must be {authorized}, got {updated}"
        );
        assert_eq!(
            flag(&updated, "insecureIngest"),
            insecure_ingest,
            "case {case:?}: insecureIngest must be {insecure_ingest}, got {updated}"
        );

        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await;
        assert_eq!(
            fetched["channel"], updated,
            "case {case:?}: GetChannel must observe the updated flags"
        );
    }
}

/// Caller-driven growth of the store is capped on both operations that grow it, and
/// a call past a cap is the quota error the service models — not another entry.
#[tokio::test]
async fn a_create_past_its_cap_is_refused_rather_than_growing_the_store() {
    // The caps the service enforces, read from the constants it enforces them from
    // rather than re-declared here.
    use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::{
        MAX_CHANNELS, MAX_STREAM_KEYS_PER_CHANNEL,
    };

    // The stream-key quota, counted per channel: CreateChannel mints the channel's one
    // key, so the quota is already reached and the next CreateStreamKey on that channel
    // is refused.
    let ivs = Ivs::new();
    let owner = ivs.create_arn(json!({ "name": "keyed" })).await;
    let other = ivs.create_arn(json!({ "name": "other" })).await;
    let created_keys = ivs
        .ok("ListStreamKeys", json!({ "channelArn": owner, "maxResults": 50 }))
        .await;
    assert_eq!(
        created_keys["streamKeys"].as_array().map(Vec::len),
        Some(MAX_STREAM_KEYS_PER_CHANNEL),
        "case a fresh channel: CreateChannel mints the channel's whole quota of keys, \
         got {created_keys}"
    );
    let reply = ivs
        .call("CreateStreamKey", &json!({ "channelArn": owner }))
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case one key past the quota: expected 402 ServiceQuotaExceededException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    // The quota is per channel, so the refused call added nothing to this channel and
    // left the other channel holding its own key.
    let listed = ivs
        .ok("ListStreamKeys", json!({ "channelArn": owner, "maxResults": 50 }))
        .await;
    assert_eq!(
        listed["streamKeys"].as_array().map(Vec::len),
        Some(MAX_STREAM_KEYS_PER_CHANNEL),
        "case at the quota: the channel holds exactly its quota of keys, got {listed}"
    );
    let others = ivs
        .ok("ListStreamKeys", json!({ "channelArn": other, "maxResults": 50 }))
        .await;
    assert_eq!(
        others["streamKeys"].as_array().map(Vec::len),
        Some(MAX_STREAM_KEYS_PER_CHANNEL),
        "case another channel: a refusal on one channel must not change another \
         channel's keys, got {others}"
    );

    // The channel cap, on a store of its own.
    let ivs = Ivs::new();
    for index in 0..MAX_CHANNELS {
        let reply = ivs.call("CreateChannel", &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case channel {index} of {MAX_CHANNELS}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
    }
    let reply = ivs.call("CreateChannel", &json!({})).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case one channel past the cap: expected 402 ServiceQuotaExceededException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
}

/// Reads must not change what a later caller sees: `GetChannel`, `ListChannels` and
/// `ListStreamKeys` are reporting operations, so repeating them answers identically.
#[tokio::test]
async fn reads_are_repeatable() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "stable", "tags": { "env": "test" } })).await;
    let arn = text(&created["channel"], "arn");

    for round in 0..3 {
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await;
        assert_eq!(
            fetched["channel"], created["channel"],
            "case round {round}: GetChannel must answer the same channel every time"
        );
        let listed = ivs.ok("ListChannels", json!({})).await;
        assert_eq!(
            listed["channels"].as_array().map(Vec::len),
            Some(1),
            "case round {round}: a read must not add or remove a channel, got {listed}"
        );
        let keys = ivs.ok("ListStreamKeys", json!({ "channelArn": arn })).await;
        assert_eq!(
            keys["streamKeys"].as_array().map(Vec::len),
            Some(1),
            "case round {round}: a read must not mint a stream key, got {keys}"
        );
    }
}
