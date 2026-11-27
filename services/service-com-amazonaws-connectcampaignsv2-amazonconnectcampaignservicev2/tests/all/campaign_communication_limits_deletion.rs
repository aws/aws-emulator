// SPDX-License-Identifier: CC0-1.0

//! `DeleteCampaignCommunicationLimits`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call removes the communication-limits block that the campaign named by `{id}`
//!   holds in its stored `communicationLimitsOverride` for the ONE config `config`
//!   names, and nothing else: every other member of that override — and every other
//!   member of the campaign — stays as it was, which is what every later read
//!   (`DescribeCampaign`) projects. An override a REMOVAL left with no member at all is
//!   absent rather than empty;
//! * the operation is idempotent (`@idempotent` in the model, "This API is idempotent"
//!   in its documentation): a config whose block is not set, and a campaign carrying no
//!   override at all, are a 200 that removes nothing — and a call that removed nothing
//!   writes nothing, so the record is left exactly as it was;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and removes
//!   nothing;
//! * a `config` outside the model's `CommunicationLimitsConfigType` catalog is HTTP 400
//!   `ValidationException` and removes nothing;
//! * a campaign in a non-terminal lifecycle state takes the deletion.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES, COMMUNICATION_LIMIT_TIME_UNIT_VALUES,
    INSTANCE_LIMITS_HANDLING_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    campaign_id_not_found_message, CampaignState, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_without, error_code, router, Harness};

/// The `CommunicationLimitsConfig` member that carries the block of one
/// `CommunicationLimitsConfigType` value.
///
/// Source: the `CommunicationLimitsConfig` shape, whose only per-config member is
/// `allChannelSubtypes`; `instanceLimitsHandling` is not a config type.
fn member_of(config_type: &str) -> &'static str {
    match config_type {
        "ALL_CHANNEL_SUBTYPES" => "allChannelSubtypes",
        other => panic!("{other} is not a CommunicationLimitsConfigType value"),
    }
}

/// One `CommunicationLimit` entry.
fn limit(max_count_per_recipient: i64, frequency: i64, unit: &str) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        "unit": unit,
    })
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
        "channelSubtypeConfig": telephony_subtype_config(),
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

/// `DeleteCampaignCommunicationLimits` for `id` and the `config` config type.
async fn delete_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{id}/communication-limits?config={config}"),
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

/// The override the store should hold after deleting `config_type` from `created`:
/// `created` minus that config's own member, and absent (`Value::Null`, the shape the
/// projection carries for a member it does not hold) once no member is left behind. A
/// delete that found no block for `config_type` removed nothing, so it writes nothing
/// and `created` stays exactly as it was — but an override holding no member at all
/// overrides nothing, so a campaign created with one never carried it to begin with.
fn expected_override(created: Option<&Value>, config_type: &str) -> Value {
    let Some(created) = created else {
        return Value::Null;
    };
    let mut expected = created.clone();
    expected
        .as_object_mut()
        .expect("a communication limits config is an object")
        .remove(member_of(config_type));
    if expected
        .as_object()
        .expect("a communication limits config is an object")
        .is_empty()
    {
        return Value::Null;
    }
    expected
}

/// Deleting one config type removes exactly that block, over a TABLE spanning the whole
/// `CommunicationLimitsConfigType` catalog (`COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES`,
/// taken from the model) against overrides that differ in which members they set, how
/// many `communicationLimitsList` entries they carry (the `@length` max is 2), which
/// `InstanceLimitsHandling` value they set (source: `INSTANCE_LIMITS_HANDLING_VALUES`)
/// and the bounds of `frequency`'s `@range` 1..30.
#[tokio::test]
async fn deleting_one_config_leaves_every_other_part_of_the_campaign() {
    let region = "us-west-2";
    let router = router();
    let instance = "communication-limits-deletion-instance";
    onboard(&router, region, instance).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let opt_in = INSTANCE_LIMITS_HANDLING_VALUES[0];
    // The catalog entry this issue did not cite.
    let opt_out = INSTANCE_LIMITS_HANDLING_VALUES[1];

    // (case, the override the campaign was created with)
    let configurations: Vec<(&str, Option<Value>)> = vec![
        (
            "one limit entry only",
            Some(json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(5, 7, day)]}
            })),
        ),
        (
            "two entries, the @length maximum, at both bounds of frequency's 1..30",
            Some(json!({
                "allChannelSubtypes": {
                    "communicationLimitsList": [limit(1, 1, day), limit(10, 30, day)]
                }
            })),
        ),
        (
            "limits and instanceLimitsHandling together",
            Some(json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(3, 2, day)]},
                "instanceLimitsHandling": opt_in
            })),
        ),
        (
            "limits and the other handling value together",
            Some(json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(4, 3, day)]},
                "instanceLimitsHandling": opt_out
            })),
        ),
        (
            "instanceLimitsHandling alone, no block for any config type",
            Some(json!({"instanceLimitsHandling": opt_out})),
        ),
        ("an override with no member at all", Some(json!({}))),
        ("no override at all", None),
    ];

    for (shape, (configuration, created)) in configurations.iter().enumerate() {
        // Every catalog value is deleted against every configuration, including the
        // ones that never held a block for it.
        for target in COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES {
            let case = format!("{configuration}, delete {target}");
            let id = create_campaign(
                &router,
                region,
                instance,
                &format!("campaign-{shape}-{target}"),
                created.as_ref(),
            )
            .await;
            let before = describe(&router, region, &id, &case).await;

            let (status, body) = delete_limits(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteCampaignCommunicationLimits expected 200, \
                 got {status} {body}"
            );

            let expected = expected_override(created.as_ref(), target);
            let after = describe(&router, region, &id, &case).await;
            assert_eq!(
                after["communicationLimitsOverride"], expected,
                "case {case:?}: expected {expected}, got {}",
                after["communicationLimitsOverride"]
            );
            assert!(
                after["communicationLimitsOverride"]
                    .get(member_of(target))
                    .is_none(),
                "case {case:?}: expected no {} block, got {}",
                member_of(target),
                after["communicationLimitsOverride"]
            );

            // Idempotent: the same deletion again is a 200 and the same override.
            let (status, body) = delete_limits(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated deletion expected 200, got {status} {body}"
            );
            let repeated = describe(&router, region, &id, &case).await;
            assert_eq!(
                repeated["communicationLimitsOverride"], expected,
                "case {case:?}: expected the repeat to leave {expected}, got {}",
                repeated["communicationLimitsOverride"]
            );

            // The call changes the communication limits override and NOTHING else about
            // the campaign.
            let before_rest = campaign_without(&before, "communicationLimitsOverride");
            let after_rest = campaign_without(&repeated, "communicationLimitsOverride");
            assert_eq!(
                before_rest, after_rest,
                "case {case:?}: expected every other member untouched, got {after_rest} \
                 (was {before_rest})"
            );
        }
    }
}

/// The deletion and `UpdateCampaignCommunicationLimits` round-trip against one
/// campaign: what the update stores is what the delete removes, and the campaign takes
/// a new override afterwards — the delete removes the block rather than blocking the
/// member.
#[tokio::test]
async fn an_override_can_be_set_deleted_and_set_again() {
    let region = "eu-central-1";
    let router = router();
    let instance = "communication-limits-roundtrip-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "roundtrip", None).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let target = COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0];

    // (case, the override to store before the deletion)
    let rounds: Vec<(&str, Value)> = vec![
        (
            "one entry",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(2, 4, day)]}}),
        ),
        (
            "two entries",
            json!({"allChannelSubtypes": {
                "communicationLimitsList": [limit(1, 1, day), limit(6, 30, day)]
            }}),
        ),
        (
            "limits beside instanceLimitsHandling",
            json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(9, 12, day)]},
                "instanceLimitsHandling": INSTANCE_LIMITS_HANDLING_VALUES[1]
            }),
        ),
    ];

    for (case, config) in rounds {
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/communication-limits"),
            &json!({"communicationLimitsOverride": config}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignCommunicationLimits expected 200, got {status} {body}"
        );
        let stored = describe(&router, region, &id, case).await;
        assert_eq!(
            stored["communicationLimitsOverride"], config,
            "case {case:?}: expected the update to store {config}, got {}",
            stored["communicationLimitsOverride"]
        );

        let (status, body) = delete_limits(&router, region, &id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deletion expected 200, got {status} {body}"
        );

        let expected = expected_override(Some(&config), target);
        let after = describe(&router, region, &id, case).await;
        assert_eq!(
            after["communicationLimitsOverride"], expected,
            "case {case:?}: expected {expected} after the deletion, got {}",
            after["communicationLimitsOverride"]
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and removes nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_loses_no_limits() {
    let region = "eu-west-1";
    let router = router();
    let instance = "communication-limits-deletion-404-instance";
    onboard(&router, region, instance).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let kept = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(8, 9, day)]},
        "instanceLimitsHandling": INSTANCE_LIMITS_HANDLING_VALUES[0]
    });

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
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
        let (status, body) =
            delete_limits(&router, region, &id, COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0]).await;
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
        // override it was created with.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["communicationLimitsOverride"], kept,
            "case {case:?}: expected the control campaign's override untouched, got {}",
            campaign["communicationLimitsOverride"]
        );
    }
}

/// A `config` outside the model's `CommunicationLimitsConfigType` catalog names no
/// communication-limits config, so it is refused as a bad request and removes nothing.
///
/// The rows differ along what the rule generalises over: a value from a NEIGHBOURING
/// catalog (`CommunicationTimeConfigType`'s channels), a plausible config name the
/// catalog does not hold, a catalog value in the wrong case, and an empty value. A
/// request carrying no `config` at all is left out: the generated routing table
/// resolves this route by its required query parameter, so such a request names no
/// operation and never reaches this handler.
#[tokio::test]
async fn a_config_outside_the_catalog_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "communication-limits-deletion-validation-instance";
    onboard(&router, region, instance).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let kept = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(7, 7, day)]}
    });
    let id = create_campaign(&router, region, instance, "validated", Some(&kept)).await;

    // (case, the query string the request carries)
    let cases: Vec<(&str, &str)> = vec![
        ("a value from the communication time catalog", "?config=TELEPHONY"),
        ("a plausible name this catalog does not hold", "?config=ALL_CHANNELS"),
        ("the catalog value in the wrong case", "?config=all_channel_subtypes"),
        ("an empty value", "?config="),
    ];

    for (case, query) in cases {
        let (status, response) = call(
            &router,
            region,
            "DELETE",
            &format!("/v2/campaigns/{id}/communication-limits{query}"),
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
            campaign["communicationLimitsOverride"], kept,
            "case {case:?}: expected the refusal to leave the override alone, got {}",
            campaign["communicationLimitsOverride"]
        );
    }
}

/// Which lifecycle states take the deletion: every non-terminal one, through the
/// shared predicate the configuration operations refuse on.
///
/// The table spans the whole `CampaignState` catalog against that predicate, and then
/// drives the deletion over HTTP in each state the campaign lifecycle can actually
/// reach, which is where the answer is observable.
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
    let instance = "communication-limits-deletion-state-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "stateful", None).await;

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let target = COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES[0];

    // Each lifecycle action the emulator's state machine offers, with the state it
    // leaves the campaign in. The frequency differs per step so each assertion observes
    // that step's own write being removed.
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
            "case {case:?}: expected state {:?}, got {}",
            state.as_wire(),
            read["state"]
        );

        let config = json!({
            "allChannelSubtypes": {"communicationLimitsList": [limit(1, *frequency, day)]}
        });
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/communication-limits"),
            &json!({"communicationLimitsOverride": config}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body}"
        );

        let (status, body) = delete_limits(&router, region, &id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deletion to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert!(
            campaign.get("communicationLimitsOverride").is_none(),
            "case {case:?}: expected no communicationLimitsOverride, got {:?}",
            campaign.get("communicationLimitsOverride")
        );
    }
}
