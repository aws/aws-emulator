// SPDX-License-Identifier: CC0-1.0

//! What every read of a playback configuration reports, end to end over HTTP.
//!
//! The harness is the one `playback_configuration.rs` owns, so these assertions
//! go through the router too: a request is built, dispatched to the mounted
//! service, and the JSON response read back.
//!
//! Two rules are under test.
//!
//! * A playback configuration's log settings are the ones last written for it:
//!   `ConfigureLogsForPlaybackConfiguration` defines the configuration's whole
//!   `LogConfiguration`, and every projection of that configuration — the
//!   configure-logs answer itself, `GetPlaybackConfiguration`,
//!   `ListPlaybackConfigurations` and a later `PutPlaybackConfiguration` —
//!   reports it unchanged until the next configure-logs call. The table spans
//!   the class of log settings: the percentage at both documented bounds and in
//!   between, every value of the logging-strategy catalog alone, both together
//!   in either order, no strategy at all, and the two interaction-log settings.
//! * The members MediaTailor reports for every configuration are always
//!   present: the tag map under the wire key `tags`, as an empty object when
//!   the configuration carries no tags. And a read reports NOTHING ELSE than
//!   the members its output shape declares, the same set through all three
//!   projections of one configuration. The table spans tag counts from none to
//!   several and configurations that carry other members or none.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{NOT_FOUND_CODE, NOT_FOUND_STATUS};
use crate::playback_configuration::{Emulator, Reply};

/// The logging strategies this service accepts, from the model's
/// `LoggingStrategy` enum (`smithy_query(query="shape",shape="LoggingStrategy")`
/// → `VENDED_LOGS`, `LEGACY_CLOUDWATCH`).
pub(crate) const LOGGING_STRATEGIES: [&str; 2] = ["VENDED_LOGS", "LEGACY_CLOUDWATCH"];

/// The documented bounds of `PercentEnabled` ("Valid values: 0 - 100").
const PERCENT_BOUNDS: [i64; 2] = [0, 100];

/// The number of event types one log-configuration list may name
/// (`crate::state::MAX_LOG_EVENT_TYPES_PER_LIST`): a capacity this emulator
/// chooses, so one stored log configuration cannot grow without bound.
const MAX_EVENT_TYPES_PER_LIST: usize = 100;

/// The members a read of a playback configuration may report: every member
/// `PutPlaybackConfigurationResponse`, `GetPlaybackConfigurationResponse` and
/// the `PlaybackConfiguration` list item declare — the same set on all three —
/// under the key the codec puts it on the wire under, which is `Tags`'
/// `@jsonName` `tags` and each other member's own name
/// (`smithy_query(query="shape",shape="PutPlaybackConfigurationResponse,\
/// GetPlaybackConfigurationResponse,PlaybackConfiguration")`).
const REPORTED_MEMBERS: [&str; 29] = [
    "AdConditioningConfiguration",
    "AdDecisionServerConfiguration",
    "AdDecisionServerUrl",
    "AdsPersonalizationConcurrency",
    "AdsPersonalizationTimeouts",
    "AvailSuppression",
    "BeaconingConfiguration",
    "Bumper",
    "CdnConfiguration",
    "ConfigurationAliases",
    "DashConfiguration",
    "DualStackPlaybackEndpointPrefix",
    "DualStackSessionInitializationEndpointPrefix",
    "FunctionMapping",
    "HlsConfiguration",
    "InsertionMode",
    "LivePreRollConfiguration",
    "LogConfiguration",
    "ManifestProcessingRules",
    "Name",
    "PersonalizationThresholdSeconds",
    "PlaybackConfigurationArn",
    "PlaybackEndpointPrefix",
    "SessionInitializationEndpointPrefix",
    "SlateAdUrl",
    "TranscodeProfileName",
    "VideoContentSourceUrl",
    "YieldOptimizationConfiguration",
    "tags",
];

/// `PUT /configureLogs/playbackConfiguration` for one configuration.
pub(crate) async fn configure_logs(emulator: &Emulator, name: &str, settings: &Value) -> Reply {
    let mut body = settings.clone();
    body.as_object_mut()
        .expect("the row is an object")
        .insert("PlaybackConfigurationName".to_string(), json!(name));
    emulator
        .request("PUT", "/configureLogs/playbackConfiguration", Some(body))
        .await
}

/// The list item `ListPlaybackConfigurations` reports for `name`.
async fn listed_item(emulator: &Emulator, name: &str) -> Value {
    emulator
        .list(Some(100), None)
        .await
        .get("/Items")
        .as_array()
        .expect("Items is a list")
        .iter()
        .find(|item| item.get("Name") == Some(&json!(name)))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The log settings the table spans: each row differs from the others in the
/// percentage it enables, which strategies it names and in what order, and
/// whether it carries the interaction-log settings.
pub(crate) fn log_settings_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the lower bound and no strategy named",
            json!({ "PercentEnabled": PERCENT_BOUNDS[0] }),
        ),
        (
            "half the sessions, vended logs",
            json!({ "PercentEnabled": 50, "EnabledLoggingStrategies": [LOGGING_STRATEGIES[0]] }),
        ),
        (
            "the upper bound, legacy CloudWatch",
            json!({
                "PercentEnabled": PERCENT_BOUNDS[1],
                "EnabledLoggingStrategies": [LOGGING_STRATEGIES[1]],
            }),
        ),
        (
            "both strategies at once",
            json!({ "PercentEnabled": 1, "EnabledLoggingStrategies": LOGGING_STRATEGIES }),
        ),
        (
            "both strategies, the other order",
            json!({
                "PercentEnabled": 99,
                "EnabledLoggingStrategies": [LOGGING_STRATEGIES[1], LOGGING_STRATEGIES[0]],
            }),
        ),
        (
            "an explicitly empty strategy list",
            json!({ "PercentEnabled": 25, "EnabledLoggingStrategies": [] }),
        ),
        (
            "the interaction-log settings too",
            json!({
                "PercentEnabled": 7,
                "EnabledLoggingStrategies": [LOGGING_STRATEGIES[0]],
                "AdsInteractionLog": {
                    "PublishOptInEventTypes": ["RAW_ADS_RESPONSE"],
                    "ExcludeEventTypes": ["BEACON_FIRED", "FILLED_AVAIL"],
                },
                "ManifestServiceInteractionLog": {
                    "PublishOptInEventTypes": ["PRE_SESSION_INIT_HOOK_SUMMARY"],
                    "ExcludeEventTypes": ["GENERATED_MANIFEST"],
                },
            }),
        ),
    ]
}

/// The `LogConfiguration` a configure-logs request leaves in force: the
/// percentage it carried, the strategies it named (none at all when it named
/// none) and the interaction-log settings it carried.
pub(crate) fn expected_log_configuration(settings: &Value) -> Value {
    let mut expected = json!({
        "PercentEnabled": settings["PercentEnabled"].clone(),
        "EnabledLoggingStrategies": settings
            .get("EnabledLoggingStrategies")
            .cloned()
            .unwrap_or_else(|| json!([])),
    });
    let members = expected.as_object_mut().expect("an object");
    for member in ["AdsInteractionLog", "ManifestServiceInteractionLog"] {
        if let Some(value) = settings.get(member) {
            members.insert(member.to_string(), value.clone());
        }
    }
    expected
}

#[tokio::test]
async fn the_log_settings_a_configure_call_wrote_are_what_every_read_reports() {
    let emulator = Emulator::new();

    for (case, settings) in log_settings_table() {
        let name = format!("logged-{}", case.replace([' ', ','], "-"));
        let created = emulator.put(&json!({ "Name": name })).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the configuration to be created, got {} {:?}",
            created.status, created.body
        );
        // Nothing has configured logs yet: the structure's default is what a
        // configuration reports until a configure-logs call writes one.
        assert_eq!(
            created.get("/LogConfiguration"),
            &json!({ "PercentEnabled": 0, "EnabledLoggingStrategies": [] }),
            "case {case:?}: expected logging to start at 0 percent with no strategy, got {:?}",
            created.get("/LogConfiguration")
        );

        let configured = configure_logs(&emulator, &name, &settings).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected ConfigureLogsForPlaybackConfiguration to answer 200, got {} {:?}",
            configured.status, configured.body
        );
        assert_eq!(
            configured.string("/PlaybackConfigurationName"),
            name,
            "case {case:?}: expected the answer to name the configuration it wrote, got {:?}",
            configured.get("/PlaybackConfigurationName")
        );

        let expected = expected_log_configuration(&settings);
        for (source, body) in [
            ("the configure-logs answer", {
                // The configure-logs answer carries the same members as the
                // configuration's `LogConfiguration`, minus the name it echoes.
                let mut answer = configured.body.clone();
                answer
                    .as_object_mut()
                    .expect("an object body")
                    .remove("PlaybackConfigurationName");
                answer
            }),
            ("the read", emulator.get(&name).await.get("/LogConfiguration").clone()),
            (
                "the list item",
                listed_item(&emulator, &name)
                    .await
                    .get("LogConfiguration")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
        ] {
            assert_eq!(
                body, expected,
                "case {case:?}: expected {source} to report the stored log configuration \
                 {expected:?}, got {body:?}"
            );
        }

        // A later Put says nothing about logging, so it leaves the settings
        // alone and reports them as they stand.
        let replaced = emulator
            .put(&json!({ "Name": name, "AdDecisionServerUrl": "https://ads.example.com/vast" }))
            .await;
        assert_eq!(
            replaced.get("/LogConfiguration"),
            &expected,
            "case {case:?}: expected the replacement to carry the log settings forward, got {:?}",
            replaced.get("/LogConfiguration")
        );
    }
}

#[tokio::test]
async fn a_later_configure_call_replaces_the_whole_log_configuration() {
    let emulator = Emulator::new();
    let name = "reconfigured";
    emulator.put(&json!({ "Name": name })).await;

    // Each row is applied over the one before it, so what the read reports is
    // the LAST call's settings and nothing of the previous one's.
    for (case, settings) in log_settings_table() {
        let configured = configure_logs(&emulator, name, &settings).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected the reconfigure to answer 200, got {} {:?}",
            configured.status, configured.body
        );
        let expected = expected_log_configuration(&settings);
        let got = emulator.get(name).await;
        assert_eq!(
            got.get("/LogConfiguration"),
            &expected,
            "case {case:?}: expected the read to report only the last settings written, got {:?}",
            got.get("/LogConfiguration")
        );
    }
}

#[tokio::test]
async fn a_refused_configure_call_writes_nothing() {
    let emulator = Emulator::new();
    let name = "bounded-percent";
    emulator.put(&json!({ "Name": name })).await;
    let in_force = json!({ "PercentEnabled": 50, "EnabledLoggingStrategies": ["VENDED_LOGS"] });
    assert_eq!(
        configure_logs(&emulator, name, &in_force).await.status,
        200,
        "expected the settings under test to be written"
    );

    // One past each documented bound, and a strategy the enum does not list.
    // The refusals outside the bound say which limit was exceeded, so the
    // caller can fix the request from the message alone.
    for (case, settings, expected_message) in [
        (
            "one below the lower bound",
            json!({ "PercentEnabled": PERCENT_BOUNDS[0] - 1 }),
            Some("PercentEnabled must not be less than 0"),
        ),
        (
            "one above the upper bound",
            json!({ "PercentEnabled": PERCENT_BOUNDS[1] + 1 }),
            Some("PercentEnabled must not exceed 100"),
        ),
        (
            "far above the upper bound",
            json!({ "PercentEnabled": 1_000 }),
            Some("PercentEnabled must not exceed 100"),
        ),
        (
            "a strategy the model does not list",
            json!({ "PercentEnabled": 10, "EnabledLoggingStrategies": ["SYSLOG"] }),
            None,
        ),
        (
            "one event type past the list capacity",
            json!({
                "PercentEnabled": 10,
                "EnabledLoggingStrategies": vec![LOGGING_STRATEGIES[0]; MAX_EVENT_TYPES_PER_LIST + 1],
            }),
            Some(
                "EnabledLoggingStrategies names at most 100 event types, \
                 and this request names 101",
            ),
        ),
        (
            "an interaction-log list past the capacity",
            json!({
                "PercentEnabled": 10,
                "AdsInteractionLog": {
                    "ExcludeEventTypes": vec!["BEACON_FIRED"; MAX_EVENT_TYPES_PER_LIST + 1],
                },
            }),
            Some(
                "AdsInteractionLog.ExcludeEventTypes names at most 100 event types, \
                 and this request names 101",
            ),
        ),
    ] {
        let refused = configure_logs(&emulator, name, &settings).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the request to be refused, got {} {:?}",
            refused.status, refused.body
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                refused.message(),
                expected_message,
                "case {case:?}: expected the refusal to name the limit, got {:?}",
                refused.body
            );
        }
        assert_eq!(
            emulator.get(name).await.get("/LogConfiguration"),
            &expected_log_configuration(&in_force),
            "case {case:?}: expected the refused call to leave the settings alone"
        );
    }

    // A name no configuration is stored under addresses nothing to configure.
    let unknown = configure_logs(&emulator, "never-created", &in_force).await;
    assert_eq!(
        unknown.status, NOT_FOUND_STATUS,
        "case {:?}: expected an unknown configuration to be refused as not found, got {} {:?}",
        "unknown configuration", unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code, NOT_FOUND_CODE,
        "case {:?}: expected {NOT_FOUND_CODE}, got {:?}",
        "unknown configuration", unknown.error_code
    );
}

#[tokio::test]
async fn a_value_a_request_names_twice_round_trips_as_sent() {
    let emulator = Emulator::new();
    let name = "repeated-values";
    emulator.put(&json!({ "Name": name })).await;

    // The model constrains these lists' VALUES (each is an enum value the
    // framework refuses anything outside of), not their order or their length,
    // so a list is stored as the request sent it and the read reports it that
    // way — the handler stores what it was given rather than normalising it.
    for (case, settings) in [
        (
            "one strategy named twice",
            json!({
                "PercentEnabled": 10,
                "EnabledLoggingStrategies": [LOGGING_STRATEGIES[0], LOGGING_STRATEGIES[0]],
            }),
        ),
        (
            "both strategies with one repeated in between",
            json!({
                "PercentEnabled": 20,
                "EnabledLoggingStrategies": [
                    LOGGING_STRATEGIES[0],
                    LOGGING_STRATEGIES[1],
                    LOGGING_STRATEGIES[0],
                ],
            }),
        ),
        (
            "an interaction-log event type named twice",
            json!({
                "PercentEnabled": 30,
                "AdsInteractionLog": {
                    "ExcludeEventTypes": ["BEACON_FIRED", "BEACON_FIRED", "FILLED_AVAIL"],
                },
            }),
        ),
        (
            "a list right at the capacity",
            json!({
                "PercentEnabled": 40,
                "EnabledLoggingStrategies": vec![LOGGING_STRATEGIES[1]; MAX_EVENT_TYPES_PER_LIST],
            }),
        ),
    ] {
        let configured = configure_logs(&emulator, name, &settings).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected the request to be served, got {} {:?}",
            configured.status, configured.body
        );
        let expected = expected_log_configuration(&settings);
        assert_eq!(
            emulator.get(name).await.get("/LogConfiguration"),
            &expected,
            "case {case:?}: expected the list stored as sent {expected:?}, got {:?}",
            emulator.get(name).await.get("/LogConfiguration")
        );
    }
}

/// The keys `body` carries, as a sorted set.
fn reported_keys(body: &Value) -> BTreeSet<String> {
    body.as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn every_read_reports_the_tag_map_and_only_members_the_model_declares() {
    let emulator = Emulator::new();

    // The rows differ in how many tags the configuration carries, from none to
    // several, and in whether it carries any other member at all: the `tags`
    // key is present either way, and no row's answer carries a key the output
    // shape does not declare.
    for (case, tags, extra) in [
        ("no Tags member at all", None, None),
        ("an empty tag map", Some(json!({})), None),
        ("one tag", Some(json!({ "Env": "dev" })), None),
        (
            "three tags",
            Some(json!({ "Env": "prod", "Team": "video", "Cost-Center": "1234" })),
            None,
        ),
        (
            "one tag and several other members",
            Some(json!({ "Env": "staging" })),
            Some(json!({
                "AdDecisionServerUrl": "https://ads.example.com/vast",
                "VideoContentSourceUrl": "https://origin.example.com/out/v1",
                "SlateAdUrl": "https://origin.example.com/slate.m3u8",
                "PersonalizationThresholdSeconds": 3,
            })),
        ),
    ] {
        let name = format!("tagged-{}", case.replace([' ', ','], "-"));
        let mut request = json!({ "Name": name });
        let members = request.as_object_mut().expect("an object");
        if let Some(tags) = tags.clone() {
            members.insert("tags".to_string(), tags);
        }
        if let Some(extra) = extra {
            for (member, value) in extra.as_object().expect("an object") {
                members.insert(member.clone(), value.clone());
            }
        }
        let created = emulator.put(&request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the configuration to be created, got {} {:?}",
            created.status, created.body
        );

        let expected_tags = tags.unwrap_or_else(|| json!({}));
        let got = emulator.get(&name).await;
        let item = listed_item(&emulator, &name).await;
        let readings = [
            ("the write", &created.body),
            ("the read", &got.body),
            ("the list item", &item),
        ];
        for (source, body) in readings {
            assert_eq!(
                body.get("tags"),
                Some(&expected_tags),
                "case {case:?}: expected {source} to report the tag map {expected_tags:?}, got {:?}",
                body.get("tags")
            );
            // A key the output shape does not declare is a member no client
            // generated from the model can read, so no read reports one.
            let declared: BTreeSet<String> =
                REPORTED_MEMBERS.iter().map(|m| (*m).to_string()).collect();
            let undeclared: BTreeSet<String> = reported_keys(body)
                .difference(&declared)
                .cloned()
                .collect();
            assert!(
                undeclared.is_empty(),
                "case {case:?}: expected {source} to report only members the output shape \
                 declares, got the undeclared {undeclared:?} in {body:?}"
            );
        }
        // One stored configuration viewed through three shapes that declare the
        // same members: the three readings report the same key set.
        let keys = reported_keys(readings[0].1);
        for (source, body) in &readings[1..] {
            assert_eq!(
                reported_keys(body),
                keys,
                "case {case:?}: expected {source} to report the same members as the write \
                 {keys:?}, got {:?}",
                reported_keys(body)
            );
        }
    }

    // Tagging a configuration after the fact is visible through its own read,
    // so the map a read reports is the resource's tag set and not the Put's.
    let created = emulator.put(&json!({ "Name": "tagged-later" })).await;
    let arn = created.string("/PlaybackConfigurationArn");
    assert_eq!(
        emulator.tag(&arn, json!({ "Env": "dev" })).await.status,
        204,
        "expected TagResource to answer 204"
    );
    assert_eq!(
        emulator.get("tagged-later").await.get("/tags"),
        &json!({ "Env": "dev" }),
        "case {:?}: expected the read to report the tag written through the tagging surface",
        "tagged later"
    );
}
