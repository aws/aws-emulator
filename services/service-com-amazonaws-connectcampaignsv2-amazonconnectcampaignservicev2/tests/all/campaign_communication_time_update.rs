// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignCommunicationTime`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call sets the campaign `{id}` names to the `communicationTimeConfig` the
//!   request carries and REPLACES whatever it held — the config is one modelled
//!   structure, so a channel the new config leaves out is a channel the campaign no
//!   longer opens, and its `localTimeZoneConfig` is the new one — which is what every
//!   later read (`DescribeCampaign`) projects, while every other member of the
//!   campaign stays as it was;
//! * the operation is idempotent (`@idempotent` in the model, "This API is
//!   idempotent" in its documentation): the same config sent twice leaves the
//!   campaign exactly as the first call left it;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and stores
//!   nothing;
//! * a `dailyHours` key outside the model's `DayOfWeek` catalog is HTTP 400
//!   `ValidationException` and stores nothing, on the update and on `CreateCampaign`
//!   alike;
//! * a campaign in a non-terminal lifecycle state takes the update.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    COMMUNICATION_TIME_CONFIG_TYPE_VALUES, DAY_OF_WEEK_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, CAMPAIGN_NOT_FOUND_MESSAGE, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_without, error_code, router, Harness};

// The fixtures below are this suite's own, as every suite in this crate keeps its
// own (`campaign_communication_limits`, `campaign_flow_association`,
// `campaign_lifecycle`, …): `tests/all/common.rs` is the shared HTTP harness, and a
// suite's fixtures state the shape ITS cases vary — here the forms a
// `CommunicationTimeConfig` takes across an update, which is what this suite is
// about and not what the deletion suite's fixtures span.

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
        let mut ranges = vec![json!({"startTime": "T08:30", "endTime": "T11:45"})];
        if index % 2 == 0 {
            ranges.push(json!({"startTime": "T14:15", "endTime": "T18:00"}));
        }
        daily_hours.insert((*day).to_string(), Value::Array(ranges));
    }
    let mut time_window = json!({"openHours": {"dailyHours": daily_hours}});
    if shape % 3 == 1 {
        time_window["restrictedPeriods"] = json!({
            "restrictedPeriodList": [
                {"name": "holiday", "startDate": "2026-12-24", "endDate": "2026-12-26"},
                {"startDate": "2027-01-01", "endDate": "2027-01-01"}
            ]
        });
    }
    time_window
}

/// A `localTimeZoneConfig` — the one `@required` member of
/// `CommunicationTimeConfig` — whose form varies with `shape`.
///
/// Source: the `LocalTimeZoneConfig` shape (`defaultTimeZone`, a `TimeZone`;
/// `localTimeZoneDetection`, a list of `LocalTimeZoneDetectionType`
/// — `ZIP_CODE` | `AREA_CODE`; `localTimeZoneDetectionScope`, `PRIMARY_ONLY` |
/// `ALL_AVAILABLE`).
fn local_time_zone_config(shape: usize, time_zone: &str) -> Value {
    match shape % 3 {
        0 => json!({"defaultTimeZone": time_zone}),
        1 => json!({
            "defaultTimeZone": time_zone,
            "localTimeZoneDetection": ["ZIP_CODE", "AREA_CODE"],
        }),
        _ => json!({
            "defaultTimeZone": time_zone,
            "localTimeZoneDetection": ["AREA_CODE"],
            "localTimeZoneDetectionScope": "ALL_AVAILABLE",
        }),
    }
}

/// A `communicationTimeConfig` opening `channels` (each a
/// `CommunicationTimeConfigType` value), rooted in the `shape`-th form of
/// `localTimeZoneConfig` for `time_zone`.
fn time_config(channels: &[&str], time_zone: &str, shape: usize) -> Value {
    let mut config = json!({"localTimeZoneConfig": local_time_zone_config(shape, time_zone)});
    for (offset, channel) in channels.iter().enumerate() {
        config[member_of(channel)] = window(shape + offset);
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

/// `CreateCampaign` on an onboarded `instance`, optionally carrying
/// `communication_time_config`, returning the status and the response body.
async fn create(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    communication_time_config: Option<&Value>,
) -> (u16, Value) {
    let mut body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_subtype_config(),
    });
    if let Some(config) = communication_time_config {
        body["communicationTimeConfig"] = config.clone();
    }
    call(router, region, "PUT", "/v2/campaigns", &body).await
}

/// [`create`], asserting the create succeeded, and handing back the campaign id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    communication_time_config: Option<&Value>,
) -> String {
    let (status, response) = create(router, region, instance, name, communication_time_config).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCampaign expected 200, got {status} {response}"
    );
    response["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCampaign returned no id: {response}"))
        .to_string()
}

/// `UpdateCampaignCommunicationTime` for `id`, carrying `config`.
async fn update_time(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/communication-time"),
        &json!({"communicationTimeConfig": config}),
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

/// One row of the replacement table in
/// [`an_update_replaces_the_whole_communication_time_config`].
struct Row<'a> {
    /// What an assertion calls this row.
    name: &'a str,
    /// The channels the campaign is created with, or `None` for a campaign created
    /// with no communication time config at all.
    created: Option<Vec<&'a str>>,
    /// The channels the update's config opens.
    updated: Vec<&'a str>,
    /// The time zone the update's config is rooted in.
    time_zone: &'a str,
    /// Which form the update's windows and `localTimeZoneConfig` take.
    shape: usize,
}

/// The update stores the config it was given, whole: every later read projects
/// exactly that config, a channel the new config leaves out is gone, the time zone
/// config is the new one, and every other member of the campaign is untouched.
///
/// The table's rows differ along what the rule generalises over: which channels the
/// stored config opens, which the new one opens (more, fewer, the same, none at all),
/// the form of each window (days opened, ranges per day, restricted periods) and the
/// form of `localTimeZoneConfig` — including a campaign that started with no
/// communication time config at all.
#[tokio::test]
async fn an_update_replaces_the_whole_communication_time_config() {
    let region = "us-west-2";
    let router = router();
    let instance = "communication-time-update-instance";
    onboard(&router, region, instance).await;

    let every_channel = COMMUNICATION_TIME_CONFIG_TYPE_VALUES.to_vec();

    let cases: Vec<Row> = vec![
        Row {
            name: "no config at all, then every channel",
            created: None,
            updated: every_channel.clone(),
            time_zone: "America/New_York",
            shape: 0,
        },
        Row {
            name: "every channel, then one channel",
            created: Some(every_channel.clone()),
            updated: vec!["SMS"],
            time_zone: "Europe/Dublin",
            shape: 1,
        },
        Row {
            name: "one channel, then every channel",
            created: Some(vec!["TELEPHONY"]),
            updated: every_channel.clone(),
            time_zone: "Asia/Kolkata",
            shape: 2,
        },
        Row {
            name: "two channels, then two other channels",
            created: Some(vec!["EMAIL", "SMS"]),
            updated: vec!["TELEPHONY", "WHATSAPP"],
            time_zone: "Australia/Sydney",
            shape: 3,
        },
        Row {
            name: "every channel, then the time zone only",
            created: Some(every_channel.clone()),
            updated: Vec::new(),
            time_zone: "UTC",
            shape: 4,
        },
        Row {
            name: "the time zone only, then one channel",
            created: Some(Vec::new()),
            updated: vec!["WHATSAPP"],
            time_zone: "America/Los_Angeles",
            shape: 5,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let Row {
            name: case,
            created: created_channels,
            updated: updated_channels,
            time_zone,
            shape,
        } = row;
        let created = created_channels
            .as_ref()
            .map(|channels| time_config(channels, "Africa/Cairo", index));
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("campaign-{index}"),
            created.as_ref(),
        )
        .await;
        let before = describe(&router, region, &id, case).await;
        assert_eq!(
            before.get("communicationTimeConfig"),
            created.as_ref(),
            "case {case:?}: expected the created config stored, got {:?}",
            before.get("communicationTimeConfig")
        );

        let updated = time_config(updated_channels, time_zone, *shape);
        let (status, body) = update_time(&router, region, &id, &updated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignCommunicationTime expected 200, got {status} {body}"
        );

        // The stored config is the one the request carried, and nothing of the old
        // one survives: a channel the update left out holds no window.
        let after = describe(&router, region, &id, case).await;
        assert_eq!(
            after["communicationTimeConfig"], updated,
            "case {case:?}: expected {updated}, got {:?}",
            after["communicationTimeConfig"]
        );
        for channel in COMMUNICATION_TIME_CONFIG_TYPE_VALUES {
            let expected_window = updated.get(member_of(channel));
            assert_eq!(
                after["communicationTimeConfig"].get(member_of(channel)),
                expected_window,
                "case {case:?}: expected the {} window to be {expected_window:?}, got {:?}",
                member_of(channel),
                after["communicationTimeConfig"].get(member_of(channel))
            );
        }

        // Idempotent: the same config sent again is a 200 and the same stored config.
        let (status, body) = update_time(&router, region, &id, &updated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated update expected 200, got {status} {body}"
        );
        let repeated = describe(&router, region, &id, case).await;
        assert_eq!(
            repeated["communicationTimeConfig"], updated,
            "case {case:?}: expected the repeat to leave {updated}, got {:?}",
            repeated["communicationTimeConfig"]
        );

        // The call changes the communication time config and NOTHING else about the
        // campaign.
        let before_rest = campaign_without(&before, "communicationTimeConfig");
        let after_rest = campaign_without(&repeated, "communicationTimeConfig");
        assert_eq!(
            before_rest, after_rest,
            "case {case:?}: expected every other member untouched, got {after_rest} \
             (was {before_rest})"
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and stores nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_takes_no_config() {
    let region = "eu-west-1";
    let router = router();
    let instance = "communication-time-update-404-instance";
    onboard(&router, region, instance).await;

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let kept = time_config(COMMUNICATION_TIME_CONFIG_TYPE_VALUES, "Europe/Dublin", 1);
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

    let sent = time_config(&["TELEPHONY"], "America/Chicago", 0);
    for (case, id) in cases {
        let (status, body) = update_time(&router, region, &id, &sent).await;
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

        // A refused call stores nothing: the campaign that does exist keeps the
        // config it was created with.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], kept,
            "case {case:?}: expected the control campaign's config untouched, got {:?}",
            campaign["communicationTimeConfig"]
        );
    }
}

/// A `dailyHours` key names a day of the week, so a key outside the model's
/// `DayOfWeek` catalog (`DAY_OF_WEEK_VALUES`) is refused as a bad request and stores
/// nothing — on the update, and on `CreateCampaign`, which takes the same config
/// shape.
///
/// The rows differ along what the rule generalises over: a day name the catalog does
/// not hold, a catalog value in the wrong case, an abbreviation of one, and an empty
/// key. A catalog value the cases above do not exercise (`SUNDAY`) is asserted to be
/// accepted, so the check refuses only what the catalog leaves out.
#[tokio::test]
async fn a_day_outside_the_modelled_catalog_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "communication-time-update-validation-instance";
    onboard(&router, region, instance).await;

    let kept = time_config(COMMUNICATION_TIME_CONFIG_TYPE_VALUES, "UTC", 2);
    let id = create_campaign(&router, region, instance, "validated", Some(&kept)).await;

    // (case, the `dailyHours` key the config carries)
    let cases: Vec<(&str, &str)> = vec![
        ("a day name the catalog does not hold", "FUNDAY"),
        ("a catalog value in the wrong case", "monday"),
        ("an abbreviation of a catalog value", "TUE"),
        ("an empty key", ""),
    ];

    for (case, day) in cases {
        let config = json!({
            "localTimeZoneConfig": {"defaultTimeZone": "UTC"},
            "telephony": {
                "openHours": {
                    "dailyHours": {day: [{"startTime": "T09:00", "endTime": "T17:00"}]}
                }
            }
        });

        let (status, response) = update_time(&router, region, &id, &config).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from the update, got {status} {response}"
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

        // The same config is refused where a campaign is created with it, so the
        // store never holds a day the model does not model.
        let (status, response) = create(&router, region, instance, case, Some(&config)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from CreateCampaign, got {status} {response}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException from CreateCampaign, got {}",
            error_code(&response)
        );
    }

    // A catalog value none of the rows above used is accepted, on both paths.
    let accepted = json!({
        "localTimeZoneConfig": {"defaultTimeZone": "UTC"},
        "telephony": {
            "openHours": {
                "dailyHours": {"SUNDAY": [{"startTime": "T09:00", "endTime": "T17:00"}]}
            }
        }
    });
    let (status, response) = update_time(&router, region, &id, &accepted).await;
    assert_eq!(
        status, 200,
        "case \"a catalog value\": expected HTTP 200, got {status} {response}"
    );
    let campaign = describe(&router, region, &id, "a catalog value").await;
    assert_eq!(
        campaign["communicationTimeConfig"], accepted,
        "case \"a catalog value\": expected {accepted}, got {:?}",
        campaign["communicationTimeConfig"]
    );
    let created =
        create_campaign(&router, region, instance, "sunday", Some(&accepted)).await;
    let campaign = describe(&router, region, &created, "a catalog value on create").await;
    assert_eq!(
        campaign["communicationTimeConfig"], accepted,
        "case \"a catalog value on create\": expected {accepted}, got {:?}",
        campaign["communicationTimeConfig"]
    );
}

/// Which lifecycle states take the update: every non-terminal one, through the shared
/// predicate the configuration operations refuse on.
///
/// The table spans the whole `CampaignState` catalog against that predicate, and then
/// drives the update over HTTP in each state the campaign lifecycle can actually
/// reach — a different config in each state, which is where the answer is observable.
#[tokio::test]
async fn every_non_terminal_state_takes_the_update() {
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
    let instance = "communication-time-update-state-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    // Each lifecycle action the emulator's state machine offers, the state it leaves
    // the campaign in, and the channel the config written there opens.
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

        let config = time_config(&[channel], "Asia/Kolkata", shape);
        let (status, body) = update_time(&router, region, &id, &config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], config,
            "case {case:?}: expected {config}, got {:?}",
            campaign["communicationTimeConfig"]
        );
    }
}
