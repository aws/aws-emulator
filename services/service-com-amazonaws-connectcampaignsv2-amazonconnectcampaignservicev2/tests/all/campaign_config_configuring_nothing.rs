// SPDX-License-Identifier: CC0-1.0

//! A campaign configuration that configures NOTHING, end to end over the wire.
//!
//! Every member of `ChannelSubtypeConfig` (`telephony`, `sms`, `email`, `whatsApp`) and
//! of `CommunicationLimitsConfig` (`allChannelSubtypes`, `instanceLimitsHandling`) is
//! optional, and both members that carry them on `Campaign` are optional too, so a
//! request can carry a config holding no member at all. The rule under test: such a
//! config configures nothing, so the record carries NO member rather than an empty
//! structure — whichever call wrote it (`CreateCampaign`, the member's own update, or the
//! deletion that takes the last block out), which is what makes
//! `DescribeCampaign` -> update a round trip and lets a client branch on the member's
//! presence.
//!
//! The rows span what the rule generalises over: which of the two members is written,
//! which path writes it (create or update), how many members the config holds (none,
//! one, two, every one of them), which member that is — the channels from the model's
//! `ChannelSubtype` catalog (`CHANNEL_SUBTYPE_VALUES`) and the handling values from
//! `INSTANCE_LIMITS_HANDLING_VALUES` — and what the record held before the write.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    CHANNEL_SUBTYPE_VALUES, COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES,
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES, INSTANCE_LIMITS_HANDLING_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, create_campaign_with, describe_campaign, router, stored_config, Harness};

// The fixtures below are this suite's own, as every suite in this crate keeps its own:
// what they state is the shape ITS cases vary — how many members a configuration holds,
// which is what this suite is about.

/// The `ChannelSubtypeConfig` member that carries the block of one `ChannelSubtype`
/// value.
///
/// Source: the `ChannelSubtypeConfig` shape, whose per-channel members are `telephony`,
/// `sms`, `email` and `whatsApp`.
fn member_of(channel_subtype: &str) -> &'static str {
    match channel_subtype {
        "EMAIL" => "email",
        "SMS" => "sms",
        "TELEPHONY" => "telephony",
        "WHATSAPP" => "whatsApp",
        other => panic!("{other} is not a ChannelSubtype value"),
    }
}

/// The config block of one channel, in the minimal form its own shape requires:
/// `outboundMode` and `defaultOutboundConfig` on every channel block;
/// `connectContactFlowId` on `TelephonyOutboundConfig`; `connectSourcePhoneNumberArn`
/// and `wisdomTemplateArn` on `SmsOutboundConfig` and `WhatsAppOutboundConfig`;
/// `connectSourceEmailAddress` and `wisdomTemplateArn` on `EmailOutboundConfig`.
fn block(channel_subtype: &str) -> Value {
    match channel_subtype {
        "TELEPHONY" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }),
        "SMS" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
            }
        }),
        "EMAIL" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourceEmailAddress": "campaigns@example.com",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-2"
            }
        }),
        "WHATSAPP" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-2",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-3"
            }
        }),
        other => panic!("{other} is not a ChannelSubtype value"),
    }
}

/// A `channelSubtypeConfig` configuring each channel in `channels` — and, for an empty
/// `channels`, the config that configures no channel at all.
fn subtype_config(channels: &[&str]) -> Value {
    let mut config = json!({});
    for channel in channels {
        config[member_of(channel)] = block(channel);
    }
    config
}

/// One `CommunicationLimit` entry: every member of the shape is `@required`.
fn limit(max_count_per_recipient: i64, frequency: i64) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        // `CommunicationLimitTimeUnit`'s catalog, taken from the model.
        "unit": COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0],
    })
}

/// One of the two campaign members that carry a configuration whose every modelled
/// member is optional, with the update that replaces it.
struct ConfigMember {
    /// The member `DescribeCampaign` projects the config as, which `CreateCampaign`
    /// also takes it in.
    campaign_member: &'static str,
    /// The path of the update that replaces it, under `/v2/campaigns/{id}`.
    path_suffix: &'static str,
    /// The removal that takes one block out of it, as a query string under the same
    /// path — the other writer of the member.
    removal_query: String,
}

/// `channelSubtypeConfig`, replaced by `UpdateCampaignChannelSubtypeConfig` and reduced
/// by `DeleteCampaignChannelSubtypeConfig` one channel at a time.
fn channel_subtype_member(channel_subtype: &str) -> ConfigMember {
    ConfigMember {
        campaign_member: "channelSubtypeConfig",
        path_suffix: "channel-subtype-config",
        removal_query: format!("?channelSubtype={channel_subtype}"),
    }
}

/// `communicationLimitsOverride`, replaced by `UpdateCampaignCommunicationLimits` and
/// reduced by `DeleteCampaignCommunicationLimits` one config type at a time.
fn communication_limits_member(config_type: &str) -> ConfigMember {
    ConfigMember {
        campaign_member: "communicationLimitsOverride",
        path_suffix: "communication-limits",
        removal_query: format!("?config={config_type}"),
    }
}

/// The request body (or the `CreateCampaign` member) that carries `config` for
/// `member`: one object holding that member alone.
fn request_for(member: &ConfigMember, config: &Value) -> Value {
    let mut body = json!({});
    body[member.campaign_member] = config.clone();
    body
}

/// Write `config` onto `member` of the campaign `id` through that member's own update,
/// asserting it was accepted.
async fn update_member(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    member: &ConfigMember,
    config: &Value,
    case: &str,
) {
    let (status, body) = call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/{}", member.path_suffix),
        &request_for(member, config),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the update of {} expected 200, got {status} {body}",
        member.campaign_member
    );
}

/// Take one block out of `member` through that member's own deletion, asserting it was
/// accepted.
async fn delete_block(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    member: &ConfigMember,
    case: &str,
) {
    let (status, body) = call(
        router,
        region,
        "DELETE",
        &format!(
            "/v2/campaigns/{id}/{}{}",
            member.path_suffix, member.removal_query
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the deletion of {} expected 200, got {status} {body}",
        member.campaign_member
    );
}

/// The `channelSubtypes` `ListCampaigns` derives for the campaign `id`.
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

/// Assert the campaign `id` holds exactly what a write of `written` leaves on the
/// record: the config itself when it holds a member, and NO member at all when it holds
/// none.
async fn assert_record_holds(
    router: &Harness,
    region: &str,
    id: &str,
    member: &ConfigMember,
    written: &Value,
    case: &str,
) {
    let campaign = describe_campaign(router, region, id, case).await;
    let expected = stored_config(written);
    if expected.is_null() {
        assert!(
            campaign.get(member.campaign_member).is_none(),
            "case {case:?}: expected no {} on the record, got {:?}",
            member.campaign_member,
            campaign.get(member.campaign_member)
        );
    } else {
        assert_eq!(
            campaign[member.campaign_member], expected,
            "case {case:?}: expected the record to hold {expected}, got {}",
            campaign[member.campaign_member]
        );
    }
}

/// A configuration holding no member at all leaves the record carrying no member,
/// through BOTH paths a configuration reaches the record by, while one holding a member
/// is stored exactly as it was sent.
///
/// The rows span both members, the number of members a config holds (none, one, two,
/// every one), each entry of the model's `ChannelSubtype` catalog and each entry of
/// `InstanceLimitsHandling`.
#[tokio::test]
async fn a_config_configuring_nothing_leaves_the_record_holding_no_member() {
    let region = "us-west-2";
    let router = router();
    let instance = "config-configuring-nothing-instance";

    let mut rows: Vec<(String, ConfigMember, Value)> = vec![
        (
            "a channel subtype config configuring no channel".to_string(),
            channel_subtype_member(CHANNEL_SUBTYPE_VALUES[0]),
            subtype_config(&[]),
        ),
        (
            "a channel subtype config configuring two channels".to_string(),
            channel_subtype_member("SMS"),
            subtype_config(&["TELEPHONY", "SMS"]),
        ),
        (
            "a channel subtype config configuring every channel".to_string(),
            channel_subtype_member("EMAIL"),
            subtype_config(CHANNEL_SUBTYPE_VALUES),
        ),
        (
            "an override holding no member".to_string(),
            communication_limits_member(COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0]),
            json!({}),
        ),
        (
            "an override holding limits alone".to_string(),
            communication_limits_member(COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0]),
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(5, 7)]}}),
        ),
        (
            "an override holding both members, two limit entries".to_string(),
            communication_limits_member(COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0]),
            json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(1, 1), limit(9, 30)]},
                "instanceLimitsHandling": INSTANCE_LIMITS_HANDLING_VALUES[0]
            }),
        ),
    ];
    // One row per channel the model's catalog holds, so the rule is asserted on every
    // channel and not only on the ones the issue cited.
    for channel in CHANNEL_SUBTYPE_VALUES {
        rows.push((
            format!("a channel subtype config configuring {channel} alone"),
            channel_subtype_member(channel),
            subtype_config(&[channel]),
        ));
    }
    // And one per `InstanceLimitsHandling` value, each the only member its override
    // holds.
    for handling in INSTANCE_LIMITS_HANDLING_VALUES {
        rows.push((
            format!("an override holding instanceLimitsHandling {handling} alone"),
            communication_limits_member(COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0]),
            json!({"instanceLimitsHandling": handling}),
        ));
    }

    for (index, (case, member, config)) in rows.iter().enumerate() {
        // The create path: the campaign is born with the config the request carried.
        let created_case = format!("{case}, at CreateCampaign");
        let created = create_campaign_with(
            &router,
            region,
            instance,
            &format!("created-{index}"),
            request_for(member, config),
        )
        .await;
        assert_record_holds(&router, region, &created, member, config, &created_case).await;

        // The update path: a campaign that held the fixture's own configuration takes
        // the config instead.
        let updated_case = format!("{case}, through the update");
        let updated = create_campaign_with(
            &router,
            region,
            instance,
            &format!("updated-{index}"),
            json!({}),
        )
        .await;
        update_member(&router, region, &updated, member, config, &updated_case).await;
        assert_record_holds(&router, region, &updated, member, config, &updated_case).await;

        // Neither path can leave a state the other would not: the two records agree on
        // the member.
        let from_create = describe_campaign(&router, region, &created, &created_case).await;
        let from_update = describe_campaign(&router, region, &updated, &updated_case).await;
        assert_eq!(
            from_create.get(member.campaign_member),
            from_update.get(member.campaign_member),
            "case {case:?}: expected the create and the update to leave the same {}, \
             got {:?} and {:?}",
            member.campaign_member,
            from_create.get(member.campaign_member),
            from_update.get(member.campaign_member)
        );

        // `ListCampaigns` derives `channelSubtypes` from the same record, so a campaign
        // configured for no channel is listed with no channel subtype.
        if member.campaign_member == "channelSubtypeConfig" {
            let expected: Vec<String> = CHANNEL_SUBTYPE_VALUES
                .iter()
                .filter(|channel| config.get(member_of(channel)).is_some())
                .map(|channel| (*channel).to_string())
                .collect();
            for (id, case) in [(&created, &created_case), (&updated, &updated_case)] {
                let listed = listed_channel_subtypes(&router, region, id, case).await;
                assert_eq!(
                    listed, expected,
                    "case {case:?}: expected channelSubtypes {expected:?}, got {listed:?}"
                );
            }
        }

        // The round trip a client makes: whatever the record projects, written back
        // through the update, leaves the record exactly as it was — and a record holding
        // no member projects none, which is the config holding no member.
        let projected = from_update
            .get(member.campaign_member)
            .cloned()
            .unwrap_or_else(|| json!({}));
        let round_trip_case = format!("{case}, Describe -> update round trip");
        update_member(
            &router,
            region,
            &updated,
            member,
            &projected,
            &round_trip_case,
        )
        .await;
        assert_record_holds(
            &router,
            region,
            &updated,
            member,
            &projected,
            &round_trip_case,
        )
        .await;
    }
}

/// The update and the removal leave the SAME record: a config reduced by an update
/// that carries only the blocks that are to survive and one reduced by the deletion of
/// a block are indistinguishable, member for member, through `DescribeCampaign` and
/// `ListCampaigns`.
///
/// Driven over every entry of the model's `ChannelSubtype` catalog and of
/// `CommunicationLimitsConfigType`, so each removal the service models is compared with
/// the update that reaches the same meaning. A channel-subtype row keeps a second
/// channel configured: a campaign always keeps at least one channel subtype, so the
/// removal of the LAST block is refused rather than emptying the config (which
/// `campaign_channel_subtype_config_deletion` owns), while an override emptied of its
/// last member is the rule above.
#[tokio::test]
async fn an_emptied_config_is_the_same_record_whichever_call_emptied_it() {
    let region = "eu-central-1";
    let router = router();
    let instance = "emptied-config-instance";

    // (case, the member and its removal, the config the campaign is created with, the
    //  config an update writes to reach what the removal leaves)
    let mut rows: Vec<(String, ConfigMember, Value, Value)> = Vec::new();
    for channel in CHANNEL_SUBTYPE_VALUES {
        // A second channel, so the removal is not of the campaign's last block.
        let survivor = CHANNEL_SUBTYPE_VALUES
            .iter()
            .copied()
            .find(|other| other != channel)
            .expect("the catalog holds more than one channel");
        rows.push((
            format!("one channel block of two, {channel}"),
            channel_subtype_member(channel),
            subtype_config(&[channel, survivor]),
            subtype_config(&[survivor]),
        ));
    }
    for config_type in COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES {
        rows.push((
            format!("the only override member, {config_type}"),
            communication_limits_member(config_type),
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(4, 3)]}}),
            json!({}),
        ));
    }

    for (index, (case, member, config, reduced)) in rows.iter().enumerate() {
        let extra = request_for(member, config);
        let reduced_by_update = create_campaign_with(
            &router,
            region,
            instance,
            &format!("by-update-{index}"),
            extra.clone(),
        )
        .await;
        let reduced_by_removal = create_campaign_with(
            &router,
            region,
            instance,
            &format!("by-removal-{index}"),
            extra,
        )
        .await;

        // One campaign is reduced by an update carrying what is to survive, the other
        // by the deletion of the block that is to go.
        update_member(&router, region, &reduced_by_update, member, reduced, case).await;
        delete_block(&router, region, &reduced_by_removal, member, case).await;

        let mut from_update = describe_campaign(&router, region, &reduced_by_update, case).await;
        let mut from_removal = describe_campaign(&router, region, &reduced_by_removal, case).await;
        let expected = stored_config(reduced);
        for projection in [&mut from_update, &mut from_removal] {
            let members = projection
                .as_object_mut()
                .expect("a campaign projection is an object");
            let held = members.get(member.campaign_member).cloned();
            if expected.is_null() {
                assert!(
                    held.is_none(),
                    "case {case:?}: expected no {}, got {held:?}",
                    member.campaign_member
                );
            } else {
                assert_eq!(
                    held.as_ref(),
                    Some(&expected),
                    "case {case:?}: expected {} to hold {expected}, got {held:?}",
                    member.campaign_member
                );
            }
            // The server-owned identity differs between the two campaigns by
            // construction; everything else must agree. The campaign's own
            // last-modified stamp is server-owned in the same way: these are two
            // records, written by two calls at two instants. It is read and asserted
            // present rather than ignored, and which calls move it is pinned by
            // `campaign_configuration_is_one_rule.rs`.
            assert!(
                members["lastModifiedTime"].is_string(),
                "case {case:?}: expected a lastModifiedTime, got {members:?}"
            );
            for identity in ["id", "arn", "name", "lastModifiedTime"] {
                members.remove(identity);
            }
        }
        assert_eq!(
            from_update, from_removal,
            "case {case:?}: expected the update and the removal to leave the same record, \
             got {from_update} and {from_removal}"
        );

        let listed_after_update =
            listed_channel_subtypes(&router, region, &reduced_by_update, case).await;
        let listed_after_removal =
            listed_channel_subtypes(&router, region, &reduced_by_removal, case).await;
        assert_eq!(
            listed_after_update, listed_after_removal,
            "case {case:?}: expected the same channelSubtypes, got {listed_after_update:?} \
             and {listed_after_removal:?}"
        );
    }
}
