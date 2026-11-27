// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignFlowAssociation`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call ASSOCIATES the `connectCampaignFlowArn` it was given with the campaign
//!   `{id}` names, replacing whatever flow that campaign was associated with, and the
//!   association is what every later read of the campaign (`DescribeCampaign`,
//!   `ListCampaigns`) projects — nothing else about the campaign changes;
//! * the operation is idempotent (`@idempotent` in the model, "This API is
//!   idempotent" in its documentation): repeating it with the same ARN leaves the
//!   same association;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and
//!   associates nothing;
//! * an ARN outside `Arn`'s modelled constraints (len 20..500, the `^arn:…$` pattern)
//!   is HTTP 400 `ValidationException` and leaves the stored association alone;
//! * a campaign in a terminal lifecycle state takes no configuration change, which is
//!   this operation's `InvalidCampaignStateException`.

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

/// Create a campaign on an onboarded `instance`, optionally already associated with
/// `flow_arn`, and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    flow_arn: Option<&str>,
) -> String {
    let mut body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_config(),
    });
    if let Some(flow_arn) = flow_arn {
        body["connectCampaignFlowArn"] = json!(flow_arn);
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

/// `UpdateCampaignFlowAssociation` for `id`.
async fn associate(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/flow"),
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

/// The `connectCampaignFlowArn` the summary of `id` carries in `ListCampaigns`, or
/// `None` when the summary omits it.
async fn summary_flow_arn(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> Option<String> {
    let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {body}"
    );
    body["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .and_then(|summary| summary["connectCampaignFlowArn"].as_str())
        .map(str::to_string)
}

/// An ARN of exactly `len` characters, matching `Arn`'s pattern: the fixed
/// `arn:<partition>:<service>:<region>:<account>:` head (44 characters) and a
/// `flow/<name>` resource padded out to the requested length. Used for the published
/// bounds of `Arn`'s `@length` 20..500 and one character past the upper one.
fn arn_of_length(len: usize) -> String {
    let mut arn = String::from("arn:aws:connect:us-east-1:000000000000:flow/");
    assert!(
        len > arn.len(),
        "arn_of_length({len}): too short for the ARN head"
    );
    while arn.len() < len {
        arn.push('a');
    }
    arn
}

/// The association is whatever the last accepted call named, over a TABLE of flow
/// ARNs and update sequences, and every read of the campaign projects it.
///
/// The rows differ along what the rule generalises over: whether the campaign was
/// created with an association already, how many successive updates it takes, and the
/// shape of the ARN — partition, region, resource-path depth, an ARN qualifier, and
/// both published bounds of `Arn`'s `@length` 20..500 (source: the `Arn` shape
/// `UpdateCampaignFlowAssociation$connectCampaignFlowArn` targets).
#[tokio::test]
async fn an_association_is_stored_and_every_read_projects_it() {
    let region = "us-west-2";
    let router = router();
    let instance = "flow-association-instance";
    onboard(&router, region, instance).await;

    // (case, flow ARN the create carried, the ARNs to associate in order)
    let cases: Vec<(&str, Option<&str>, Vec<String>)> = vec![
        (
            "a campaign created with no association",
            None,
            vec!["arn:aws:connect:us-west-2:000000000000:instance/i-1/flow/f-1".to_string()],
        ),
        (
            "replaces the association the create carried",
            Some("arn:aws:connect:us-west-2:000000000000:instance/i-2/flow/old"),
            vec!["arn:aws:connect:us-west-2:000000000000:instance/i-2/flow/new".to_string()],
        ),
        (
            "three successive updates: the last one is the association",
            None,
            vec![
                "arn:aws:connect:us-west-2:000000000000:instance/i-3/flow/first".to_string(),
                "arn:aws:connect:us-west-2:000000000000:instance/i-3/flow/second".to_string(),
                "arn:aws:connect:us-west-2:000000000000:instance/i-3/flow/third".to_string(),
            ],
        ),
        (
            "another partition and a nested resource path with a qualifier",
            None,
            vec![
                "arn:aws-us-gov:connect:us-west-1:000000000000:instance/i-4/flow/f-4:prod"
                    .to_string(),
            ],
        ),
        (
            "a China-partition ARN",
            None,
            vec!["arn:aws-cn:connect:cn-north-1:000000000000:instance/i-5/flow/f-5".to_string()],
        ),
        (
            "the 20-character lower bound of Arn's @length",
            None,
            // Hand-written rather than padded: 20 characters is shorter than the
            // ARN head `arn_of_length` builds from.
            vec!["arn:a:a:aa-aa-1:a:xx".to_string()],
        ),
        (
            "the 500-character upper bound of Arn's @length",
            None,
            vec![arn_of_length(500)],
        ),
    ];

    for (index, (case, created_with, sequence)) in cases.iter().enumerate() {
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("campaign-{index}"),
            *created_with,
        )
        .await;
        let before = describe(&router, region, &id, case).await;

        for arn in sequence {
            let (status, body) =
                associate(&router, region, &id, &json!({"connectCampaignFlowArn": arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateCampaignFlowAssociation expected 200, got {status} {body}"
            );

            let campaign = describe(&router, region, &id, case).await;
            assert_eq!(
                campaign["connectCampaignFlowArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the association {arn:?}, got {:?}",
                campaign["connectCampaignFlowArn"]
            );
        }

        let last = sequence
            .last()
            .expect("every case associates at least one flow");

        // Idempotent: the same ARN again is the same association, and still a 200.
        let (status, body) =
            associate(&router, region, &id, &json!({"connectCampaignFlowArn": last})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated association expected 200, got {status} {body}"
        );

        let after = describe(&router, region, &id, case).await;
        assert_eq!(
            after["connectCampaignFlowArn"].as_str(),
            Some(last.as_str()),
            "case {case:?}: expected the repeat to leave the association {last:?}, got {:?}",
            after["connectCampaignFlowArn"]
        );
        assert_eq!(
            summary_flow_arn(&router, region, &id, case).await,
            Some(last.clone()),
            "case {case:?}: expected the ListCampaigns summary to carry {last:?}"
        );

        // The call changes the association and NOTHING else about the campaign.
        let before_rest = campaign_without(&before, "connectCampaignFlowArn");
        let after_rest = campaign_without(&after, "connectCampaignFlowArn");
        assert_eq!(
            before_rest, after_rest,
            "case {case:?}: expected every other member untouched, got {after_rest} \
             (was {before_rest})"
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and associates nothing — asserted over a table of well-formed `CampaignId`s
/// the store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_takes_no_association() {
    let region = "eu-west-1";
    let router = router();
    let instance = "flow-association-404-instance";
    onboard(&router, region, instance).await;

    let flow = "arn:aws:connect:eu-west-1:000000000000:instance/i-1/flow/f-1";

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let control = create_campaign(&router, region, instance, "control", None).await;
    let (status, body) = associate(
        &router,
        region,
        &control,
        &json!({"connectCampaignFlowArn": flow}),
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
        let (status, body) = associate(
            &router,
            region,
            &id,
            &json!({"connectCampaignFlowArn": flow}),
        )
        .await;
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

        // A refused call associates nothing: the id is still absent...
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
            campaign["connectCampaignFlowArn"].as_str(),
            Some(flow),
            "case {case:?}: expected the control campaign's association untouched, got {:?}",
            campaign["connectCampaignFlowArn"]
        );
    }
}

/// An ARN outside `Arn`'s modelled constraints is refused as a bad request before the
/// handler runs, and the stored association is left alone.
///
/// The rows differ along what the rule generalises over: one character past each
/// published bound of `@length` 20..500, text that is not an ARN at all, and the
/// required member omitted.
#[tokio::test]
async fn an_arn_outside_the_modelled_constraints_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "flow-association-validation-instance";
    onboard(&router, region, instance).await;

    let stored = "arn:aws:connect:us-east-1:000000000000:instance/i-1/flow/kept";
    let id = create_campaign(&router, region, instance, "validated", Some(stored)).await;

    // (case, request body)
    let cases: Vec<(String, Value)> = vec![
        (
            "19 characters, one below the lower bound".to_string(),
            json!({"connectCampaignFlowArn": "arn:a:a:aa-aa-1:a:x"}),
        ),
        (
            "501 characters, one past the upper bound".to_string(),
            json!({"connectCampaignFlowArn": arn_of_length(501)}),
        ),
        (
            "not an ARN at all".to_string(),
            json!({"connectCampaignFlowArn": "just-a-flow-name-not-an-arn"}),
        ),
        (
            "an ARN with no resource segment".to_string(),
            json!({"connectCampaignFlowArn": "arn:aws:connect:us-east-1:000000000000:"}),
        ),
        (
            "the required member omitted".to_string(),
            json!({}),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = associate(&router, region, &id, &body).await;
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
            campaign["connectCampaignFlowArn"].as_str(),
            Some(stored),
            "case {case:?}: expected the refusal to leave {stored:?} associated, got {:?}",
            campaign["connectCampaignFlowArn"]
        );
    }
}

/// Which lifecycle states take a configuration change: every non-terminal one, and
/// neither terminal one.
///
/// The table spans the whole `CampaignState` catalog (source: `CAMPAIGN_STATE_VALUES`,
/// the model's `CampaignState` enum) against the shared predicate the configuration
/// operations refuse through, and then drives the update over HTTP in each state the
/// campaign lifecycle can actually reach, which is where the operation's answer is
/// observable.
#[tokio::test]
async fn every_non_terminal_state_takes_an_association() {
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
    let instance = "flow-association-state-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "stateful", None).await;

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

        let arn = format!(
            "arn:aws:connect:ap-south-1:000000000000:instance/i-1/flow/{}",
            state.as_wire()
        );
        let (status, body) =
            associate(&router, region, &id, &json!({"connectCampaignFlowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the association to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["connectCampaignFlowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the association {arn:?}, got {:?}",
            campaign["connectCampaignFlowArn"]
        );
    }
}
