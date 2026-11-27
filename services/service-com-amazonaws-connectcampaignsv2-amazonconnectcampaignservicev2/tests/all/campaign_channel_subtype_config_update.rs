// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignChannelSubtypeConfig`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call sets the campaign `{id}` names to the `channelSubtypeConfig` the request
//!   carries and REPLACES whatever it held — the config is one modelled structure, so a
//!   channel the new config leaves out is a channel the campaign is no longer
//!   configured for — which is what every later read projects (`DescribeCampaign`, and
//!   `ListCampaigns`' derived `channelSubtypes`), while every other member of the
//!   campaign stays as it was;
//! * every member of the config is optional, so a config configuring NO channel is a
//!   legal body: the campaign then holds no `channelSubtypeConfig` at all, the one
//!   representation of "configured for no channel" the removal path leaves behind too;
//! * the operation is idempotent (`@idempotent` in the model, "This API is idempotent"
//!   in its documentation): the same config sent twice leaves the campaign exactly as
//!   the first call left it;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and stores
//!   nothing — including an id that names a campaign of ANOTHER region, which this
//!   region's endpoint does not reach;
//! * the update turns on the campaign existing, not on where its lifecycle is: this
//!   operation models no `InvalidCampaignStateException`, so every state the campaign
//!   lifecycle reaches takes it.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::CHANNEL_SUBTYPE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, CAMPAIGN_NOT_FOUND_MESSAGE, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_without, error_code, router, stored_config, Harness};

// The fixtures below are this suite's own, as every suite in this crate keeps its own
// (`campaign_communication_time_update`, `campaign_communication_limits`,
// `campaign_lifecycle`, …): `tests/all/common.rs` is the shared HTTP harness, and a
// suite's fixtures state the shape ITS cases vary — here the forms a
// `ChannelSubtypeConfig` takes ACROSS an update, which is what this suite is about.

/// The `ChannelSubtypeConfig` member that carries the block of one `ChannelSubtype`
/// value.
///
/// Source: the `ChannelSubtypeConfig` shape, whose per-channel members are
/// `telephony`, `sms`, `email` and `whatsApp`.
fn member_of(channel_subtype: &str) -> &'static str {
    match channel_subtype {
        "EMAIL" => "email",
        "SMS" => "sms",
        "TELEPHONY" => "telephony",
        "WHATSAPP" => "whatsApp",
        other => panic!("{other} is not a ChannelSubtype value"),
    }
}

/// A `TelephonyOutboundMode` — the union whose member decides how a telephony block
/// dials — in a form that varies with `shape`.
///
/// Source: the `TelephonyOutboundMode` union (`agentless`, `progressive`,
/// `predictive`, `preview`) and the configs its members target:
/// `BandwidthAllocation` (range 0..2), `PacingStrategyList` (length 1..1) carrying an
/// `AbandonmentRatePacingConfig` (`targetRate` range 0..1, `connectionStartPoint` one
/// of `CONNECTED_TO_SYSTEM` | `GREETING_START` | `GREETING_END`,
/// `connectionThresholdSeconds` min 1, `evaluationWindow` pattern `^PT([1-9]|1[0-9]|2[0-4])H$`),
/// `TimeoutDuration` (range 1..300) and `AgentActions` (`DISCARD`).
fn telephony_outbound_mode(shape: usize) -> Value {
    match shape % 5 {
        0 => json!({"agentless": {}}),
        1 => json!({"progressive": {"bandwidthAllocation": 0.5}}),
        2 => json!({"predictive": {"bandwidthAllocation": 2.0}}),
        3 => json!({
            "predictive": {
                "bandwidthAllocation": 0.25,
                "pacingStrategies": [{
                    "abandonmentRate": {
                        "targetRate": 0.05,
                        "connectionStartPoint": "GREETING_END",
                        "connectionThresholdSeconds": 10,
                        "evaluationWindow": "PT4H"
                    }
                }]
            }
        }),
        _ => json!({
            "preview": {
                "bandwidthAllocation": 0.75,
                "timeoutConfig": {"durationInSeconds": 300},
                "agentActions": ["DISCARD"]
            }
        }),
    }
}

/// The config block of one channel, in a form that varies with `shape`: which outbound
/// mode a telephony block runs, whether it carries the optional `capacity` (`Capacity`,
/// range 0.01..1) and which optional members of its `defaultOutboundConfig` it fills.
///
/// The required members come from each channel's own shape: `outboundMode` and
/// `defaultOutboundConfig` on every channel block; `connectContactFlowId` on
/// `TelephonyOutboundConfig`; `connectSourcePhoneNumberArn` and `wisdomTemplateArn` on
/// `SmsOutboundConfig` and `WhatsAppOutboundConfig`; `connectSourceEmailAddress` and
/// `wisdomTemplateArn` on `EmailOutboundConfig`.
fn block(channel_subtype: &str, shape: usize) -> Value {
    let mut block = match channel_subtype {
        "TELEPHONY" => {
            let mut telephony = json!({
                "outboundMode": telephony_outbound_mode(shape),
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            });
            if shape % 2 == 1 {
                telephony["connectQueueId"] = json!("queue-0001");
                telephony["defaultOutboundConfig"]["ringTimeout"] = json!(15);
                telephony["defaultOutboundConfig"]["connectSourcePhoneNumber"] = json!("+15550100");
                telephony["defaultOutboundConfig"]["answerMachineDetectionConfig"] = json!({
                    "enableAnswerMachineDetection": true,
                    "awaitAnswerMachinePrompt": false
                });
            }
            telephony
        }
        "SMS" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
            }
        }),
        "EMAIL" => {
            let mut email = json!({
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {
                    "connectSourceEmailAddress": "campaigns@example.com",
                    "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-2"
                }
            });
            if shape.is_multiple_of(3) {
                email["defaultOutboundConfig"]["sourceEmailAddressDisplayName"] =
                    json!("Campaigns");
            }
            email
        }
        "WHATSAPP" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-2",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-3"
            }
        }),
        other => panic!("{other} is not a ChannelSubtype value"),
    };
    // `capacity` is optional on every channel block, inside `Capacity`'s 0.01..1
    // range; the rows differ in whether it is set and at what value — including each
    // published bound.
    if shape % 3 == 1 {
        block["capacity"] = json!(0.01);
    } else if shape % 3 == 2 {
        block["capacity"] = json!(1.0);
    }
    block
}

/// A `channelSubtypeConfig` configuring each channel in `channels` (each a
/// `ChannelSubtype` value), every block in a form of its own, rooted at `shape`.
fn subtype_config(channels: &[&str], shape: usize) -> Value {
    let mut config = json!({});
    for (offset, channel) in channels.iter().enumerate() {
        config[member_of(channel)] = block(channel, shape + offset);
    }
    config
}

/// Onboard `instance` and wait out the span the clock owes its onboarding job, so the
/// instance is a campaign target by the time this returns: `CreateCampaign` mints a
/// campaign only for an instance whose job has SUCCEEDED. Nothing in the emulator
/// sleeps for that span — only a caller that wants the settled state does.
async fn onboard(router: &Harness, region: &str, instance: &str) {
    // Onboarding resolves the `connectInstanceId` against the account's Connect
    // instance registry, so the instance is registered there first.
    router.register_connect_instance(region, instance);
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
}

/// Create a campaign on an onboarded `instance`, optionally carrying
/// `channel_subtype_config`, and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    channel_subtype_config: Option<&Value>,
) -> String {
    let mut body = json!({"name": name, "connectInstanceId": instance});
    if let Some(config) = channel_subtype_config {
        body["channelSubtypeConfig"] = config.clone();
    }
    let (status, response) = call(router, region, "PUT", "/v2/campaigns", &body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCampaign expected 200, got {status} {response}"
    );
    response["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCampaign returned no id: {response}"))
        .to_string()
}

/// `UpdateCampaignChannelSubtypeConfig` for `id`, carrying `config`.
async fn update_subtype_config(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/channel-subtype-config"),
        &json!({"channelSubtypeConfig": config}),
    )
    .await
}

/// The `campaign` object `DescribeCampaign` projects for `id`, asserting the read
/// succeeded.
async fn describe(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCampaign expected 200, got {status} {body}"
    );
    body["campaign"].clone()
}

/// The `channelSubtypes` `ListCampaigns` derives for the first summary it returns.
async fn listed_channel_subtypes(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> Vec<String> {
    let (status, listed) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {listed}"
    );
    let summaries = listed["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {listed}"));
    let summary = summaries
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {id}: {listed}"));
    summary["channelSubtypes"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `ChannelSubtype` values a config configures, in the catalog's own order — what
/// `ListCampaigns` derives from the stored config.
fn configured_subtypes(config: &Value) -> Vec<String> {
    CHANNEL_SUBTYPE_VALUES
        .iter()
        .filter(|channel| config.get(member_of(channel)).is_some())
        .map(|channel| (*channel).to_string())
        .collect()
}

/// One row of the replacement table in
/// [`an_update_replaces_the_whole_channel_subtype_config`].
struct Row<'a> {
    /// What an assertion calls this row.
    name: &'a str,
    /// The channels the campaign is created with, or `None` for a campaign created
    /// with no channel subtype config at all.
    created: Option<Vec<&'a str>>,
    /// The channels the update's config configures.
    updated: Vec<&'a str>,
    /// Which form each block of the update's config takes.
    shape: usize,
}

/// The update stores the config it was given, whole: every later read projects exactly
/// that config, a channel the new config leaves out is no longer configured, and every
/// other member of the campaign is untouched.
///
/// The table's rows differ along what the rule generalises over: which channels the
/// stored config configures, which the new one configures (more, fewer, the same, none
/// at all), and the form of each block — the telephony outbound-mode union member, the
/// optional `capacity` at each published bound, and the optional members of each
/// `defaultOutboundConfig` — including a campaign that started with no channel subtype
/// config at all.
#[tokio::test]
async fn an_update_replaces_the_whole_channel_subtype_config() {
    let region = "us-west-2";
    let router = router();
    let instance = "channel-subtype-update-instance";
    onboard(&router, region, instance).await;

    let every_channel = CHANNEL_SUBTYPE_VALUES.to_vec();

    let cases: Vec<Row> = vec![
        Row {
            name: "no config at all, then every channel",
            created: None,
            updated: every_channel.clone(),
            shape: 0,
        },
        Row {
            name: "every channel, then one channel",
            created: Some(every_channel.clone()),
            updated: vec!["SMS"],
            shape: 1,
        },
        Row {
            name: "one channel, then every channel",
            created: Some(vec!["TELEPHONY"]),
            updated: every_channel.clone(),
            shape: 2,
        },
        Row {
            name: "two channels, then two other channels",
            created: Some(vec!["EMAIL", "SMS"]),
            updated: vec!["TELEPHONY", "WHATSAPP"],
            shape: 3,
        },
        Row {
            name: "one channel, then the same channel in another form",
            created: Some(vec!["TELEPHONY"]),
            updated: vec!["TELEPHONY"],
            shape: 4,
        },
        Row {
            name: "every channel, then a config configuring none",
            created: Some(every_channel.clone()),
            updated: Vec::new(),
            shape: 5,
        },
        Row {
            name: "a config configuring none, then one channel",
            created: Some(Vec::new()),
            updated: vec!["WHATSAPP"],
            shape: 6,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let Row {
            name: case,
            created: created_channels,
            updated: updated_channels,
            shape,
        } = row;
        let created = created_channels
            .as_ref()
            .map(|channels| subtype_config(channels, index));
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("campaign-{index}"),
            created.as_ref(),
        )
        .await;
        let before = describe(&router, region, &id, case).await;
        let expected_created = created.as_ref().map_or(Value::Null, stored_config);
        assert_eq!(
            before["channelSubtypeConfig"], expected_created,
            "case {case:?}: expected the created config stored as {expected_created}, got {:?}",
            before.get("channelSubtypeConfig")
        );

        let updated = subtype_config(updated_channels, *shape);
        let (status, body) = update_subtype_config(&router, region, &id, &updated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignChannelSubtypeConfig expected 200, got {status} {body}"
        );

        // The stored config is the one the request carried, and nothing of the old one
        // survives: a channel the update left out holds no block. A config configuring
        // no channel at all configures nothing, so the record then carries no
        // `channelSubtypeConfig` rather than an empty one.
        let after = describe(&router, region, &id, case).await;
        let expected = stored_config(&updated);
        assert_eq!(
            after["channelSubtypeConfig"], expected,
            "case {case:?}: expected {expected}, got {:?}",
            after["channelSubtypeConfig"]
        );
        for channel in CHANNEL_SUBTYPE_VALUES {
            let expected_block = updated.get(member_of(channel));
            assert_eq!(
                after["channelSubtypeConfig"].get(member_of(channel)),
                expected_block,
                "case {case:?}: expected the {} block to be {expected_block:?}, got {:?}",
                member_of(channel),
                after["channelSubtypeConfig"].get(member_of(channel))
            );
        }

        // `ListCampaigns`' `channelSubtypes` is derived from the same stored config, so
        // it reports the updated channels in the catalog's order.
        let expected_subtypes = configured_subtypes(&updated);
        let listed = listed_channel_subtypes(&router, region, &id, case).await;
        assert_eq!(
            listed, expected_subtypes,
            "case {case:?}: expected channelSubtypes {expected_subtypes:?}, got {listed:?}"
        );

        // Idempotent: the same config sent again is a 200 and the same stored config.
        let (status, body) = update_subtype_config(&router, region, &id, &updated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated update expected 200, got {status} {body}"
        );
        let repeated = describe(&router, region, &id, case).await;
        assert_eq!(
            repeated["channelSubtypeConfig"], expected,
            "case {case:?}: expected the repeat to leave {expected}, got {:?}",
            repeated["channelSubtypeConfig"]
        );

        // The call changes the channel subtype config and NOTHING else about the
        // campaign.
        let before_rest = campaign_without(&before, "channelSubtypeConfig");
        let after_rest = campaign_without(&repeated, "channelSubtypeConfig");
        assert_eq!(
            before_rest, after_rest,
            "case {case:?}: expected every other member untouched, got {after_rest} \
             (was {before_rest})"
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found text,
/// and stores nothing — asserted over a table of well-formed `CampaignId`s the store
/// never held, one it held until `DeleteCampaign`, and one that names a campaign of
/// another region.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_takes_no_config() {
    let region = "eu-west-1";
    let other_region = "eu-central-1";
    let router = router();
    let instance = "channel-subtype-update-404-instance";
    onboard(&router, region, instance).await;
    onboard(&router, other_region, instance).await;

    // A campaign that stays in the store, so the 404s below are the lookup failing and
    // not the operation being broken.
    let kept = subtype_config(CHANNEL_SUBTYPE_VALUES, 1);
    let control = create_campaign(&router, region, instance, "control", Some(&kept)).await;

    // One campaign created and then deleted: its id was valid a moment ago.
    let deleted = create_campaign(&router, region, instance, "doomed", Some(&kept)).await;
    let (status, body) = call(
        &router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{deleted}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete the doomed campaign\": expected 200, got {status} {body}"
    );

    // One campaign of ANOTHER region, which this region's endpoint does not reach.
    let elsewhere = create_campaign(&router, other_region, instance, "elsewhere", Some(&kept)).await;

    // (case, id) — ids spanning `CampaignId` (len 1..256, pattern ^[-:/a-zA-Z0-9]+$).
    let cases: Vec<(String, String)> = vec![
        ("never created".to_string(), "missing".to_string()),
        ("single character".to_string(), "0".to_string()),
        (
            // `/` is left out: a `@httpLabel` is one path segment, so an id carrying
            // one does not address this operation's URI at all.
            "every character class the pattern admits".to_string(),
            "A-b:C1".to_string(),
        ),
        ("the 256-character maximum".to_string(), "c".repeat(256)),
        ("created and then deleted".to_string(), deleted.clone()),
        ("a campaign of another region".to_string(), elsewhere.clone()),
    ];

    let sent = subtype_config(&["TELEPHONY"], 3);
    for (case, id) in cases {
        let (status, body) = update_subtype_config(&router, region, &id, &sent).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body["message"].as_str(),
            Some(CAMPAIGN_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the fixed not-found text, got {:?}",
            body["message"]
        );

        // A refused call stores nothing: the campaign that does exist keeps the config
        // it was created with, in this region and in the other.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], kept,
            "case {case:?}: expected the control campaign's config untouched, got {:?}",
            campaign["channelSubtypeConfig"]
        );
        let campaign = describe(&router, other_region, &elsewhere, &case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], kept,
            "case {case:?}: expected the other region's campaign untouched, got {:?}",
            campaign["channelSubtypeConfig"]
        );
    }
}

/// A config outside the constraints the model states for `ChannelSubtypeConfig` is a
/// bad request, and stores nothing: the campaign keeps the config it already held.
///
/// The rows differ along what the rule generalises over — each numeric range one step
/// past each published bound, a list longer than its `@length`, a value outside an
/// enum's catalog, a string outside its `@pattern`, and a block missing a `@required`
/// member — and the bounds themselves are asserted to be ACCEPTED, so the refusal is
/// the constraint and not the handler refusing configs at large.
///
/// Sources (the `UpdateCampaignChannelSubtypeConfig` closure): `Capacity` range
/// 0.01..1; `BandwidthAllocation` range 0..2; `RingTimeout` range 15..60;
/// `TimeoutDuration` range 1..300; `PacingStrategyList` length 1..1; `AgentAction`
/// values `DISCARD`; `EvaluationWindow` pattern `^PT([1-9]|1[0-9]|2[0-4])H$`;
/// `TelephonyChannelSubtypeConfig$outboundMode` required.
#[tokio::test]
async fn a_config_outside_the_modelled_constraints_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "channel-subtype-update-validation-instance";
    onboard(&router, region, instance).await;

    let kept = subtype_config(CHANNEL_SUBTYPE_VALUES, 2);
    let id = create_campaign(&router, region, instance, "validated", Some(&kept)).await;

    /// A telephony block running `outbound_mode`, with no optional member set.
    fn telephony(outbound_mode: Value) -> Value {
        json!({
            "telephony": {
                "outboundMode": outbound_mode,
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            }
        })
    }

    /// An `abandonmentRate` pacing strategy with the given rate and window.
    fn abandonment_rate(target_rate: f64, threshold: i64, window: &str) -> Value {
        json!({
            "abandonmentRate": {
                "targetRate": target_rate,
                "connectionStartPoint": "CONNECTED_TO_SYSTEM",
                "connectionThresholdSeconds": threshold,
                "evaluationWindow": window
            }
        })
    }

    // (case, the config the update carries) — every row a bad request.
    let refused: Vec<(&str, Value)> = vec![
        ("capacity one step below the 0.01 minimum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["capacity"] = json!(0.0);
            config
        }),
        ("capacity one step above the 1 maximum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["capacity"] = json!(1.01);
            config
        }),
        (
            "bandwidthAllocation one step below the 0 minimum",
            telephony(json!({"progressive": {"bandwidthAllocation": -0.5}})),
        ),
        (
            "bandwidthAllocation one step above the 2 maximum",
            telephony(json!({"predictive": {"bandwidthAllocation": 2.5}})),
        ),
        ("ringTimeout one step below the 15 minimum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["defaultOutboundConfig"]["ringTimeout"] = json!(14);
            config
        }),
        ("ringTimeout one step above the 60 maximum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["defaultOutboundConfig"]["ringTimeout"] = json!(61);
            config
        }),
        (
            "a preview timeout one step below the 1 second minimum",
            telephony(json!({
                "preview": {
                    "bandwidthAllocation": 1.0,
                    "timeoutConfig": {"durationInSeconds": 0}
                }
            })),
        ),
        (
            "a preview timeout one step above the 300 second maximum",
            telephony(json!({
                "preview": {
                    "bandwidthAllocation": 1.0,
                    "timeoutConfig": {"durationInSeconds": 301}
                }
            })),
        ),
        (
            "two pacing strategies where the list holds one",
            telephony(json!({
                "predictive": {
                    "bandwidthAllocation": 1.0,
                    "pacingStrategies": [
                        abandonment_rate(0.05, 10, "PT4H"),
                        abandonment_rate(0.1, 20, "PT8H")
                    ]
                }
            })),
        ),
        (
            "an agent action the catalog does not hold",
            telephony(json!({
                "preview": {
                    "bandwidthAllocation": 1.0,
                    "timeoutConfig": {"durationInSeconds": 30},
                    "agentActions": ["KEEP"]
                }
            })),
        ),
        (
            "an evaluationWindow the pattern does not admit",
            telephony(json!({
                "predictive": {
                    "bandwidthAllocation": 1.0,
                    "pacingStrategies": [abandonment_rate(0.05, 10, "PT25H")]
                }
            })),
        ),
        (
            "a block carrying no outboundMode",
            json!({
                "telephony": {
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            }),
        ),
        (
            "an email address outside the address pattern",
            json!({
                "email": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {
                        "connectSourceEmailAddress": "campaigns-at-example.com",
                        "wisdomTemplateArn":
                            "arn:aws:wisdom:us-east-1:000000000000:template/t-2"
                    }
                }
            }),
        ),
        (
            "a wisdomTemplateArn below the 20-character minimum",
            json!({
                "sms": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {
                        "connectSourcePhoneNumberArn":
                            "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                        "wisdomTemplateArn": "arn:aws:wisdom:x"
                    }
                }
            }),
        ),
    ];

    for (case, config) in &refused {
        let (status, response) = update_subtype_config(&router, region, &id, config).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&response)
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], kept,
            "case {case:?}: expected the refusal to leave the config alone, got {:?}",
            campaign["channelSubtypeConfig"]
        );
    }

    // (case, a config sitting exactly ON a published bound) — every row accepted and
    // stored, so each range is refused only outside itself.
    let accepted: Vec<(&str, Value)> = vec![
        ("capacity at the 0.01 minimum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["capacity"] = json!(0.01);
            config
        }),
        ("capacity at the 1 maximum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["capacity"] = json!(1.0);
            config
        }),
        (
            "bandwidthAllocation at the 0 minimum",
            telephony(json!({"progressive": {"bandwidthAllocation": 0.0}})),
        ),
        (
            "bandwidthAllocation at the 2 maximum",
            telephony(json!({"progressive": {"bandwidthAllocation": 2.0}})),
        ),
        ("ringTimeout at the 15 minimum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["defaultOutboundConfig"]["ringTimeout"] = json!(15);
            config
        }),
        ("ringTimeout at the 60 maximum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["defaultOutboundConfig"]["ringTimeout"] = json!(60);
            config
        }),
        (
            "a preview timeout at the 1 second minimum",
            telephony(json!({
                "preview": {
                    "bandwidthAllocation": 1.0,
                    "timeoutConfig": {"durationInSeconds": 1}
                }
            })),
        ),
        (
            "a preview timeout at the 300 second maximum, discarding",
            telephony(json!({
                "preview": {
                    "bandwidthAllocation": 1.0,
                    "timeoutConfig": {"durationInSeconds": 300},
                    "agentActions": ["DISCARD"]
                }
            })),
        ),
        (
            "a pacing strategy at the lowest target rate and window",
            telephony(json!({
                "predictive": {
                    "bandwidthAllocation": 1.0,
                    "pacingStrategies": [abandonment_rate(0.0, 1, "PT1H")]
                }
            })),
        ),
        (
            "a pacing strategy at the highest target rate and window",
            telephony(json!({
                "predictive": {
                    "bandwidthAllocation": 1.0,
                    "pacingStrategies": [abandonment_rate(1.0, 600, "PT24H")]
                }
            })),
        ),
    ];

    for (case, config) in &accepted {
        let (status, response) = update_subtype_config(&router, region, &id, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {response}"
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            &campaign["channelSubtypeConfig"], config,
            "case {case:?}: expected {config}, got {:?}",
            campaign["channelSubtypeConfig"]
        );
    }
}

/// The update turns on the campaign existing, not on where its lifecycle is: this
/// operation models no `InvalidCampaignStateException`, so the table walks the campaign
/// through every state its lifecycle calls reach and writes a different config in each.
#[tokio::test]
async fn every_state_the_lifecycle_reaches_takes_the_update() {
    let region = "ap-south-1";
    let router = router();
    let instance = "channel-subtype-update-state-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    // Each lifecycle action the emulator's state machine offers, the state it leaves
    // the campaign in, and the channel the config written there configures.
    let walk: &[(&str, CampaignState, &str)] = &[
        ("", CampaignState::Initialized, "EMAIL"),
        ("start", CampaignState::Running, "SMS"),
        ("pause", CampaignState::Paused, "TELEPHONY"),
        ("stop", CampaignState::Stopped, "WHATSAPP"),
    ];
    for (shape, (action, state, channel)) in walk.iter().enumerate() {
        if !action.is_empty() {
            let (status, body) = call(
                &router,
                region,
                "POST",
                &format!("/v2/campaigns/{id}/{action}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {action:?}: expected the lifecycle action to succeed, got {status} {body}"
            );
        }
        let case = format!("in state {}", state.as_wire());
        let (status, read) = call(
            &router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}/state"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCampaignState expected 200, got {status} {read}"
        );
        assert_eq!(
            read["state"].as_str(),
            Some(state.as_wire()),
            "case {case:?}: expected state {:?}, got {:?}",
            state.as_wire(),
            read["state"]
        );

        let config = subtype_config(&[channel], shape);
        let (status, body) = update_subtype_config(&router, region, &id, &config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], config,
            "case {case:?}: expected {config}, got {:?}",
            campaign["channelSubtypeConfig"]
        );
    }
}
