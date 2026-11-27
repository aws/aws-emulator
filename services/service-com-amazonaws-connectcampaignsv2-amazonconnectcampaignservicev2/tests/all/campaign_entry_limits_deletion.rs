// SPDX-License-Identifier: CC0-1.0

//! `DeleteCampaignEntryLimits`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call removes the `entryLimitsConfig` the campaign named by `{id}` holds, and
//!   nothing else: every other member of the campaign stays as it was, which is what
//!   every later read (`DescribeCampaign`, `ListCampaigns`) projects, and a campaign
//!   holding no config carries the member absent rather than empty;
//! * the operation is idempotent (`@idempotent` in the model, "This API is idempotent"
//!   in its documentation): a campaign carrying no entry limits config is a 200 that
//!   removes nothing;
//! * the deletion and `UpdateCampaignEntryLimits` round-trip: a config can be stored,
//!   removed and stored again;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and removes
//!   nothing;
//! * a campaign whose state is terminal takes no configuration change, which is this
//!   operation's `InvalidCampaignStateException` — judged on the state every reader
//!   reports, so a campaign whose run window has closed is refused too.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    campaign_id_not_found_message, CampaignState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, campaign_without, create_campaign, error_code, onboard_instance, router,
};

/// One `EntryLimitsConfig`: both its members are `@required`.
fn entry_limits(max_entry_count: i64, min_entry_interval: &str) -> Value {
    json!({
        "maxEntryCount": max_entry_count,
        "minEntryInterval": min_entry_interval,
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

/// `UpdateCampaignEntryLimits` for `id`, asserting it was accepted.
async fn store_entry_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &Value,
    case: &str,
) {
    let (status, body) = call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/entry-limits"),
        &json!({"entryLimitsConfig": config}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateCampaignEntryLimits expected 200, got {status} {body}"
    );
}

/// `DeleteCampaignEntryLimits` for `id`.
async fn delete_entry_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{id}/entry-limits"),
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

/// The deletion leaves the campaign with no entry limits config, whatever it held, and
/// changes nothing else about it — over a TABLE of configs the campaign is carrying
/// when the deletion arrives.
///
/// The rows differ along what the rule generalises over: whether a config was stored at
/// all, how many successive updates preceded the deletion, the published minimum of
/// `maxEntryCount`'s `@range` (0, which the model documents as unlimited entries) and
/// values above it, and the forms `minEntryInterval`'s ISO-8601 duration pattern admits
/// (source: `EntryLimitsConfig$minEntryInterval`'s `@pattern`, days and/or a time part
/// of hours, minutes and fractional seconds).
#[tokio::test]
async fn deleting_the_entry_limits_leaves_every_other_part_of_the_campaign() {
    let region = "us-west-2";
    let router = router();
    let instance = "entry-limits-deletion-instance";

    // (case, the configs stored in order before the deletion)
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("no entry limits config at all", vec![]),
        ("one config stored", vec![entry_limits(3, "PT1H")]),
        (
            "maxEntryCount 0, the published minimum of its @range (unlimited entries)",
            vec![entry_limits(0, "PT15M")],
        ),
        (
            "successive updates, then the deletion",
            vec![
                entry_limits(1, "PT1M"),
                entry_limits(2, "PT2H"),
                entry_limits(100, "P7D"),
            ],
        ),
        (
            "a duration carrying days and a time part together",
            vec![entry_limits(5, "P1DT2H3M4S")],
        ),
        (
            "a duration with fractional seconds",
            vec![entry_limits(7, "PT0.500S")],
        ),
        (
            "the bare duration designator the pattern admits",
            vec![entry_limits(1, "P")],
        ),
        ("a large maxEntryCount", vec![entry_limits(1_000_000, "P30D")]),
    ];

    for (index, (case, sequence)) in cases.iter().enumerate() {
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("entry-limits-deletion-{index}"),
            None,
        )
        .await;
        // The campaign as it is with no entry limits config: what the deletion has to
        // leave behind, whatever it held in between.
        let bare = describe(&router, region, &id, case).await;
        assert_eq!(
            bare["entryLimitsConfig"],
            Value::Null,
            "case {case:?}: expected a fresh campaign to carry no config, got {}",
            bare["entryLimitsConfig"]
        );

        for config in sequence {
            store_entry_limits(&router, region, &id, config, case).await;
        }

        let (status, body) = delete_entry_limits(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCampaignEntryLimits expected 200, got {status} {body}"
        );

        let after = describe(&router, region, &id, case).await;
        assert!(
            after.get("entryLimitsConfig").is_none(),
            "case {case:?}: expected no entryLimitsConfig member, got {:?}",
            after.get("entryLimitsConfig")
        );
        let listed = listed_entry_limits(&router, region, &id, case).await;
        assert_eq!(
            listed,
            Value::Null,
            "case {case:?}: expected ListCampaigns to project no config, got {listed}"
        );

        // Idempotent: the same deletion again is a 200 and the same campaign.
        let (status, body) = delete_entry_limits(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated deletion expected 200, got {status} {body}"
        );
        let repeated = describe(&router, region, &id, case).await;
        assert!(
            repeated.get("entryLimitsConfig").is_none(),
            "case {case:?}: expected the repeat to leave no config, got {:?}",
            repeated.get("entryLimitsConfig")
        );

        // The call removes the entry limits and NOTHING else about the campaign.
        let bare_rest = campaign_without(&bare, "entryLimitsConfig");
        let repeated_rest = campaign_without(&repeated, "entryLimitsConfig");
        assert_eq!(
            bare_rest, repeated_rest,
            "case {case:?}: expected every other member untouched, got {repeated_rest} \
             (was {bare_rest})"
        );
    }
}

/// The deletion and `UpdateCampaignEntryLimits` round-trip against one campaign: what
/// the update stores is what the delete removes, and the campaign takes a new config
/// afterwards — the delete removes the config rather than blocking the member.
#[tokio::test]
async fn an_entry_limits_config_can_be_set_deleted_and_set_again() {
    let region = "eu-central-1";
    let router = router();
    let instance = "entry-limits-roundtrip-instance";
    let id = create_campaign(&router, region, instance, "roundtrip", None).await;

    // (case, the config to store before the deletion)
    let rounds: Vec<(&str, Value)> = vec![
        ("the first round", entry_limits(2, "PT4M")),
        ("a day-long interval", entry_limits(11, "P1D")),
        ("maxEntryCount at its @range minimum", entry_limits(0, "PT1H")),
        (
            "the longest interval the @length max of 50 admits",
            // 50 characters: `P`, 48 digits and `D`, all within the pattern's
            // day-component syntax (source: `EntryLimitsConfig$minEntryInterval`).
            entry_limits(4, &format!("P{}D", "1".repeat(48))),
        ),
    ];

    for (case, config) in rounds {
        store_entry_limits(&router, region, &id, &config, case).await;
        let stored = describe(&router, region, &id, case).await;
        assert_eq!(
            stored["entryLimitsConfig"], config,
            "case {case:?}: expected the update to store {config}, got {}",
            stored["entryLimitsConfig"]
        );

        let (status, body) = delete_entry_limits(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deletion expected 200, got {status} {body}"
        );
        let after = describe(&router, region, &id, case).await;
        assert!(
            after.get("entryLimitsConfig").is_none(),
            "case {case:?}: expected no config after the deletion, got {:?}",
            after.get("entryLimitsConfig")
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and removes nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_loses_no_entry_limits() {
    let region = "eu-west-1";
    let router = router();
    let instance = "entry-limits-deletion-404-instance";

    let kept = entry_limits(8, "PT9M");

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let control = create_campaign(&router, region, instance, "control", None).await;
    store_entry_limits(&router, region, &control, &kept, "control campaign").await;

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
        let (status, body) = delete_entry_limits(&router, region, &id).await;
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
            "case {case:?}: expected this operation's not-found text naming the id, got {}",
            body["message"]
        );

        // A refused call removes nothing: the campaign that does exist keeps the
        // config it was given.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["entryLimitsConfig"], kept,
            "case {case:?}: expected the control campaign's config untouched, got {}",
            campaign["entryLimitsConfig"]
        );
    }
}

/// Which lifecycle states take the deletion: every non-terminal one, and neither
/// terminal one.
///
/// The table spans the whole `CampaignState` catalog (source: the model's
/// `CampaignState` enum) against the shared predicate the configuration operations
/// refuse through, and then drives the deletion over HTTP in each state the campaign
/// lifecycle can actually reach, which is where the answer is observable.
#[tokio::test]
async fn every_non_terminal_state_takes_the_entry_limits_deletion() {
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
    let instance = "entry-limits-deletion-state-instance";
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    // Each lifecycle action the emulator's state machine offers, with the state it
    // leaves the campaign in: the deletion is accepted in every one of them. The count
    // differs per step so each assertion observes that step's own write being removed.
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
            "case {case:?}: expected state {:?}, got {}",
            state.as_wire(),
            read["state"]
        );

        let config = entry_limits(*max_entry_count, "PT10M");
        store_entry_limits(&router, region, &id, &config, &case).await;

        let (status, body) = delete_entry_limits(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deletion to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert!(
            campaign.get("entryLimitsConfig").is_none(),
            "case {case:?}: expected no entryLimitsConfig, got {:?}",
            campaign.get("entryLimitsConfig")
        );
    }
}

/// A campaign whose run window has closed reports `Completed`, and the deletion is
/// refused from there with HTTP 409 naming that state, leaving the stored config in
/// place: the gate judges the state a reader reports, not the stored lifecycle state.
///
/// The rows are the two ways the same campaign answers: while its window is open the
/// deletion is taken, and once the window has closed the very same request is refused.
#[tokio::test]
async fn a_campaign_past_its_run_window_refuses_the_entry_limits_deletion() {
    let region = "us-east-2";
    let router = router();
    let instance = "entry-limits-deletion-window-instance";
    onboard_instance(&router, region, instance).await;

    // (case, run window as offsets in seconds from now, expected status)
    let cases: &[(&str, (f64, f64), u16)] = &[
        ("window still open", (-60.0, 3_600.0), 200),
        ("window already closed", (-7_200.0, -3_600.0), 409),
    ];

    for (case, (start, end), expected) in cases {
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("windowed-{case}"),
            Some(json!({"startTime": instant(*start), "endTime": instant(*end)})),
        )
        .await;

        let config = entry_limits(2, "PT20M");
        store_entry_limits(&router, region, &id, &config, case).await;

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

        let (status, body) = delete_entry_limits(&router, region, &id).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected} in state {reported:?}, got {status} {body}"
        );

        let campaign = describe(&router, region, &id, case).await;
        if *expected == 409 {
            assert!(
                error_code(&body).ends_with("InvalidCampaignStateException"),
                "case {case:?}: expected InvalidCampaignStateException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body["state"].as_str(),
                Some(reported.as_str()),
                "case {case:?}: expected the refusal to name state {reported:?}, got {}",
                body["state"]
            );
            assert_eq!(
                campaign["entryLimitsConfig"], config,
                "case {case:?}: expected the refusal to remove nothing, got {}",
                campaign["entryLimitsConfig"]
            );
        } else {
            assert!(
                campaign.get("entryLimitsConfig").is_none(),
                "case {case:?}: expected the config removed, got {:?}",
                campaign.get("entryLimitsConfig")
            );
        }
    }
}
