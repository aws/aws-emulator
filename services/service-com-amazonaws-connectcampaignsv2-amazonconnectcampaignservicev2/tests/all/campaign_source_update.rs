// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignSource`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call SETS the campaign `{id}` names to be sourced from the `source` the
//!   request carries, replacing whatever that campaign was sourced from, and the stored
//!   source is what every later read of the campaign (`DescribeCampaign`) projects —
//!   nothing else about the campaign changes;
//! * `Source` is a union, so exactly ONE member is in force: an update naming the
//!   other member leaves no trace of the one it replaced;
//! * the operation is idempotent (`@idempotent` in the model, "This API is
//!   idempotent" in its documentation): repeating it with the same source leaves the
//!   same source;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and stores
//!   nothing;
//! * the required member omitted is HTTP 400 `ValidationException` and leaves the
//!   stored source alone;
//! * a campaign in a terminal lifecycle state takes no configuration change, which is
//!   this operation's HTTP 409 `InvalidCampaignStateException`.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::CAMPAIGN_STATE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    campaign_id_not_found_message, CampaignState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, campaign_state, campaign_without, create_campaign_with, describe_campaign,
    drive_lifecycle, error_code, instant, padded_arn, router, Harness,
};

/// The `arn:<partition>:<service>:<region>:<account>:<resource-prefix>` head
/// [`padded_arn`] grows a Customer Profiles segment ARN from, for the published bounds
/// of `Arn`'s `@length` 20..500 — the shape both `Source$customerProfilesSegmentArn`
/// and `EventTrigger$customerProfilesDomainArn` target.
const SEGMENT_ARN_HEAD: &str = "arn:aws:profile:us-east-1:000000000000:segment/";

/// The shared campaign fixture ([`create_campaign_with`], which onboards `instance`
/// first), optionally already sourced from `source` and optionally carrying `window`
/// (offsets in seconds from now) as its schedule — the run window is how a case
/// reaches the state the CLOCK owns. Hands back the minted id.
async fn create_sourced_campaign(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
    source: Option<&Value>,
    window: Option<(f64, f64)>,
) -> String {
    let mut extra = json!({});
    if let Some(source) = source {
        extra["source"] = source.clone();
    }
    if let Some((start, end)) = window {
        extra["schedule"] = json!({"startTime": instant(start), "endTime": instant(end)});
    }
    create_campaign_with(router, region, instance, name, extra).await
}

/// `UpdateCampaignSource` for `id`.
async fn update_source(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/source"),
        body,
    )
    .await
}

/// The source is whatever the last accepted call set, over a TABLE of source values
/// and update sequences, and every read of the campaign projects it.
///
/// The rows differ along what the rule generalises over: which member of the `Source`
/// union is in force (`customerProfilesSegmentArn`, `eventTrigger` carrying a domain
/// ARN, `eventTrigger` carrying none), whether the campaign was created sourced
/// already, how many successive updates it takes, whether those updates switch the
/// member in force, and the shape of the ARN — partition, region, resource-path depth,
/// an ARN qualifier, and both published bounds of `Arn`'s `@length` 20..500.
#[tokio::test]
async fn a_source_is_stored_and_every_read_projects_it() {
    let region = "us-west-2";
    let router = router();
    let instance = "campaign-source-instance";

    // (case, the source the create carried, the sources to set in order)
    let cases: Vec<(&str, Option<Value>, Vec<Value>)> = vec![
        (
            "a campaign created with no source takes a segment ARN",
            None,
            vec![json!({"customerProfilesSegmentArn":
                "arn:aws:profile:us-west-2:000000000000:domains/d-1/segments/s-1"})],
        ),
        (
            "replaces the source the create carried",
            Some(json!({"customerProfilesSegmentArn":
                "arn:aws:profile:us-west-2:000000000000:domains/d-2/segments/old"})),
            vec![json!({"customerProfilesSegmentArn":
                "arn:aws:profile:us-west-2:000000000000:domains/d-2/segments/new"})],
        ),
        (
            "an event trigger carrying a domain ARN",
            None,
            vec![json!({"eventTrigger": {"customerProfilesDomainArn":
                "arn:aws:profile:us-west-2:000000000000:domains/d-3"}})],
        ),
        (
            "an event trigger carrying no domain ARN at all",
            None,
            vec![json!({"eventTrigger": {}})],
        ),
        (
            "the union member in force switches, both ways",
            Some(json!({"customerProfilesSegmentArn":
                "arn:aws:profile:us-west-2:000000000000:domains/d-4/segments/s-4"})),
            vec![
                json!({"eventTrigger": {"customerProfilesDomainArn":
                    "arn:aws:profile:us-west-2:000000000000:domains/d-4"}}),
                json!({"customerProfilesSegmentArn":
                    "arn:aws:profile:us-west-2:000000000000:domains/d-4/segments/back"}),
                json!({"eventTrigger": {}}),
            ],
        ),
        (
            "three successive segment ARNs: the last one is the source",
            None,
            vec![
                json!({"customerProfilesSegmentArn":
                    "arn:aws:profile:us-west-2:000000000000:domains/d-5/segments/first"}),
                json!({"customerProfilesSegmentArn":
                    "arn:aws:profile:us-west-2:000000000000:domains/d-5/segments/second"}),
                json!({"customerProfilesSegmentArn":
                    "arn:aws:profile:us-west-2:000000000000:domains/d-5/segments/third"}),
            ],
        ),
        (
            "another partition and a nested resource path with a qualifier",
            None,
            // `aws-us-gov` with a commercial-style region: `Arn`'s `@pattern` spells
            // the region field `[a-z]{2}-[a-z]+-\d{1,2}`, which a GovCloud region's
            // second hyphen group is outside — the same spelling the flow-ARN suite
            // uses for the partition, where the boundary walk enforces that pattern.
            vec![json!({"customerProfilesSegmentArn":
                "arn:aws-us-gov:profile:us-west-1:000000000000:domains/d-6/segments/s-6:v2"})],
        ),
        (
            "a China-partition ARN behind the event trigger",
            None,
            vec![json!({"eventTrigger": {"customerProfilesDomainArn":
                "arn:aws-cn:profile:cn-north-1:000000000000:domains/d-7"}})],
        ),
        (
            "the 20-character lower bound of Arn's @length",
            None,
            // Hand-written rather than padded: 20 characters is shorter than the
            // ARN head `arn_of_length` builds from.
            vec![json!({"customerProfilesSegmentArn": "arn:a:a:aa-aa-1:a:xx"})],
        ),
        (
            "the 500-character upper bound of Arn's @length",
            None,
            vec![json!({"customerProfilesSegmentArn": padded_arn(SEGMENT_ARN_HEAD, 500)})],
        ),
    ];

    for (index, (case, created_with, sequence)) in cases.iter().enumerate() {
        let id = create_sourced_campaign(
            &router,
            region,
            instance,
            &format!("campaign-{index}"),
            created_with.as_ref(),
            None,
        )
        .await;
        let before = describe_campaign(&router, region, &id, case).await;

        for source in sequence {
            let (status, body) =
                update_source(&router, region, &id, &json!({"source": source})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateCampaignSource expected 200, got {status} {body}"
            );

            let campaign = describe_campaign(&router, region, &id, case).await;
            assert_eq!(
                campaign["source"], *source,
                "case {case:?}: expected the source {source}, got {:?}",
                campaign["source"]
            );
        }

        let last = sequence.last().expect("every case sets at least one source");

        // Idempotent: the same source again is the same source, and still a 200.
        let (status, body) = update_source(&router, region, &id, &json!({"source": last})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated UpdateCampaignSource expected 200, got {status} {body}"
        );

        let after = describe_campaign(&router, region, &id, case).await;
        assert_eq!(
            after["source"], *last,
            "case {case:?}: expected the repeat to leave the source {last}, got {:?}",
            after["source"]
        );

        // The call changes the source and NOTHING else about the campaign.
        let before_rest = campaign_without(&before, "source");
        let after_rest = campaign_without(&after, "source");
        assert_eq!(
            before_rest, after_rest,
            "case {case:?}: expected every other member untouched, got {after_rest} \
             (was {before_rest})"
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and sources nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_takes_no_source() {
    let region = "eu-west-1";
    let router = router();
    let instance = "campaign-source-404-instance";

    let source = json!({"customerProfilesSegmentArn":
        "arn:aws:profile:eu-west-1:000000000000:domains/d-1/segments/s-1"});

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let control = create_sourced_campaign(&router, region, instance, "control", None, None).await;
    let (status, body) = update_source(&router, region, &control, &json!({"source": source})).await;
    assert_eq!(
        status, 200,
        "case \"control campaign\": expected 200, got {status} {body}"
    );

    // One campaign created and then deleted: its id was valid a moment ago.
    let deleted = create_sourced_campaign(&router, region, instance, "doomed", None, None).await;
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
        let (status, body) = update_source(&router, region, &id, &json!({"source": source})).await;
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

        // A refused call sources nothing: the id is still absent...
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
        let campaign = describe_campaign(&router, region, &control, &case).await;
        assert_eq!(
            campaign["source"], source,
            "case {case:?}: expected the control campaign's source untouched, got {:?}",
            campaign["source"]
        );
    }
}

/// A request that does not carry the one member the model requires is refused as a bad
/// request before the handler runs, and the stored source is left alone.
///
/// The rows differ along how the required `source` is missing: omitted entirely, sent
/// as an empty union (no member in force), and sent with no body at all.
#[tokio::test]
async fn a_request_without_a_source_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "campaign-source-validation-instance";

    let stored = json!({"customerProfilesSegmentArn":
        "arn:aws:profile:us-east-1:000000000000:domains/d-1/segments/kept"});
    let id = create_sourced_campaign(
        &router,
        region,
        instance,
        "validated",
        Some(&stored),
        None,
    )
    .await;

    // (case, request body)
    let cases: Vec<(&str, Value)> = vec![
        ("the required member omitted", json!({})),
        ("the union carries no member", json!({"source": {}})),
        ("no body at all", Value::Null),
    ];

    for (case, body) in cases {
        let (status, response) = update_source(&router, region, &id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&response)
        );
        let campaign = describe_campaign(&router, region, &id, case).await;
        assert_eq!(
            campaign["source"], stored,
            "case {case:?}: expected the refusal to leave {stored} stored, got {:?}",
            campaign["source"]
        );
    }
}

/// Which lifecycle states take a change of source: every non-terminal one, and
/// neither terminal one.
///
/// The table spans the whole `CampaignState` catalog (source: `CAMPAIGN_STATE_VALUES`,
/// the model's `CampaignState` enum) against the shared predicate the configuration
/// operations refuse through, then drives the update over HTTP in each state the
/// campaign lifecycle can actually reach, and finally against a campaign the CLOCK has
/// carried into a terminal state — where the operation's
/// `InvalidCampaignStateException` is observable.
#[tokio::test]
async fn every_non_terminal_state_takes_a_source_and_a_terminal_one_refuses_it() {
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

    // The table above is exhaustive over the model's own catalog, so a state the
    // model gains cannot slip past it unjudged.
    let judged: Vec<&str> = catalog.iter().map(|(state, _)| state.as_wire()).collect();
    assert_eq!(
        judged.len(),
        CAMPAIGN_STATE_VALUES.len(),
        "case \"the catalog\": expected {CAMPAIGN_STATE_VALUES:?} to be judged, got {judged:?}"
    );
    for value in CAMPAIGN_STATE_VALUES {
        assert!(
            judged.contains(value),
            "case {value:?}: expected the state to be judged, got {judged:?}"
        );
    }

    let region = "ap-south-1";
    let router = router();
    let instance = "campaign-source-state-instance";
    let id = create_sourced_campaign(&router, region, instance, "stateful", None, None).await;

    // Each lifecycle action the emulator's state machine offers, with the state it
    // leaves the campaign in: the update is accepted in every one of them.
    let walk: &[(&str, CampaignState)] = &[
        ("", CampaignState::Initialized),
        ("start", CampaignState::Running),
        ("pause", CampaignState::Paused),
        ("stop", CampaignState::Stopped),
    ];
    for (action, state) in walk {
        if !action.is_empty() {
            drive_lifecycle(&router, region, &id, &[action], action).await;
        }
        let case = format!("in state {}", state.as_wire());
        assert_eq!(
            campaign_state(&router, region, &id, &case).await,
            state.as_wire(),
            "case {case:?}: expected state {:?}",
            state.as_wire()
        );

        let source = json!({"customerProfilesSegmentArn": format!(
            "arn:aws:profile:ap-south-1:000000000000:domains/d-1/segments/{}",
            state.as_wire()
        )});
        let (status, body) =
            update_source(&router, region, &id, &json!({"source": source})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the source to be accepted, got {status} {body}"
        );
        let campaign = describe_campaign(&router, region, &id, &case).await;
        assert_eq!(
            campaign["source"], source,
            "case {case:?}: expected the source {source}, got {:?}",
            campaign["source"]
        );
    }

    // A campaign started while its run window was open, whose window has since
    // closed, is `Completed` — a terminal state the clock wrote, nobody else — so it
    // is past being reconfigured.
    let stored = json!({"eventTrigger": {"customerProfilesDomainArn":
        "arn:aws:profile:ap-south-1:000000000000:domains/kept"}});
    let completed = create_sourced_campaign(
        &router,
        region,
        instance,
        "completed",
        Some(&stored),
        Some((-7_200.0, -3_600.0)),
    )
    .await;
    drive_lifecycle(&router, region, &completed, &["start"], "terminal state").await;

    let case = "a campaign past its run window";
    let (status, refused) = update_source(
        &router,
        region,
        &completed,
        &json!({"source": {"customerProfilesSegmentArn":
            "arn:aws:profile:ap-south-1:000000000000:domains/d-2/segments/rejected"}}),
    )
    .await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected HTTP 409, got {status} {refused}"
    );
    assert!(
        error_code(&refused).ends_with("InvalidCampaignStateException"),
        "case {case:?}: expected InvalidCampaignStateException, got {}",
        error_code(&refused)
    );
    assert_eq!(
        refused["state"].as_str(),
        Some(CampaignState::Completed.as_wire()),
        "case {case:?}: expected the refusal to name the state the campaign is at, got {:?}",
        refused["state"]
    );
    let campaign = describe_campaign(&router, region, &completed, case).await;
    assert_eq!(
        campaign["source"], stored,
        "case {case:?}: expected the refusal to leave {stored} stored, got {:?}",
        campaign["source"]
    );
}
