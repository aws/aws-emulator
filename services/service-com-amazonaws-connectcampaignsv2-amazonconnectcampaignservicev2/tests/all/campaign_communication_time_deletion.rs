// SPDX-License-Identifier: CC0-1.0

//! `DeleteCampaignCommunicationTime`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call removes the communication time window the campaign `{id}` names holds
//!   for the ONE channel `config` names, and nothing else: the config's required
//!   `localTimeZoneConfig` and every other channel's window stay as they were, and so
//!   does every other member of the campaign — which is what every later read
//!   (`DescribeCampaign`) projects;
//! * the operation is idempotent (`@idempotent` in the model, "This API is
//!   idempotent" in its documentation): a channel that holds no window, and a
//!   campaign carrying no communication time config at all, are a 200 that removes
//!   nothing;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and
//!   removes nothing;
//! * a `config` outside the model's `CommunicationTimeConfigType` catalog is HTTP 400
//!   `ValidationException` and removes nothing;
//! * a campaign in a non-terminal lifecycle state takes the deletion.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    COMMUNICATION_TIME_CONFIG_TYPE_VALUES, DAY_OF_WEEK_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, CAMPAIGN_NOT_FOUND_MESSAGE, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_without, error_code, router, Harness};

/// The `CommunicationTimeConfig` member that carries the time window of one
/// `CommunicationTimeConfigType` value.
///
/// Source: the `CommunicationTimeConfig` shape, whose per-channel members are
/// `telephony`, `sms`, `email` and `whatsApp`.
fn member_of(config_type: &str) -> &'static str {
    match config_type {
        "EMAIL" => "email",
        "SMS" => "sms",
        "TELEPHONY" => "telephony",
        "WHATSAPP" => "whatsApp",
        other => panic!("{other} is not a CommunicationTimeConfigType value"),
    }
}

/// A `TimeWindow` whose form varies with `shape`: how many days of `dailyHours` it
/// opens, how many ranges each day holds and whether it carries `restrictedPeriods`.
///
/// Day names come from the model's own `DayOfWeek` catalog (`DAY_OF_WEEK_VALUES`),
/// and the times are `Iso8601Time` (`^T\d{2}:\d{2}$`).
fn window(shape: usize) -> Value {
    let days = 1 + shape % DAY_OF_WEEK_VALUES.len();
    let mut daily_hours = serde_json::Map::new();
    for (index, day) in DAY_OF_WEEK_VALUES.iter().take(days).enumerate() {
        let mut ranges = vec![json!({"startTime": "T09:00", "endTime": "T12:00"})];
        if index % 2 == 0 {
            ranges.push(json!({"startTime": "T13:00", "endTime": "T17:00"}));
        }
        daily_hours.insert((*day).to_string(), Value::Array(ranges));
    }
    let mut time_window = json!({"openHours": {"dailyHours": daily_hours}});
    if shape % 2 == 1 {
        time_window["restrictedPeriods"] = json!({
            "restrictedPeriodList": [
                {"name": "holiday", "startDate": "2026-12-24", "endDate": "2026-12-26"}
            ]
        });
    }
    time_window
}

/// A `communicationTimeConfig` opening `channels` (each a
/// `CommunicationTimeConfigType` value), rooted in `time_zone`.
fn time_config(channels: &[&str], time_zone: &str) -> Value {
    let mut config = json!({"localTimeZoneConfig": {"defaultTimeZone": time_zone}});
    for (shape, channel) in channels.iter().enumerate() {
        config[member_of(channel)] = window(shape);
    }
    config
}

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_subtype_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
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
/// `communication_time_config`, and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    communication_time_config: Option<&Value>,
) -> String {
    let mut body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_subtype_config(),
    });
    if let Some(config) = communication_time_config {
        body["communicationTimeConfig"] = config.clone();
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

/// `DeleteCampaignCommunicationTime` for `id` and the `config` channel.
async fn delete_window(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{id}/communication-time?config={config}"),
        &Value::Null,
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

/// Deleting one channel's window removes exactly that window, over a TABLE spanning
/// the whole `CommunicationTimeConfigType` catalog
/// (`COMMUNICATION_TIME_CONFIG_TYPE_VALUES`, taken from the model) against configs
/// that differ in which channels they open and in the form of each window.
#[tokio::test]
async fn deleting_one_channel_leaves_every_other_part_of_the_config() {
    let region = "us-west-2";
    let router = router();
    let instance = "communication-time-instance";
    onboard(&router, region, instance).await;

    // (case, the channels the created config opens)
    let configurations: Vec<(&str, Vec<&str>)> = vec![
        ("every channel open", COMMUNICATION_TIME_CONFIG_TYPE_VALUES.to_vec()),
        ("two channels open", vec!["TELEPHONY", "WHATSAPP"]),
        ("one channel open", vec!["SMS"]),
        ("no channel open, time zone only", Vec::new()),
    ];

    for (shape, (configuration, channels)) in configurations.iter().enumerate() {
        // Every catalog value is deleted against every configuration, including the
        // ones that never held a window for it.
        for target in COMMUNICATION_TIME_CONFIG_TYPE_VALUES {
            let case = format!("{configuration}, delete {target}");
            let created = time_config(channels, "America/New_York");
            let id = create_campaign(
                &router,
                region,
                instance,
                &format!("campaign-{shape}-{target}"),
                Some(&created),
            )
            .await;
            let before = describe(&router, region, &id, &case).await;
            assert_eq!(
                before["communicationTimeConfig"], created,
                "case {case:?}: expected the created config stored, got {:?}",
                before["communicationTimeConfig"]
            );

            let (status, body) = delete_window(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteCampaignCommunicationTime expected 200, got {status} {body}"
            );

            // The config the store should now hold: the created one minus the target
            // channel's window, with everything else in place.
            let mut expected = created.clone();
            expected
                .as_object_mut()
                .expect("a communication time config is an object")
                .remove(member_of(target));

            let after = describe(&router, region, &id, &case).await;
            assert_eq!(
                after["communicationTimeConfig"], expected,
                "case {case:?}: expected {expected}, got {:?}",
                after["communicationTimeConfig"]
            );
            assert!(
                after["communicationTimeConfig"]
                    .get(member_of(target))
                    .is_none(),
                "case {case:?}: expected no {} window, got {:?}",
                member_of(target),
                after["communicationTimeConfig"]
            );

            // Idempotent: the same deletion again is a 200 and the same config.
            let (status, body) = delete_window(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated deletion expected 200, got {status} {body}"
            );
            let repeated = describe(&router, region, &id, &case).await;
            assert_eq!(
                repeated["communicationTimeConfig"], expected,
                "case {case:?}: expected the repeat to leave {expected}, got {:?}",
                repeated["communicationTimeConfig"]
            );

            // The call changes the communication time config and NOTHING else about
            // the campaign.
            let before_rest = campaign_without(&before, "communicationTimeConfig");
            let after_rest = campaign_without(&repeated, "communicationTimeConfig");
            assert_eq!(
                before_rest, after_rest,
                "case {case:?}: expected every other member untouched, got {after_rest} \
                 (was {before_rest})"
            );
        }
    }
}

/// A campaign created with no communication time config at all holds no window for
/// any channel, so every deletion is a 200 that stores nothing — the config stays
/// absent rather than being minted by the delete.
#[tokio::test]
async fn deleting_from_a_campaign_with_no_config_stores_nothing() {
    let region = "eu-central-1";
    let router = router();
    let instance = "communication-time-absent-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "no-config", None).await;

    for target in COMMUNICATION_TIME_CONFIG_TYPE_VALUES {
        let case = format!("no config at all, delete {target}");
        let (status, body) = delete_window(&router, region, &id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert!(
            campaign.get("communicationTimeConfig").is_none(),
            "case {case:?}: expected no communicationTimeConfig, got {:?}",
            campaign.get("communicationTimeConfig")
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and removes nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_loses_no_config() {
    let region = "eu-west-1";
    let router = router();
    let instance = "communication-time-404-instance";
    onboard(&router, region, instance).await;

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let kept = time_config(COMMUNICATION_TIME_CONFIG_TYPE_VALUES, "Europe/Dublin");
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

    // (case, id) — ids spanning `CampaignId` (len 1..256, pattern ^[-:/a-zA-Z0-9]+$).
    let cases: Vec<(String, String)> = vec![
        ("never created".to_string(), "missing".to_string()),
        ("single character".to_string(), "0".to_string()),
        (
            // `/` is left out: a `@httpLabel` is one path segment, so an id
            // carrying one does not address this operation's URI at all.
            "every character class the pattern admits".to_string(),
            "A-b:C1".to_string(),
        ),
        ("the 256-character maximum".to_string(), "c".repeat(256)),
        ("created and then deleted".to_string(), deleted.clone()),
    ];

    for (case, id) in cases {
        let (status, body) = delete_window(&router, region, &id, "TELEPHONY").await;
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

        // A refused call removes nothing: the campaign that does exist keeps every
        // window it was created with.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], kept,
            "case {case:?}: expected the control campaign's config untouched, got {:?}",
            campaign["communicationTimeConfig"]
        );
    }
}

/// A `config` outside the model's `CommunicationTimeConfigType` catalog names no
/// channel, so it is refused as a bad request and removes nothing.
///
/// The rows differ along what the rule generalises over: a value from a NEIGHBOURING
/// catalog, a catalog value in the wrong case, and an empty value. A request carrying
/// no `config` at all is left out: the generated routing table resolves this route by
/// its required query parameter, so such a request names no operation and never
/// reaches this handler.
#[tokio::test]
async fn a_config_outside_the_catalog_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "communication-time-validation-instance";
    onboard(&router, region, instance).await;

    let kept = time_config(COMMUNICATION_TIME_CONFIG_TYPE_VALUES, "UTC");
    let id = create_campaign(&router, region, instance, "validated", Some(&kept)).await;

    // (case, the query string the request carries)
    let cases: Vec<(&str, &str)> = vec![
        // `VOICE` is a plausible channel name that this catalog does not hold.
        ("a value the catalog does not hold", "?config=VOICE"),
        // The catalog is upper case, and an enum value is matched exactly.
        ("a catalog value in the wrong case", "?config=email"),
        ("an empty value", "?config="),
    ];

    for (case, query) in cases {
        let (status, response) = call(
            &router,
            region,
            "DELETE",
            &format!("/v2/campaigns/{id}/communication-time{query}"),
            &Value::Null,
        )
        .await;
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
            campaign["communicationTimeConfig"], kept,
            "case {case:?}: expected the refusal to leave the config alone, got {:?}",
            campaign["communicationTimeConfig"]
        );
    }
}

/// Which lifecycle states take the deletion: every non-terminal one, through the
/// shared predicate the configuration operations refuse on.
///
/// The table spans the whole `CampaignState` catalog against that predicate, and then
/// drives the deletion over HTTP in each state the campaign lifecycle can actually
/// reach — one channel per state, which is where the answer is observable.
#[tokio::test]
async fn every_non_terminal_state_takes_the_deletion() {
    // (state, takes a configuration change)
    let catalog: &[(CampaignState, bool)] = &[
        (CampaignState::Initialized, true),
        (CampaignState::Running, true),
        (CampaignState::Paused, true),
        (CampaignState::Stopped, true),
        (CampaignState::Completed, false),
        (CampaignState::Failed, false),
    ];
    for (state, expected) in catalog {
        let actual = state.accepts_configuration_change();
        assert_eq!(
            actual,
            *expected,
            "case {:?}: expected accepts_configuration_change {expected}, got {actual}",
            state.as_wire()
        );
    }

    let region = "ap-south-1";
    let router = router();
    let instance = "communication-time-state-instance";
    onboard(&router, region, instance).await;
    let created = time_config(COMMUNICATION_TIME_CONFIG_TYPE_VALUES, "Asia/Kolkata");
    let id = create_campaign(&router, region, instance, "stateful", Some(&created)).await;

    // Each lifecycle action the emulator's state machine offers, the state it leaves
    // the campaign in, and the channel deleted there.
    let walk: &[(&str, CampaignState, &str)] = &[
        ("", CampaignState::Initialized, "EMAIL"),
        ("start", CampaignState::Running, "SMS"),
        ("pause", CampaignState::Paused, "TELEPHONY"),
        ("stop", CampaignState::Stopped, "WHATSAPP"),
    ];
    let mut expected = created.clone();
    for (action, state, target) in walk {
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

        let (status, body) = delete_window(&router, region, &id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deletion to be accepted, got {status} {body}"
        );
        expected
            .as_object_mut()
            .expect("a communication time config is an object")
            .remove(member_of(target));
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], expected,
            "case {case:?}: expected {expected}, got {:?}",
            campaign["communicationTimeConfig"]
        );
    }

    // Every channel's window is gone and the required time zone config is still
    // there: the deletions removed windows, never the config itself.
    assert_eq!(
        expected,
        json!({"localTimeZoneConfig": {"defaultTimeZone": "Asia/Kolkata"}}),
        "expected the time zone config to survive every deletion, got {expected}"
    );
}
