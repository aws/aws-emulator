// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignCommunicationLimits`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call REPLACES the campaign's `communicationLimitsOverride` with the config it
//!   was given, whatever that campaign held before, and the stored config is what every
//!   later read of the campaign (`DescribeCampaign`) projects — nothing else about the
//!   campaign changes;
//! * the operation is idempotent ("This API is idempotent", per the model's own
//!   documentation): sending the same config again leaves the same config stored;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and stores
//!   nothing;
//! * a config outside `CommunicationLimit`'s modelled constraints (`frequency` in
//!   1..30, `maxCountPerRecipient` at least 1, `unit` from
//!   `CommunicationLimitTimeUnit`, a `communicationLimitsList` of at most 2) is HTTP 400
//!   `ValidationException` and leaves the stored config alone;
//! * a campaign in a terminal lifecycle state takes no configuration change, which is
//!   this operation's `InvalidCampaignStateException`.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES, INSTANCE_LIMITS_HANDLING_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    campaign_id_not_found_message, CampaignState, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_without, error_code, router, stored_config, Harness};

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

/// One `CommunicationLimit` entry.
fn limit(max_count_per_recipient: i64, frequency: i64, unit: &str) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        "unit": unit,
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
/// `communication_limits_override`, and hand back its id.
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
        body["communicationLimitsOverride"] = limits.clone();
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

/// `UpdateCampaignCommunicationLimits` for `id`.
async fn update_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/communication-limits"),
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

/// The stored override is whatever the last accepted call carried, over a TABLE of
/// configs, and `DescribeCampaign` projects it verbatim.
///
/// The rows differ along what the rule generalises over: whether the campaign was
/// created with an override already, how many successive updates it takes, how many
/// entries the `communicationLimitsList` carries (its `@length` max is 2), which
/// `instanceLimitsHandling` value is set (source: the model's `InstanceLimitsHandling`
/// enum, `INSTANCE_LIMITS_HANDLING_VALUES`), the `unit` (source:
/// `COMMUNICATION_LIMIT_TIME_UNIT_VALUES`) and both published bounds of `frequency`'s
/// `@range` 1..30 together with `maxCountPerRecipient`'s minimum of 1.
#[tokio::test]
async fn a_communication_limits_config_is_stored_and_every_read_projects_it() {
    let region = "us-west-2";
    let router = router();
    let instance = "communication-limits-instance";
    onboard(&router, region, instance).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let opt_in = INSTANCE_LIMITS_HANDLING_VALUES[0];
    // `OPT_OUT` is the catalog entry this issue did not cite.
    let opt_out = INSTANCE_LIMITS_HANDLING_VALUES[1];

    // (case, the override the create carried, the overrides to send in order)
    let cases: Vec<(&str, Option<Value>, Vec<Value>)> = vec![
        (
            "a campaign created with no override takes one limit",
            None,
            vec![json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(5, 7, day)]}
            })],
        ),
        (
            "replaces the override the create carried",
            Some(json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, day)]}
            })),
            vec![json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(9, 14, day)]}
            })],
        ),
        (
            "two entries, the @length maximum of communicationLimitsList",
            None,
            vec![json!({
                "allChannelSubtypes": {
                    "communicationLimitsList": [limit(2, 1, day), limit(10, 30, day)]
                }
            })],
        ),
        (
            "both published bounds of frequency's @range 1..30 and the minimum count",
            None,
            vec![
                json!({
                    "allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, day)]}
                }),
                json!({
                    "allChannelSubtypes": {"communicationLimitsList": [limit(1, 30, day)]}
                }),
            ],
        ),
        (
            "instanceLimitsHandling alone, with no per-channel limits",
            None,
            vec![json!({"instanceLimitsHandling": opt_in})],
        ),
        (
            "both members together, then the other handling value",
            None,
            vec![
                json!({
                    "allChannelSubtypes": {"communicationLimitsList": [limit(3, 2, day)]},
                    "instanceLimitsHandling": opt_in
                }),
                json!({
                    "allChannelSubtypes": {"communicationLimitsList": [limit(4, 3, day)]},
                    "instanceLimitsHandling": opt_out
                }),
            ],
        ),
        (
            "an empty config: the shape has no required member, so it overrides nothing",
            Some(json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(6, 6, day)]}
            })),
            vec![json!({})],
        ),
        (
            "a large maxCountPerRecipient",
            None,
            vec![json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(1_000_000, 30, day)]}
            })],
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
            let (status, body) = update_limits(
                &router,
                region,
                &id,
                &json!({"communicationLimitsOverride": config}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateCampaignCommunicationLimits expected 200, \
                 got {status} {body}"
            );

            let campaign = describe(&router, region, &id, case).await;
            assert_eq!(
                campaign["communicationLimitsOverride"],
                stored_config(config),
                "case {case:?}: expected the stored override {}, got {}",
                stored_config(config),
                campaign["communicationLimitsOverride"]
            );
        }

        let last = sequence.last().expect("every case sends at least one config");

        // Idempotent: the same config again is the same write, and still a 200.
        let (status, body) = update_limits(
            &router,
            region,
            &id,
            &json!({"communicationLimitsOverride": last}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated update expected 200, got {status} {body}"
        );

        let after = describe(&router, region, &id, case).await;
        assert_eq!(
            after["communicationLimitsOverride"],
            stored_config(last),
            "case {case:?}: expected the repeat to leave {}, got {}",
            stored_config(last),
            after["communicationLimitsOverride"]
        );

        // The call changes the override and NOTHING else about the campaign.
        let before_rest = campaign_without(&before, "communicationLimitsOverride");
        let after_rest = campaign_without(&after, "communicationLimitsOverride");
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
async fn an_unknown_campaign_is_not_found_and_takes_no_communication_limits() {
    let region = "eu-west-1";
    let router = router();
    let instance = "communication-limits-404-instance";
    onboard(&router, region, instance).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let config = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(8, 9, day)]}
    });

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let control = create_campaign(&router, region, instance, "control", None).await;
    let (status, body) = update_limits(
        &router,
        region,
        &control,
        &json!({"communicationLimitsOverride": config}),
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
        let (status, body) = update_limits(
            &router,
            region,
            &id,
            &json!({"communicationLimitsOverride": config}),
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
            campaign["communicationLimitsOverride"], config,
            "case {case:?}: expected the control campaign's override untouched, got {}",
            campaign["communicationLimitsOverride"]
        );
    }
}

/// A config outside the modelled constraints is refused as a bad request before the
/// handler runs, and the stored override is left alone.
///
/// The rows differ along what the rule generalises over: one past each published bound
/// of `frequency`'s `@range` 1..30 and of `maxCountPerRecipient`'s minimum of 1, one
/// past `communicationLimitsList`'s `@length` max of 2, a `unit` outside
/// `CommunicationLimitTimeUnit`'s catalog, each required member of
/// `CommunicationLimit` omitted, and the operation's own required member omitted.
#[tokio::test]
async fn a_config_outside_the_modelled_constraints_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "communication-limits-validation-instance";
    onboard(&router, region, instance).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let stored = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(7, 7, day)]}
    });
    let id = create_campaign(&router, region, instance, "validated", Some(&stored)).await;

    // (case, request body)
    let cases: Vec<(String, Value)> = vec![
        (
            "frequency 0, one below the lower bound".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [limit(1, 0, day)]}
            }}),
        ),
        (
            "frequency 31, one past the upper bound".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [limit(1, 31, day)]}
            }}),
        ),
        (
            "maxCountPerRecipient 0, one below its minimum".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [limit(0, 1, day)]}
            }}),
        ),
        (
            "three entries, one past communicationLimitsList's @length max".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [
                    limit(1, 1, day), limit(2, 2, day), limit(3, 3, day)
                ]}
            }}),
        ),
        (
            "a unit outside CommunicationLimitTimeUnit's catalog".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, "WEEK")]}
            }}),
        ),
        (
            "the required unit omitted".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [
                    {"maxCountPerRecipient": 1, "frequency": 1}
                ]}
            }}),
        ),
        (
            "the required frequency omitted".to_string(),
            json!({"communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [
                    {"maxCountPerRecipient": 1, "unit": day}
                ]}
            }}),
        ),
        (
            "the operation's required communicationLimitsOverride omitted".to_string(),
            json!({}),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = update_limits(&router, region, &id, &body).await;
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
            campaign["communicationLimitsOverride"], stored,
            "case {case:?}: expected the refusal to leave {stored} stored, got {}",
            campaign["communicationLimitsOverride"]
        );
    }

    // The bound belongs to the config shape, not to this one operation: every
    // operation that accepts a `CommunicationLimitsConfig` refuses an over-long list
    // the same way, `CreateCampaign` included.
    let case = "an over-long list at CreateCampaign";
    let (status, response) = call(
        &router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": "over-long-at-create",
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
            "communicationLimitsOverride": {
                "allChannelSubtypes": {"communicationLimitsList": [
                    limit(1, 1, day), limit(2, 2, day), limit(3, 3, day)
                ]}
            },
        }),
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
async fn every_non_terminal_state_takes_a_communication_limits_update() {
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
    let instance = "communication-limits-state-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];

    // Each lifecycle action the emulator's state machine offers, with the state it
    // leaves the campaign in: the update is accepted in every one of them. The
    // frequency differs per step so each assertion observes that step's own write.
    let walk: &[(&str, CampaignState, i64)] = &[
        ("", CampaignState::Initialized, 1),
        ("start", CampaignState::Running, 2),
        ("pause", CampaignState::Paused, 3),
        ("stop", CampaignState::Stopped, 4),
    ];
    for (action, state, frequency) in walk {
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

        let config = json!({
            "allChannelSubtypes": {"communicationLimitsList": [limit(1, *frequency, day)]}
        });
        let (status, body) = update_limits(
            &router,
            region,
            &id,
            &json!({"communicationLimitsOverride": config}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["communicationLimitsOverride"], config,
            "case {case:?}: expected the stored override {config}, got {}",
            campaign["communicationLimitsOverride"]
        );
    }
}
