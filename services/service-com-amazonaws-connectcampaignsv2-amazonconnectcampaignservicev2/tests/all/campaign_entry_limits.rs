// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignEntryLimits`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call REPLACES the campaign's `entryLimitsConfig` with the config it was given,
//!   whatever that campaign held before, and the stored config is what every later read
//!   of the campaign (`DescribeCampaign`, `ListCampaigns`) projects — nothing else about
//!   the campaign changes;
//! * the operation is idempotent ("This API is idempotent", per the model's own
//!   documentation): sending the same config again leaves the same config stored;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and stores
//!   nothing;
//! * a config outside `EntryLimitsConfig`'s modelled constraints (`maxEntryCount`'s
//!   `@range` minimum of 0, `minEntryInterval`'s ISO-8601 duration pattern and its
//!   `@length` max of 50) is HTTP 400 `ValidationException` and leaves the stored config
//!   alone;
//! * a campaign whose state is terminal takes no configuration change, which is this
//!   operation's `InvalidCampaignStateException` — judged on the state every reader
//!   reports, so a campaign whose run window has closed is refused too.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    campaign_id_not_found_message, CampaignState, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_without, error_code, router, Harness};

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

/// `now + offset_secs` as the RFC 3339 instant a `Schedule` member carries on the wire:
/// `Schedule$startTime` / `$endTime` target `TimeStamp`, which the model marks
/// `@timestampFormat("date-time")`.
fn instant(offset_secs: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64();
    aws_smithy_types::DateTime::from_secs_f64(now + offset_secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable instant")
}

/// One `EntryLimitsConfig`: both its members are `@required`.
fn entry_limits(max_entry_count: i64, min_entry_interval: &str) -> Value {
    json!({
        "maxEntryCount": max_entry_count,
        "minEntryInterval": min_entry_interval,
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

/// Create a campaign on an onboarded `instance`, optionally already carrying
/// `entryLimitsConfig`, and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    limits: Option<&Value>,
) -> String {
    let mut body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_config(),
    });
    if let Some(limits) = limits {
        body["entryLimitsConfig"] = limits.clone();
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

/// `UpdateCampaignEntryLimits` for `id`.
async fn update_entry_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/entry-limits"),
        body,
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

/// The `entryLimitsConfig` the campaign `id` carries in a `ListCampaigns` page.
async fn listed_entry_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> Value {
    let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {body}"
    );
    let summary = body["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: campaign {id} absent from the page: {body}"))
        .clone();
    summary["entryLimitsConfig"].clone()
}

/// The stored entry limits are whatever the last accepted call carried, over a TABLE of
/// configs, and every later read projects them verbatim.
///
/// The rows differ along what the rule generalises over: whether the campaign was
/// created with a config already, how many successive updates it takes, the published
/// minimum of `maxEntryCount`'s `@range` (0, which the model documents as unlimited
/// entries) and values above it, and the forms `minEntryInterval`'s ISO-8601 duration
/// pattern admits (source: `EntryLimitsConfig$minEntryInterval`'s `@pattern`, days
/// and/or a time part of hours, minutes and fractional seconds).
#[tokio::test]
async fn an_entry_limits_config_is_stored_and_every_read_projects_it() {
    let region = "us-west-2";
    let router = router();
    let instance = "entry-limits-instance";
    onboard(&router, region, instance).await;

    // (case, the config the create carried, the configs to send in order)
    let cases: Vec<(&str, Option<Value>, Vec<Value>)> = vec![
        (
            "a campaign created with no entry limits takes one",
            None,
            vec![entry_limits(3, "PT1H")],
        ),
        (
            "replaces the config the create carried",
            Some(entry_limits(1, "P1D")),
            vec![entry_limits(9, "PT30M")],
        ),
        (
            "maxEntryCount 0, the published minimum of its @range (unlimited entries)",
            None,
            vec![entry_limits(0, "PT15M")],
        ),
        (
            "successive updates: the last one is what is stored",
            None,
            vec![
                entry_limits(1, "PT1M"),
                entry_limits(2, "PT2H"),
                entry_limits(100, "P7D"),
            ],
        ),
        (
            "a duration carrying days and a time part together",
            None,
            vec![entry_limits(5, "P1DT2H3M4S")],
        ),
        (
            "a duration with fractional seconds",
            None,
            vec![entry_limits(7, "PT0.500S")],
        ),
        (
            "the bare duration designator the pattern admits",
            None,
            vec![entry_limits(1, "P")],
        ),
        (
            "a large maxEntryCount",
            None,
            vec![entry_limits(1_000_000, "P30D")],
        ),
    ];

    for (index, (case, created_with, sequence)) in cases.iter().enumerate() {
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("campaign-{index}"),
            created_with.as_ref(),
        )
        .await;
        let before = describe(&router, region, &id, case).await;

        for config in sequence {
            let (status, body) =
                update_entry_limits(&router, region, &id, &json!({"entryLimitsConfig": config}))
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateCampaignEntryLimits expected 200, got {status} {body}"
            );

            let campaign = describe(&router, region, &id, case).await;
            assert_eq!(
                &campaign["entryLimitsConfig"], config,
                "case {case:?}: expected the stored config {config}, got {}",
                campaign["entryLimitsConfig"]
            );
            let listed = listed_entry_limits(&router, region, &id, case).await;
            assert_eq!(
                &listed, config,
                "case {case:?}: expected ListCampaigns to project {config}, got {listed}"
            );
        }

        let last = sequence.last().expect("every case sends at least one config");

        // Idempotent: the same config again is the same write, and still a 200.
        let (status, body) =
            update_entry_limits(&router, region, &id, &json!({"entryLimitsConfig": last})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated update expected 200, got {status} {body}"
        );

        let after = describe(&router, region, &id, case).await;
        assert_eq!(
            &after["entryLimitsConfig"], last,
            "case {case:?}: expected the repeat to leave {last} stored, got {}",
            after["entryLimitsConfig"]
        );

        // The call changes the entry limits and NOTHING else about the campaign.
        let before_rest = campaign_without(&before, "entryLimitsConfig");
        let after_rest = campaign_without(&after, "entryLimitsConfig");
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
async fn an_unknown_campaign_is_not_found_and_takes_no_entry_limits() {
    let region = "eu-west-1";
    let router = router();
    let instance = "entry-limits-404-instance";
    onboard(&router, region, instance).await;

    let config = entry_limits(4, "PT45M");

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let control = create_campaign(&router, region, instance, "control", None).await;
    let (status, body) = update_entry_limits(
        &router,
        region,
        &control,
        &json!({"entryLimitsConfig": config}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"control campaign\": expected 200, got {status} {body}"
    );

    // One campaign created and then deleted: its id was valid a moment ago.
    let deleted = create_campaign(&router, region, instance, "doomed", None).await;
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
        let (status, body) =
            update_entry_limits(&router, region, &id, &json!({"entryLimitsConfig": config})).await;
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
            Some(campaign_id_not_found_message(&id).as_str()),
            "case {case:?}: expected this operation's not-found text naming the id, got {:?}",
            body["message"]
        );

        // A refused call stores nothing: the id is still absent...
        let (status, read) = call(
            &router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the campaign to still be absent, got {status} {read}"
        );
        // ...and the campaign that does exist is untouched.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["entryLimitsConfig"], config,
            "case {case:?}: expected the control campaign's config untouched, got {}",
            campaign["entryLimitsConfig"]
        );
    }
}

/// A config outside the modelled constraints is refused as a bad request, and the
/// stored config is left alone.
///
/// The rows differ along what the rule generalises over: one past `maxEntryCount`'s
/// published `@range` minimum of 0, one past `minEntryInterval`'s `@length` max of 50,
/// durations outside its `@pattern`, each required member omitted, and the operation's
/// own required member omitted.
#[tokio::test]
async fn a_config_outside_the_modelled_constraints_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "entry-limits-validation-instance";
    onboard(&router, region, instance).await;

    let stored = entry_limits(6, "PT6H");
    let id = create_campaign(&router, region, instance, "validated", Some(&stored)).await;

    // (case, request body)
    let cases: Vec<(String, Value)> = vec![
        (
            "maxEntryCount -1, one below its @range minimum of 0".to_string(),
            json!({"entryLimitsConfig": entry_limits(-1, "PT1H")}),
        ),
        (
            "a minEntryInterval one past its @length max of 50".to_string(),
            json!({"entryLimitsConfig": {
                "maxEntryCount": 1,
                // 51 characters, all within the pattern's day-component syntax.
                "minEntryInterval": format!("P{}D", "1".repeat(49)),
            }}),
        ),
        (
            "a duration with no leading P, outside the @pattern".to_string(),
            json!({"entryLimitsConfig": entry_limits(1, "1H")}),
        ),
        (
            "an ISO-8601 month component the @pattern does not admit".to_string(),
            json!({"entryLimitsConfig": entry_limits(1, "P1M")}),
        ),
        (
            "a time part with no T, outside the @pattern".to_string(),
            json!({"entryLimitsConfig": entry_limits(1, "P1H")}),
        ),
        (
            "free text, outside the @pattern".to_string(),
            json!({"entryLimitsConfig": entry_limits(1, "one hour")}),
        ),
        (
            "the required minEntryInterval omitted".to_string(),
            json!({"entryLimitsConfig": {"maxEntryCount": 1}}),
        ),
        (
            "the required maxEntryCount omitted".to_string(),
            json!({"entryLimitsConfig": {"minEntryInterval": "PT1H"}}),
        ),
        (
            "the operation's required entryLimitsConfig omitted".to_string(),
            json!({}),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = update_entry_limits(&router, region, &id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&response)
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["entryLimitsConfig"], stored,
            "case {case:?}: expected the refusal to leave {stored} stored, got {}",
            campaign["entryLimitsConfig"]
        );
    }
}

/// A campaign whose run window has closed reports `Completed`, and every
/// configuration change is refused from there: the gate judges the state a reader
/// reports, not the stored lifecycle state.
///
/// The rows are the two ways the same campaign answers: while its window is open the
/// update is taken, and once the window has closed the very same request is refused
/// with HTTP 409 naming the state it is in.
#[tokio::test]
async fn a_campaign_past_its_run_window_refuses_an_entry_limits_update() {
    let region = "us-east-2";
    let router = router();
    let instance = "entry-limits-window-instance";
    onboard(&router, region, instance).await;

    // (case, run window as offsets in seconds from now, expected status)
    let cases: &[(&str, (f64, f64), u16)] = &[
        ("window still open", (-60.0, 3_600.0), 200),
        ("window already closed", (-7_200.0, -3_600.0), 409),
    ];

    for (case, (start, end), expected) in cases {
        let (status, created) = call(
            &router,
            region,
            "PUT",
            "/v2/campaigns",
            &json!({
                "name": format!("windowed-{case}"),
                "connectInstanceId": instance,
                "channelSubtypeConfig": telephony_config(),
                "schedule": {"startTime": instant(*start), "endTime": instant(*end)},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateCampaign expected 200, got {status} {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateCampaign returned no id: {created}"))
            .to_string();

        // The run has to have been entered for the clock to complete it.
        let (status, started) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/start"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartCampaign expected 200, got {status} {started}"
        );

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
        let reported = read["state"].as_str().unwrap_or("<no state>").to_string();

        let config = entry_limits(2, "PT20M");
        let (status, body) =
            update_entry_limits(&router, region, &id, &json!({"entryLimitsConfig": config})).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected} in state {reported:?}, got {status} {body}"
        );

        if *expected == 409 {
            assert!(
                error_code(&body).ends_with("InvalidCampaignStateException"),
                "case {case:?}: expected InvalidCampaignStateException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body["state"].as_str(),
                Some(reported.as_str()),
                "case {case:?}: expected the refusal to name state {reported:?}, got {:?}",
                body["state"]
            );
            let campaign = describe(&router, region, &id, case).await;
            assert_eq!(
                campaign["entryLimitsConfig"],
                Value::Null,
                "case {case:?}: expected the refusal to store nothing, got {}",
                campaign["entryLimitsConfig"]
            );
        } else {
            let campaign = describe(&router, region, &id, case).await;
            assert_eq!(
                campaign["entryLimitsConfig"], config,
                "case {case:?}: expected the stored config {config}, got {}",
                campaign["entryLimitsConfig"]
            );
        }
    }
}

/// Which lifecycle states take a configuration change: every non-terminal one, and
/// neither terminal one.
///
/// The table spans the whole `CampaignState` catalog (source: the model's
/// `CampaignState` enum) against the shared predicate the configuration operations
/// refuse through, and then drives the update over HTTP in each state the campaign
/// lifecycle can actually reach, which is where the operation's answer is observable.
#[tokio::test]
async fn every_non_terminal_state_takes_an_entry_limits_update() {
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
    let instance = "entry-limits-state-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    // Each lifecycle action the emulator's state machine offers, with the state it
    // leaves the campaign in: the update is accepted in every one of them. The count
    // differs per step so each assertion observes that step's own write.
    let walk: &[(&str, CampaignState, i64)] = &[
        ("", CampaignState::Initialized, 1),
        ("start", CampaignState::Running, 2),
        ("pause", CampaignState::Paused, 3),
        ("stop", CampaignState::Stopped, 4),
    ];
    for (action, state, max_entry_count) in walk {
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

        let config = entry_limits(*max_entry_count, "PT10M");
        let (status, body) =
            update_entry_limits(&router, region, &id, &json!({"entryLimitsConfig": config})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["entryLimitsConfig"], config,
            "case {case:?}: expected the stored config {config}, got {}",
            campaign["entryLimitsConfig"]
        );
    }
}
