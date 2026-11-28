// SPDX-License-Identifier: CC0-1.0

//! The configuration ARNs every channel projection reports, driven over the wire
//! through `MultiServiceRouter::dispatch` exactly as a signed `aws ivs` call arrives.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: a channel
//! object reports each of its optional configuration ARNs as a PRESENT key — the ARN of
//! the resource configured for it, or the empty string when none is — on every
//! projection of that channel, the full `Channel` a create, a read, an update and a batch
//! read answer with and the `ChannelSummary` a list answers with alike. A client decides
//! "is this configured?" by comparing the value with the empty string, so an absent key
//! is a different answer from "nothing is configured".
//!
//! The rows differ along which of the configuration ARNs the channel has: none, each one
//! alone, all of them together, and each sent explicitly as the empty string.

use serde_json::{json, Value};

use super::channel_lifecycle::{text, Ivs, CHANNEL_ARN_PREFIX};

/// Every configuration ARN a channel object carries.
///
/// All four are members both channel shapes declare
/// (`smithy_query(query="shape",shape="Channel,ChannelSummary")`), each documented
/// "Default: \"\"" — so each reads the same way, and every assertion below checks them
/// together.
const CONFIGURATION_ARNS: &[&str] = &[
    "recordingConfigurationArn",
    "playbackRestrictionPolicyArn",
    "adConfigurationArn",
    "failoverSlateConfigurationArn",
];

/// The member whose own resource and channel-level input this model revision declares
/// (`smithy_query(query="operation",operation="CreateChannel,UpdateChannel")` carry it,
/// and `CreateFailoverSlateConfiguration` mints what it names), checked on its own below
/// over channel shapes that configure nothing: a channel with no failover slate reports
/// the unconfigured value for it rather than no key at all.
const FAILOVER_SLATE: &str = "failoverSlateConfigurationArn";

/// The value an unconfigured configuration ARN reads as.
const UNCONFIGURED: &str = "";

/// One row: the case name, the members the `CreateChannel` request adds, and the value
/// every projection must report for each configuration ARN.
struct Case {
    case: String,
    request: Value,
    expected: Vec<(&'static str, String)>,
}

/// Create the resources a row attaches and build the table.
///
/// Each ARN comes from the create that minted the resource, never from a literal, so the
/// row attaches a resource this emulator really holds.
async fn table(ivs: &Ivs) -> Vec<Case> {
    let recording = text(
        &ivs.ok(
            "CreateRecordingConfiguration",
            json!({ "name": "recording",
                    "destinationConfiguration": { "s3": { "bucketName": "ivs-recordings" } } }),
        )
        .await["recordingConfiguration"],
        "arn",
    );
    // A playback-restriction-policy ARN of the form the modelled
    // `ChannelPlaybackRestrictionPolicyArn` pattern
    // `^^$|^arn:aws:ivs:[a-z0-9-]+:[0-9]+:playback-restriction-policy/[a-zA-Z0-9-]+$$`
    // pins, written out rather than minted because `CreatePlaybackRestrictionPolicy` is
    // still scaffolded in this emulator: what the rows vary is which ARN a channel
    // carries, and a channel create stores the ARN the request names.
    let policy =
        "arn:aws:ivs:us-east-1:000000000000:playback-restriction-policy/testpolicy".to_string();
    let ad = text(
        &ivs.ok(
            "CreateAdConfiguration",
            json!({ "name": "ads",
                    "mediaTailorPlaybackConfigurations": [
                        { "playbackConfigurationArn":
                          "arn:aws:mediatailor:us-east-1:000000000000:playbackConfiguration/ads" }
                    ] }),
        )
        .await["adConfiguration"],
        "arn",
    );
    let slate = text(
        &ivs.ok(
            "CreateFailoverSlateConfiguration",
            json!({ "name": "slate" }),
        )
        .await["failoverSlateConfiguration"],
        "arn",
    );

    let unconfigured = |overrides: &[(&'static str, &str)]| {
        CONFIGURATION_ARNS
            .iter()
            .map(|member| {
                let configured = overrides
                    .iter()
                    .find(|(name, _)| name == member)
                    .map(|(_, arn)| (*arn).to_string());
                (*member, configured.unwrap_or_else(|| UNCONFIGURED.to_string()))
            })
            .collect::<Vec<_>>()
    };

    vec![
        Case {
            case: "no configuration ARN sent at all".to_string(),
            request: json!({ "name": "bare" }),
            expected: unconfigured(&[]),
        },
        Case {
            case: "every configuration ARN sent as the empty string".to_string(),
            request: json!({ "name": "empties", "recordingConfigurationArn": "",
                             "playbackRestrictionPolicyArn": "", "adConfigurationArn": "",
                             "failoverSlateConfigurationArn": "" }),
            expected: unconfigured(&[]),
        },
        Case {
            case: "a recording configuration attached".to_string(),
            request: json!({ "name": "recorded", "recordingConfigurationArn": recording }),
            expected: unconfigured(&[("recordingConfigurationArn", &recording)]),
        },
        Case {
            case: "a playback restriction policy attached".to_string(),
            request: json!({ "name": "restricted", "playbackRestrictionPolicyArn": policy }),
            expected: unconfigured(&[("playbackRestrictionPolicyArn", &policy)]),
        },
        Case {
            case: "an ad configuration attached".to_string(),
            request: json!({ "name": "monetised", "adConfigurationArn": ad }),
            expected: unconfigured(&[("adConfigurationArn", &ad)]),
        },
        Case {
            case: "a failover slate configuration attached".to_string(),
            request: json!({ "name": "slated", "failoverSlateConfigurationArn": slate }),
            expected: unconfigured(&[("failoverSlateConfigurationArn", &slate)]),
        },
        Case {
            case: "all four configurable ARNs attached together".to_string(),
            request: json!({ "name": "everything", "recordingConfigurationArn": recording,
                             "playbackRestrictionPolicyArn": policy,
                             "adConfigurationArn": ad,
                             "failoverSlateConfigurationArn": slate }),
            expected: unconfigured(&[
                ("recordingConfigurationArn", &recording),
                ("playbackRestrictionPolicyArn", &policy),
                ("adConfigurationArn", &ad),
                ("failoverSlateConfigurationArn", &slate),
            ]),
        },
    ]
}

/// Assert one channel object reports every configuration ARN `expected` names.
fn assert_configuration_arns(
    case: &str,
    projection: &str,
    channel: &Value,
    expected: &[(&'static str, String)],
) {
    for (member, value) in expected {
        let actual = channel.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual,
            Value::String(value.clone()),
            "case {case:?}: {projection} must report {member} as {value:?}, got {actual:?} \
             in {channel}"
        );
    }
}

/// The rule over every projection of one channel: create, read, batch read, list and
/// update all report the same configuration ARNs.
#[tokio::test]
async fn every_channel_projection_reports_every_configuration_arn() {
    let ivs = Ivs::new();
    for Case {
        case,
        request,
        expected,
    } in table(&ivs).await
    {
        let created = ivs.create(request.clone()).await["channel"].clone();
        assert_configuration_arns(&case, "CreateChannel", &created, &expected);

        let arn = text(&created, "arn");
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();
        assert_configuration_arns(&case, "GetChannel", &fetched, &expected);

        let batch = ivs
            .ok("BatchGetChannel", json!({ "arns": [arn.clone()] }))
            .await;
        let batched = batch["channels"]
            .as_array()
            .and_then(|channels| channels.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_configuration_arns(&case, "BatchGetChannel", &batched, &expected);

        // A batch whose other ARN names no channel: the resolved channel is projected
        // exactly as it is on its own.
        let mixed = ivs
            .ok(
                "BatchGetChannel",
                json!({ "arns": [arn.clone(), format!("{CHANNEL_ARN_PREFIX}absentchannel")] }),
            )
            .await;
        let resolved = mixed["channels"]
            .as_array()
            .and_then(|channels| channels.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_configuration_arns(
            &case,
            "BatchGetChannel over a mixed batch",
            &resolved,
            &expected,
        );

        let listed = ivs.ok("ListChannels", json!({})).await;
        let summary = listed["channels"]
            .as_array()
            .and_then(|channels| {
                channels
                    .iter()
                    .find(|channel| text(channel, "arn") == arn)
                    .cloned()
            })
            .unwrap_or(Value::Null);
        assert_configuration_arns(&case, "ListChannels", &summary, &expected);

        // An update that names none of them keeps what the create configured, and
        // reports it on its own answer.
        let updated = ivs
            .ok("UpdateChannel", json!({ "arn": arn, "name": "renamed" }))
            .await["channel"]
            .clone();
        assert_configuration_arns(&case, "UpdateChannel", &updated, &expected);
    }
}

/// The half of the rule a channel that configures nothing states: the failover-slate ARN
/// is a present key on every channel object, whatever else the channel carries.
#[tokio::test]
async fn the_failover_slate_arn_is_present_on_every_channel_object() {
    // Rows differ along the shape of the channel itself — its type and preset, whether
    // it is authorized, whether it ingests insecurely, whether it carries tags, and
    // whether multitrack input is enabled (which forces `containerFormat`).
    let cases: Vec<(&str, Value)> = vec![
        ("a channel created from an empty request", json!({})),
        (
            "a BASIC channel with tags",
            json!({ "name": "basic", "type": "BASIC", "tags": { "team": "video" } }),
        ),
        (
            "an authorized STANDARD channel ingesting insecurely",
            json!({ "name": "standard", "type": "STANDARD", "authorized": true,
                    "insecureIngest": true }),
        ),
        (
            "an ADVANCED_HD channel, whose preset the type decides",
            json!({ "name": "advanced-hd", "type": "ADVANCED_HD" }),
        ),
        (
            "an ADVANCED_SD channel with multitrack input enabled",
            json!({ "name": "advanced-sd", "type": "ADVANCED_SD",
                    "containerFormat": "FRAGMENTED_MP4",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "REQUIRE",
                                                      "maximumResolution": "HD" } }),
        ),
        (
            "a channel with the longest name the model admits",
            json!({ "name": "n".repeat(128) }),
        ),
    ];

    for (case, request) in cases {
        let ivs = Ivs::new();
        let created = ivs.create(request.clone()).await["channel"].clone();
        let arn = text(&created, "arn");

        let listed = ivs.ok("ListChannels", json!({})).await;
        let summary = listed["channels"]
            .as_array()
            .and_then(|channels| channels.first().cloned())
            .unwrap_or(Value::Null);
        let batch = ivs
            .ok("BatchGetChannel", json!({ "arns": [arn.clone()] }))
            .await;
        let batched = batch["channels"]
            .as_array()
            .and_then(|channels| channels.first().cloned())
            .unwrap_or(Value::Null);
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();

        for (projection, channel) in [
            ("CreateChannel", &created),
            ("GetChannel", &fetched),
            ("BatchGetChannel", &batched),
            ("ListChannels", &summary),
        ] {
            let actual = channel.get(FAILOVER_SLATE).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual,
                Value::String(UNCONFIGURED.to_string()),
                "case {case:?}: {projection} must report {FAILOVER_SLATE} as the \
                 unconfigured {UNCONFIGURED:?}, got {actual:?} in {channel}"
            );
        }
    }
}

/// Reading a channel does not change what a later caller sees: the projection is a
/// rendering, so repeated reads answer byte-identical channel objects and the stored
/// record is untouched.
#[tokio::test]
async fn the_projection_leaves_the_stored_channel_unchanged() {
    let ivs = Ivs::new();
    let arn = ivs.create_arn(json!({ "name": "stable" })).await;
    let stored = ivs.with_store(|store| store.channels.get(&arn).cloned());
    assert!(
        stored.is_some(),
        "case a created channel: the store holds it under {arn}"
    );

    let first = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();
    let second = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();
    assert_eq!(
        first, second,
        "case two reads of one channel: expected identical projections, got {first} then {second}"
    );
    let after = ivs.with_store(|store| store.channels.get(&arn).cloned());
    assert_eq!(
        format!("{stored:?}"),
        format!("{after:?}"),
        "case two reads of one channel: the stored record must be untouched, got {after:?}"
    );
}
