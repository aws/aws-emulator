// SPDX-License-Identifier: CC0-1.0

//! The playback-configuration lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router: a request is built, dispatched
//! to the mounted service, and the JSON response read back — so what is checked
//! is the emulator's wire behaviour, not a handler's return value.
//!
//! The rule under test is the one the slice implements: a playback
//! configuration is a resource persisted under its `Name`, so
//! `PutPlaybackConfiguration` creates-or-replaces it and answers with the
//! stored record (every member the request carried, plus the ARN and the
//! endpoint prefixes the service derives), and `GetPlaybackConfiguration`,
//! `ListPlaybackConfigurations` and the tagging reads report that same stored
//! record until `DeletePlaybackConfiguration` removes it. The table spans the
//! class of configurations: which optional members are present, the nested
//! configuration shapes, both `InsertionMode` values, tag counts from none to
//! several, and several forms of `Name`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{NOT_FOUND_CODE, NOT_FOUND_STATUS, RESOURCE_NOT_FOUND_MESSAGE};

/// The region this suite signs for, so the ARNs and endpoint hosts it asserts
/// are the ones the request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A SigV4 `Authorization` header scoped to `mediatailor` in [`REGION`]: it is
/// what routes the request to this service and what the handler reads the ARN's
/// region from. The signature is never verified here (this router runs with
/// request authentication off).
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/us-east-1/mediatailor/aws4_request, SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

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

    /// The refusal's text, read under whichever member the error shape that
    /// answered declares for it — the sibling suite's
    /// [`crate::channel_lifecycle::Reply::message`] states the rule and the
    /// `error_envelope` suite pins which shape declares which.
    pub(crate) fn message(&self) -> String {
        match self
            .get(&format!(
                "/{}",
                crate::channel_lifecycle::MODELLED_ERROR_MESSAGE_MEMBER
            ))
            .as_str()
        {
            Some(text) => text.to_string(),
            None => self.string(&format!(
                "/{}",
                crate::channel_lifecycle::SYNTHETIC_ERROR_MESSAGE_MEMBER
            )),
        }
    }
}

/// The sentence a playback-configuration route answers a name the account holds
/// no configuration under with: fixed text around the `Name` the request
/// addressed.
fn configuration_not_found_message(name: &str) -> String {
    format!("PlaybackConfiguration {name} not found")
}

/// The emulator under test: this one service mounted on the real router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediatailor_mediatailor::EmulatedService::new(engine),
        );
        Self { router }
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

    pub(crate) async fn put(&self, configuration: &Value) -> Reply {
        self.request("PUT", "/playbackConfiguration", Some(configuration.clone()))
            .await
    }

    pub(crate) async fn get(&self, name: &str) -> Reply {
        self.request(
            "GET",
            &format!("/playbackConfiguration/{}", encode(name)),
            None,
        )
        .await
    }

    async fn delete(&self, name: &str) -> Reply {
        self.request(
            "DELETE",
            &format!("/playbackConfiguration/{}", encode(name)),
            None,
        )
        .await
    }

    pub(crate) async fn list(&self, max_results: Option<u32>, next_token: Option<&str>) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("MaxResults={max}"));
        }
        if let Some(token) = next_token {
            query.push(format!("NextToken={}", encode(token)));
        }
        let uri = if query.is_empty() {
            "/playbackConfigurations".to_string()
        } else {
            format!("/playbackConfigurations?{}", query.join("&"))
        };
        self.request("GET", &uri, None).await
    }

    async fn list_tags(&self, arn: &str) -> Reply {
        self.request("GET", &format!("/tags/{}", encode(arn)), None)
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

    async fn untag(&self, arn: &str, keys: &[&str]) -> Reply {
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
fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn expected_arn(name: &str) -> String {
    format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:playbackConfiguration/{name}")
}

/// The configurations the table spans: each row differs from the others in
/// which members it carries, the shape of the values in them, how many tags it
/// has, and the form of its name.
fn table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required member",
            json!({ "Name": "minimal" }),
        ),
        (
            "the urls and tags the CLI example passes",
            json!({
                "Name": "demo",
                "AdDecisionServerUrl": "https://ads.example.com/vast",
                "VideoContentSourceUrl": "https://cdn.example.com/out/v1/master.m3u8",
                "tags": { "Env": "dev" },
            }),
        ),
        (
            "every nested configuration shape, PLAYER_SELECT, three tags",
            json!({
                "Name": "full.configuration-1_0",
                "AdDecisionServerUrl": "https://ads.example.com/vast?pid=[player_id]",
                "VideoContentSourceUrl": "https://cdn.example.com/out/v1/index.m3u8",
                "SlateAdUrl": "https://cdn.example.com/slate.mp4",
                "TranscodeProfileName": "custom-profile",
                "InsertionMode": "PLAYER_SELECT",
                "PersonalizationThresholdSeconds": 3,
                "CdnConfiguration": {
                    "AdSegmentUrlPrefix": "https://ads.cdn.example.com",
                    "ContentSegmentUrlPrefix": "https://content.cdn.example.com",
                },
                "DashConfiguration": {
                    "MpdLocation": "DISABLED",
                    "OriginManifestType": "SINGLE_PERIOD",
                },
                "ManifestProcessingRules": { "AdMarkerPassthrough": { "Enabled": true } },
                "AvailSuppression": {
                    "Mode": "BEHIND_LIVE_EDGE",
                    "Value": "00:00:30",
                    "FillPolicy": "PARTIAL_AVAIL",
                },
                "Bumper": {
                    "StartUrl": "https://cdn.example.com/bumper-start.mp4",
                    "EndUrl": "https://cdn.example.com/bumper-end.mp4",
                },
                "ConfigurationAliases": {
                    "player_params.viewer_group": { "preferred": "1", "trailing": "2" },
                },
                "LivePreRollConfiguration": {
                    "AdDecisionServerUrl": "https://ads.example.com/preroll",
                    "MaxDurationSeconds": 15,
                },
                "AdConditioningConfiguration": { "StreamingMediaFileConditioning": "TRANSCODE" },
                "tags": { "Env": "prod", "Team": "video", "Cost-Center": "1234" },
            }),
        ),
        (
            "the default insertion mode stated explicitly",
            json!({
                "Name": "stitched-only",
                "InsertionMode": "STITCHED_ONLY",
                "AdDecisionServerUrl": "https://ads.example.com/vast",
            }),
        ),
        (
            "a single-character name and a single tag",
            json!({ "Name": "a", "tags": { "k": "v" } }),
        ),
        (
            "a 64-character name",
            json!({ "Name": "c".repeat(64) }),
        ),
    ]
}

/// Members whose response value is NOT the request's own: `Name` is asserted
/// against the ARN, `InsertionMode` carries a modelled default, and
/// `DashConfiguration` gains the generated manifest endpoint prefixes and the
/// documented defaults.
const NOT_ECHOED_VERBATIM: [&str; 3] = ["Name", "InsertionMode", "DashConfiguration"];

#[tokio::test]
async fn put_persists_every_member_and_the_reads_report_it() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut playback_prefixes: BTreeSet<String> = BTreeSet::new();

    for (case, request) in table() {
        let name = request["Name"].as_str().expect("the row names it").to_string();
        let put = emulator.put(&request).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutPlaybackConfiguration to answer 200, got {} {:?}",
            put.status, put.body
        );

        // The identity of the stored resource.
        assert_eq!(
            put.string("/Name"),
            name,
            "case {case:?}: expected Name {name:?}, got {:?}",
            put.get("/Name")
        );
        assert_eq!(
            put.string("/PlaybackConfigurationArn"),
            expected_arn(&name),
            "case {case:?}: expected the configuration's own ARN, got {:?}",
            put.get("/PlaybackConfigurationArn")
        );

        // The server-derived endpoint prefixes.
        let playback = put.string("/PlaybackEndpointPrefix");
        assert!(
            playback.starts_with("https://")
                && playback.ends_with(&format!(".mediatailor.{REGION}.amazonaws.com")),
            "case {case:?}: expected a regional playback endpoint prefix, got {playback:?}"
        );
        let dual_stack = put.string("/DualStackPlaybackEndpointPrefix");
        assert!(
            dual_stack.ends_with(&format!(".mediatailor.{REGION}.api.aws")),
            "case {case:?}: expected a dual-stack playback endpoint prefix, got {dual_stack:?}"
        );
        for (pointer, segment) in [
            ("/SessionInitializationEndpointPrefix", "/v1/session/"),
            ("/HlsConfiguration/ManifestEndpointPrefix", "/v1/master/"),
            ("/DashConfiguration/ManifestEndpointPrefix", "/v1/dash/"),
        ] {
            let prefix = put.string(pointer);
            assert!(
                prefix.starts_with(&playback)
                    && prefix.contains(segment)
                    && prefix.ends_with(&format!("/{name}/")),
                "case {case:?}: expected {pointer} under {playback:?} to carry {segment:?} and \
                 end in the configuration name, got {prefix:?}"
            );
        }
        assert_eq!(
            put.get("/LogConfiguration/PercentEnabled"),
            &json!(0),
            "case {case:?}: expected logging to start at 0 percent, got {:?}",
            put.get("/LogConfiguration")
        );

        // Every member the request carried is reported as it was sent.
        for (member, sent) in request.as_object().expect("an object row") {
            if NOT_ECHOED_VERBATIM.contains(&member.as_str()) {
                continue;
            }
            assert_eq!(
                put.body.get(member),
                Some(sent),
                "case {case:?}: expected {member} to round-trip as {sent:?}, got {:?}",
                put.body.get(member)
            );
        }

        // `InsertionMode`'s modelled default applies when the request omits it.
        let expected_mode = request
            .get("InsertionMode")
            .and_then(Value::as_str)
            .unwrap_or("STITCHED_ONLY");
        assert_eq!(
            put.string("/InsertionMode"),
            expected_mode,
            "case {case:?}: expected InsertionMode {expected_mode:?}, got {:?}",
            put.get("/InsertionMode")
        );

        // `DashConfiguration`'s own members keep the request's values, or the
        // defaults the model documents for them.
        for (member, default) in [
            ("MpdLocation", "EMT_DEFAULT"),
            ("OriginManifestType", "MULTI_PERIOD"),
        ] {
            let expected = request
                .pointer(&format!("/DashConfiguration/{member}"))
                .and_then(Value::as_str)
                .unwrap_or(default);
            assert_eq!(
                put.string(&format!("/DashConfiguration/{member}")),
                expected,
                "case {case:?}: expected DashConfiguration.{member} {expected:?}, got {:?}",
                put.get(&format!("/DashConfiguration/{member}"))
            );
        }

        // The read reports the stored record, byte for byte what the write
        // answered — not a canned or echoed body.
        let got = emulator.get(&name).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected GetPlaybackConfiguration to answer 200, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.body, put.body,
            "case {case:?}: expected the read to report the stored record, got {:?}",
            got.body
        );

        // And so does the list item for it.
        let listed = emulator.list(Some(100), None).await;
        let item = listed
            .get("/Items")
            .as_array()
            .expect("Items is a list")
            .iter()
            .find(|item| item.get("Name") == Some(&json!(name)))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            item, put.body,
            "case {case:?}: expected the list item to report the stored record, got {item:?}"
        );

        assert!(
            arns.insert(put.string("/PlaybackConfigurationArn")),
            "case {case:?}: two configurations were given the same ARN"
        );
        assert!(
            playback_prefixes.insert(playback.clone()),
            "case {case:?}: two configurations were given the same playback endpoint"
        );
    }
}

#[tokio::test]
async fn put_under_an_existing_name_updates_it_in_place() {
    let emulator = Emulator::new();
    let first = emulator
        .put(&json!({
            "Name": "replaced",
            "AdDecisionServerUrl": "https://ads.example.com/first",
            "SlateAdUrl": "https://cdn.example.com/slate.mp4",
            "tags": { "Env": "dev" },
        }))
        .await;
    assert_eq!(first.status, 200, "case {:?}: got {:?}", "create", first.body);

    let second = emulator
        .put(&json!({
            "Name": "replaced",
            "AdDecisionServerUrl": "https://ads.example.com/second",
            "InsertionMode": "PLAYER_SELECT",
        }))
        .await;
    assert_eq!(
        second.status, 200,
        "case {:?}: got {:?}",
        "replace", second.body
    );

    // The identity and the endpoints of a configuration do not change when it
    // is replaced.
    assert_eq!(
        second.string("/PlaybackConfigurationArn"),
        first.string("/PlaybackConfigurationArn"),
        "case {:?}: expected the ARN to stay {:?}",
        "replace",
        first.string("/PlaybackConfigurationArn")
    );
    assert_eq!(
        second.string("/SessionInitializationEndpointPrefix"),
        first.string("/SessionInitializationEndpointPrefix"),
        "case {:?}: expected the session endpoint prefix to stay put",
        "replace"
    );

    // The replacement's own members win, and a member it does not carry is
    // gone rather than carried over.
    assert_eq!(
        second.string("/AdDecisionServerUrl"),
        "https://ads.example.com/second",
        "case {:?}: expected the new ADS url, got {:?}",
        "replace",
        second.get("/AdDecisionServerUrl")
    );
    assert_eq!(
        second.get("/SlateAdUrl"),
        &Value::Null,
        "case {:?}: expected the omitted SlateAdUrl to be gone, got {:?}",
        "replace",
        second.get("/SlateAdUrl")
    );

    // Tags are managed by their own operations: a Put that says nothing about
    // them leaves them alone.
    assert_eq!(
        second.get("/tags"),
        &json!({ "Env": "dev" }),
        "case {:?}: expected the existing tags to survive, got {:?}",
        "replace",
        second.get("/tags")
    );

    let got = emulator.get("replaced").await;
    assert_eq!(
        got.body, second.body,
        "case {:?}: expected the read to report the replacement, got {:?}",
        "replace", got.body
    );

    let listed = emulator.list(None, None).await;
    assert_eq!(
        listed.get("/Items").as_array().map(Vec::len),
        Some(1),
        "case {:?}: expected one configuration, not a duplicate, got {:?}",
        "replace",
        listed.get("/Items")
    );
}

#[tokio::test]
async fn pagination_reports_every_configuration_exactly_once() {
    let emulator = Emulator::new();
    // Created out of order on purpose: the pages are ordered by name, not by
    // creation.
    let created = ["delta", "alpha", "echo", "charlie", "bravo"];
    for name in created {
        let reply = emulator.put(&json!({ "Name": name })).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected the configuration to be created, got {:?}",
            reply.body
        );
    }
    let mut expected: Vec<String> = created.iter().map(|name| name.to_string()).collect();
    expected.sort();

    for page_size in [1_u32, 2, 3, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = emulator.list(Some(page_size), token.as_deref()).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected ListPlaybackConfigurations to answer 200, got {} {:?}",
                page.status, page.body
            );
            let items = page.get("/Items").as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size as usize,
                "case {case:?}: expected at most {page_size} items on a page, got {}",
                items.len()
            );
            let names: Vec<String> = items
                .iter()
                .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                names, sorted,
                "case {case:?}: expected a page ordered by name, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match page.get("/NextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every configuration exactly once in name order, got {seen:?}"
        );
    }

    // A token this service never issued is refused rather than served as if it
    // were a name.
    let refused = emulator.list(Some(2), Some("not-a-token")).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected a malformed NextToken to be refused, got {} {:?}",
        "bad NextToken", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code, "BadRequestException",
        "case {:?}: expected BadRequestException, got {:?}",
        "bad NextToken", refused.error_code
    );
}

#[tokio::test]
async fn delete_removes_the_configuration_and_is_idempotent() {
    let emulator = Emulator::new();
    for name in ["kept", "removed"] {
        let reply = emulator.put(&json!({ "Name": name, "tags": { "Env": "dev" } })).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected the configuration to be created, got {:?}",
            reply.body
        );
    }
    let arn = expected_arn("removed");

    let deleted = emulator.delete("removed").await;
    assert_eq!(
        deleted.status, 204,
        "case {:?}: expected DeletePlaybackConfiguration to answer 204, got {} {:?}",
        "delete", deleted.status, deleted.body
    );

    let got = emulator.get("removed").await;
    assert_eq!(
        got.status, NOT_FOUND_STATUS,
        "case {:?}: expected the read of a deleted configuration to fail as not found, got {} {:?}",
        "read after delete", got.status, got.body
    );
    assert_eq!(
        got.message(),
        configuration_not_found_message("removed"),
        "case {:?}: expected the configuration-miss sentence, got {:?}",
        "read after delete",
        got.message()
    );

    let listed = emulator.list(None, None).await;
    let names: Vec<String> = listed
        .get("/Items")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["kept".to_string()],
        "case {:?}: expected only the surviving configuration, got {names:?}",
        "list after delete"
    );

    // The deleted resource's tags went with it: its ARN names nothing now.
    let tags = emulator.list_tags(&arn).await;
    assert_eq!(
        tags.status, NOT_FOUND_STATUS,
        "case {:?}: expected the deleted resource's ARN to name nothing, got {} {:?}",
        "tags after delete", tags.status, tags.body
    );
    assert_eq!(
        tags.message(),
        RESOURCE_NOT_FOUND_MESSAGE,
        "case {:?}: expected the tagging routes' own not-found sentence, got {:?}",
        "tags after delete",
        tags.message()
    );

    // `@idempotent`: deleting it again answers the same way.
    let again = emulator.delete("removed").await;
    assert_eq!(
        again.status, 204,
        "case {:?}: expected a repeated delete to answer 204, got {} {:?}",
        "delete again", again.status, again.body
    );
}

#[tokio::test]
async fn tags_written_either_way_are_readable_both_ways() {
    let emulator = Emulator::new();
    let created = emulator
        .put(&json!({ "Name": "tagged", "tags": { "Env": "dev" } }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: got {:?}",
        "create tagged", created.body
    );
    let arn = created.string("/PlaybackConfigurationArn");

    // Tags written at create time are readable through the tagging surface.
    let listed = emulator.list_tags(&arn).await;
    assert_eq!(
        listed.get("/tags"),
        &json!({ "Env": "dev" }),
        "case {:?}: expected the tags the Put carried, got {:?}",
        "tags at create",
        listed.get("/tags")
    );

    // And tags written through the tagging surface are readable through the
    // resource: one tag set, two ways in.
    let tagged = emulator
        .tag(&arn, json!({ "Team": "video", "Env": "prod" }))
        .await;
    assert_eq!(
        tagged.status, 204,
        "case {:?}: expected TagResource to answer 204, got {} {:?}",
        "tag", tagged.status, tagged.body
    );
    let got = emulator.get("tagged").await;
    assert_eq!(
        got.get("/tags"),
        &json!({ "Env": "prod", "Team": "video" }),
        "case {:?}: expected the merged tag set on the resource, got {:?}",
        "tag",
        got.get("/tags")
    );

    let untagged = emulator.untag(&arn, &["Team", "absent"]).await;
    assert_eq!(
        untagged.status, 204,
        "case {:?}: expected UntagResource to answer 204, got {} {:?}",
        "untag", untagged.status, untagged.body
    );
    assert_eq!(
        emulator.list_tags(&arn).await.get("/tags"),
        &json!({ "Env": "prod" }),
        "case {:?}: expected only the remaining tag",
        "untag"
    );

    // Every tag removed: the resource reports an empty tag map — the member
    // MediaTailor always emits — and its tag listing reports an empty set
    // rather than failing.
    let untagged = emulator.untag(&arn, &["Env"]).await;
    assert_eq!(
        untagged.status, 204,
        "case {:?}: got {} {:?}",
        "untag the rest", untagged.status, untagged.body
    );
    assert_eq!(
        emulator.get("tagged").await.get("/tags"),
        &json!({}),
        "case {:?}: expected an empty tag map on an untagged resource",
        "untag the rest"
    );
    assert_eq!(
        emulator.list_tags(&arn).await.get("/tags"),
        &json!({}),
        "case {:?}: expected an empty tag set",
        "untag the rest"
    );

    // An ARN that names no resource of this service is refused, whatever the
    // shape of the value — including one that ends in a stored configuration's
    // name but whose partition, region or account is not the one the resource's
    // own ARN carries. A tag is addressed by the resource's whole ARN, so an
    // ARN no resource reports addresses nothing.
    for (case, bogus) in [
        ("unknown configuration", expected_arn("never-created")),
        ("another service's ARN", format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/demo")),
        ("not an ARN at all", "tagged".to_string()),
        (
            "another account's ARN for a stored name",
            format!("arn:aws:mediatailor:{REGION}:999999999999:playbackConfiguration/tagged"),
        ),
        (
            "another region's ARN for a stored name",
            format!("arn:aws:mediatailor:eu-west-1:{ACCOUNT}:playbackConfiguration/tagged"),
        ),
        (
            "another partition's ARN for a stored name",
            format!("arn:aws-cn:mediatailor:{REGION}:{ACCOUNT}:playbackConfiguration/tagged"),
        ),
    ] {
        for reply in [
            emulator.list_tags(&bogus).await,
            emulator.tag(&bogus, json!({ "Env": "dev" })).await,
            emulator.untag(&bogus, &["Env"]).await,
        ] {
            assert_eq!(
                reply.status, NOT_FOUND_STATUS,
                "case {case:?}: expected the tagging operations to refuse {bogus:?} as not \
                 found, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code, NOT_FOUND_CODE,
                "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the tagging routes' own not-found sentence, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_names_no_storable_configuration_is_refused() {
    let emulator = Emulator::new();

    // A read of a name nothing is stored under fails — it is never a hollow 200.
    let missing = emulator.get("never-created").await;
    assert_eq!(
        missing.status, NOT_FOUND_STATUS,
        "case {:?}: expected the read to fail as not found, got {} {:?}",
        "unknown name", missing.status, missing.body
    );
    assert_eq!(
        missing.error_code, NOT_FOUND_CODE,
        "case {:?}: expected {NOT_FOUND_CODE}, got {:?}",
        "unknown name", missing.error_code
    );
    assert_eq!(
        missing.message(),
        configuration_not_found_message("never-created"),
        "case {:?}: expected the configuration-miss sentence, got {:?}",
        "unknown name",
        missing.message()
    );

    // A `Name` that cannot address a configuration, and a value outside the
    // modelled `InsertionMode` enum, are refused — and nothing is stored.
    for (case, request) in [
        ("an empty name", json!({ "Name": "" })),
        ("a name carrying the path separator", json!({ "Name": "a/b" })),
        (
            "an insertion mode outside the enum",
            json!({ "Name": "bogus-mode", "InsertionMode": "GUIDED_ONLY" }),
        ),
    ] {
        let reply = emulator.put(&request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected the request to be refused, got {} {:?}",
            reply.status, reply.body
        );
    }

    let listed = emulator.list(None, None).await;
    assert_eq!(
        listed.get("/Items"),
        &json!([]),
        "case {:?}: expected a refused request to store nothing, got {:?}",
        "nothing stored",
        listed.get("/Items")
    );
}

/// Bounds the store refuses to grow past, each row naming the member it bounds
/// and a request one step outside it. Nothing may be stored by a refused
/// request, and a request just inside the bound must still succeed.
#[tokio::test]
async fn a_request_past_a_bound_on_what_is_stored_is_refused() {
    let emulator = Emulator::new();

    // `AdDecisionServerUrl` (documented maximum 25,000 characters) and
    // `VideoContentSourceUrl` (512): the published bound passes, one character
    // past it is refused.
    for (case, member, limit) in [
        ("AdDecisionServerUrl", "AdDecisionServerUrl", 25_000_usize),
        (
            "VideoContentSourceUrl",
            "VideoContentSourceUrl",
            512_usize,
        ),
    ] {
        let at_bound = format!("https://ads.example.com/{}", "a".repeat(limit - 24));
        assert_eq!(at_bound.chars().count(), limit, "the row builds {limit} characters");
        let accepted = emulator
            .put(&json!({ "Name": format!("{case}-at-bound"), member.to_string(): at_bound }))
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the published maximum to be accepted, got {} {:?}",
            accepted.status, accepted.body
        );

        let refused = emulator
            .put(&json!({
                "Name": format!("{case}-past-bound"),
                member.to_string(): format!("https://ads.example.com/{}", "a".repeat(limit)),
            }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected one character past the maximum to be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            emulator.get(&format!("{case}-past-bound")).await.status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected the refused configuration not to be stored"
        );
    }

    // A tag set larger than AWS's 50-tag limit, and a tag key longer than its
    // 128-character limit, are refused on BOTH write paths.
    let many: serde_json::Map<String, Value> = (0..51)
        .map(|index| (format!("k{index}"), json!("v")))
        .collect();
    let long_key = json!({ "k".repeat(129): "v" });
    for (case, tags) in [
        ("51 tags", Value::Object(many)),
        ("a 129-character tag key", long_key),
    ] {
        let refused = emulator
            .put(&json!({ "Name": "over-tagged", "tags": tags.clone() }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected PutPlaybackConfiguration to refuse it, got {} {:?}",
            refused.status, refused.body
        );

        let created = emulator.put(&json!({ "Name": "tag-bound" })).await;
        let arn = created.string("/PlaybackConfigurationArn");
        let refused = emulator.tag(&arn, tags).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected TagResource to refuse it, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            emulator.list_tags(&arn).await.get("/tags"),
            &json!({}),
            "case {case:?}: expected a refused tag write to store nothing"
        );
    }

    // The caller-sized maps are bounded too: aliases for more dynamic variables
    // than the store holds, and a `FunctionMapping` key outside the modelled
    // `EventName` set.
    let aliases: serde_json::Map<String, Value> = (0..51)
        .map(|index| (format!("player_params.v{index}"), json!({ "a": "1" })))
        .collect();
    for (case, request) in [
        (
            "aliases for 51 dynamic variables",
            json!({ "Name": "too-many-aliases", "ConfigurationAliases": Value::Object(aliases) }),
        ),
        (
            "a lifecycle hook outside EventName",
            json!({ "Name": "bogus-hook", "FunctionMapping": { "PRE_PLAYBACK": "fn-1" } }),
        ),
    ] {
        let refused = emulator.put(&request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the request to be refused, got {} {:?}",
            refused.status, refused.body
        );
    }

    // A `FunctionMapping` key the model does name is accepted and round-trips.
    let accepted = emulator
        .put(&json!({
            "Name": "mapped-hook",
            "FunctionMapping": { "PRE_ADS_REQUEST": "fn-1" },
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected a modelled lifecycle hook to be accepted, got {} {:?}",
        "PRE_ADS_REQUEST", accepted.status, accepted.body
    );
    assert_eq!(
        emulator.get("mapped-hook").await.get("/FunctionMapping"),
        &json!({ "PRE_ADS_REQUEST": "fn-1" }),
        "case {:?}: expected the mapping to round-trip",
        "PRE_ADS_REQUEST"
    );
}

#[tokio::test]
async fn an_empty_tag_map_on_a_put_clears_the_resource_tags() {
    let emulator = Emulator::new();
    let created = emulator
        .put(&json!({ "Name": "cleared", "tags": { "Env": "dev" } }))
        .await;
    let arn = created.string("/PlaybackConfigurationArn");

    // An empty `Tags` map is a tag set with nothing in it, so the replacement
    // leaves the resource untagged — unlike an ABSENT `Tags` member, which says
    // nothing about tags and leaves them alone.
    let replaced = emulator
        .put(&json!({ "Name": "cleared", "tags": {} }))
        .await;
    assert_eq!(
        replaced.get("/tags"),
        &json!({}),
        "case {:?}: expected the tag set to be cleared, got {:?}",
        "empty tag map",
        replaced.get("/tags")
    );
    assert_eq!(
        emulator.list_tags(&arn).await.get("/tags"),
        &json!({}),
        "case {:?}: expected no tags on the resource",
        "empty tag map"
    );
}

#[tokio::test]
async fn the_store_holds_a_bounded_number_of_configurations() {
    let emulator = Emulator::new();
    // The bound the service declares (`crate::state::MAX_PLAYBACK_CONFIGURATIONS`),
    // named here as the number of configurations the store accepts.
    const MAX_PLAYBACK_CONFIGURATIONS: usize = 500;

    for index in 0..MAX_PLAYBACK_CONFIGURATIONS {
        let name = format!("config-{index:04}");
        let reply = emulator.put(&json!({ "Name": name })).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected configuration {index} to be accepted, got {} {:?}",
            reply.status, reply.body
        );
    }

    // One more NEW name does not fit, and the refusal is the service's modelled
    // client error rather than an exhausted process.
    let refused = emulator.put(&json!({ "Name": "one-too-many" })).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected the store to be full, got {} {:?}",
        "one past the bound", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code, "BadRequestException",
        "case {:?}: expected BadRequestException, got {:?}",
        "one past the bound", refused.error_code
    );

    // Replacing a configuration that is already stored consumes no capacity, so
    // it still succeeds at the bound.
    let replaced = emulator
        .put(&json!({ "Name": "config-0000", "InsertionMode": "PLAYER_SELECT" }))
        .await;
    assert_eq!(
        replaced.status, 200,
        "case {:?}: expected a replacement to be accepted at the bound, got {} {:?}",
        "replace at the bound", replaced.status, replaced.body
    );

    // And deleting one releases capacity for a new name.
    assert_eq!(
        emulator.delete("config-0001").await.status,
        204,
        "case {:?}: expected the delete to answer 204",
        "release capacity"
    );
    let accepted = emulator.put(&json!({ "Name": "one-too-many" })).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected the released capacity to be reusable, got {} {:?}",
        "release capacity", accepted.status, accepted.body
    );
}
