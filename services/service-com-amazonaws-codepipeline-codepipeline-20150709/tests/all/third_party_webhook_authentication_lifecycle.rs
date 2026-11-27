// SPDX-License-Identifier: CC0-1.0

//! The whole life of a webhook the third-party operations cannot connect, over the wire
//! the service actually speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: the connection `RegisterWebhookWithThirdParty` makes and
//! `DeregisterWebhookWithThirdParty` breaks is carried by the shared secret of the
//! GitHub HMAC authentication, so once the name resolves to a stored webhook, the
//! webhook's own `WebhookDefinition$authentication` is what decides the call — a webhook
//! configured with any other `WebhookAuthenticationType` is refused with
//! `ValidationException` (HTTP 400) stating that requirement and the authentication the
//! store holds for it, and nothing its definition POINTS AT (the pipeline its
//! `targetPipeline` names) enters that answer, which is why the refusal never mentions
//! that reference however it resolves.
//!
//! The suites next door assert the ordering of the two refusals
//! (`third_party_webhook_refusal_order`) and the dangling-reference refusal of a webhook
//! that CAN carry the connection (`third_party_deregistration_target_pipeline`). What
//! this suite adds is the rule along the path a caller walks: a webhook whose
//! `targetPipeline` names no pipeline is accepted and published by `PutWebhook` and
//! `ListWebhooks`, is then refused on its authentication alone, is left stored by that
//! refusal, and is deleted — idempotently — after which the pair answers the model's
//! `WebhookNotFoundException` instead. The rows span the authentication types the model
//! declares besides the served one and whether the account holds a pipeline of the
//! targeted name.
//!
//! The harness and the definition builder are the ones `pipeline_lifecycle` and
//! `third_party_webhooks` own, so every assertion goes through the same router the
//! production emulator builds.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::WEBHOOK_AUTHENTICATION_TYPE_VALUES;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    webhook_authentication_unsupported_message, webhook_target_pipeline_absent_message,
    AUTHENTICATION_GITHUB_HMAC,
};

use crate::pipeline_lifecycle::{declaration, Emulator, ACCOUNT_ID};
use crate::third_party_webhooks::definition_targeting;

/// The pair of operations that connects a stored webhook to the external tool: one rule,
/// so every case drives both.
const OPERATIONS: [&str; 2] = [
    "RegisterWebhookWithThirdParty",
    "DeregisterWebhookWithThirdParty",
];

/// The errors either operation models, with the status the model defaults its client
/// errors to.
const REFUSAL_STATUS: u16 = 400;
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "WebhookNotFoundException";

/// The pipeline name every webhook here points at — one the account does not hold until a
/// row creates it, and a name that appears nowhere else, so a refusal mentioning the
/// reference is told apart from one that does not.
const TARGET_PIPELINE: &str = "pl-target-ref";

/// The webhooks `ListWebhooks` reports, as `(name, arn, definition)`.
async fn listed_webhooks(emulator: &Emulator) -> Vec<(String, String, Value)> {
    let listed = emulator.ok("ListWebhooks", &json!({})).await;
    listed["webhooks"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWebhooks reports a webhooks list, got {listed}"))
        .iter()
        .map(|item| {
            (
                item["definition"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                item["arn"].as_str().unwrap_or_default().to_string(),
                item["definition"].clone(),
            )
        })
        .collect()
}

/// RULE: for a stored webhook whose authentication is not the GitHub HMAC one, the
/// third-party pair refuses with `ValidationException` stating the GITHUB_HMAC
/// requirement and the authentication the store holds — the whole answer, carrying
/// nothing about the pipeline the definition points at — while the webhook itself is
/// accepted, published, left where it is by the refusal, and deleted only by
/// `DeleteWebhook`, after which the name resolves to nothing and the pair answers
/// `WebhookNotFoundException`.
///
/// The rows are every value of the model's `WebhookAuthenticationType` catalog
/// (`ops::types::WEBHOOK_AUTHENTICATION_TYPE_VALUES`) besides the served one — so `IP` is
/// driven as well as `UNAUTHENTICATED` — crossed with whether the account holds a
/// pipeline of the targeted name, which the rule does not consult.
#[tokio::test]
async fn an_unservable_authentication_is_the_whole_answer_along_the_webhook_s_life() {
    let unserved: Vec<&str> = WEBHOOK_AUTHENTICATION_TYPE_VALUES
        .iter()
        .copied()
        .filter(|authentication| *authentication != AUTHENTICATION_GITHUB_HMAC)
        .collect();
    assert!(
        !unserved.is_empty(),
        "the model's authentication catalog carries a type these operations do not serve, \
         got {WEBHOOK_AUTHENTICATION_TYPE_VALUES:?}"
    );

    for authentication in unserved {
        for (reference_case, hold_target) in [
            ("a target pipeline nothing holds", false),
            ("a target pipeline the account holds", true),
        ] {
            let case = format!("{authentication} with {reference_case}");
            let name = format!("wh-{}", authentication.to_lowercase());
            let emulator = Emulator::new();
            if hold_target {
                emulator
                    .ok(
                        "CreatePipeline",
                        &json!({"pipeline": declaration(TARGET_PIPELINE)}),
                    )
                    .await;
            }

            // The webhook is accepted and published, dangling reference and all: this
            // operation stores the definition without resolving what it points at.
            let definition = definition_targeting(&name, TARGET_PIPELINE, authentication);
            let put = emulator
                .ok("PutWebhook", &json!({"webhook": definition}))
                .await;
            let arn = format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:{name}");
            assert_eq!(
                (put["webhook"]["definition"].clone(), put["webhook"]["arn"].clone()),
                (definition.clone(), json!(arn)),
                "case {case:?}: PutWebhook reports the definition as stored under its ARN, \
                 got {put}"
            );
            let listed = listed_webhooks(&emulator).await;
            assert_eq!(
                listed,
                vec![(name.clone(), arn.clone(), definition.clone())],
                "case {case:?}: ListWebhooks publishes the stored webhook, got {listed:?}"
            );

            // The refusal: the authentication requirement, stated over the webhook the
            // request named and the authentication the store holds for it. The sentence is
            // written out here, so a reword of the handler's text fails this suite rather
            // than following it.
            let expected = format!(
                "Webhook {name} must be configured with GITHUB_HMAC authentication type. \
                 Type is {authentication}."
            );
            assert_eq!(
                webhook_authentication_unsupported_message(&name, authentication),
                expected,
                "case {case:?}: both operations state the one rule one way"
            );
            // Twice through each operation: the refusal is computed from state, so
            // answering it consumed nothing and the second call answers the same.
            for attempt in 1..=2 {
                for operation in OPERATIONS {
                    let (status, code, message) = emulator
                        .refusal(operation, &json!({"webhookName": &name}))
                        .await;
                    assert_eq!(
                        (status, code.as_str(), message.as_str()),
                        (REFUSAL_STATUS, VALIDATION, expected.as_str()),
                        "case {case:?} through {operation}, attempt {attempt}: expected \
                         {REFUSAL_STATUS} {VALIDATION} {expected:?}, got {status} {code} \
                         {message:?}"
                    );
                    assert!(
                        !message.contains(TARGET_PIPELINE),
                        "case {case:?} through {operation}: the answer is the authentication \
                         requirement, so it says nothing of the reference \
                         {TARGET_PIPELINE:?}, got {message:?}"
                    );
                }
            }

            // Refusing changed nothing a later caller sees: the webhook is still stored,
            // definition and ARN unchanged.
            let listed = listed_webhooks(&emulator).await;
            assert_eq!(
                listed,
                vec![(name.clone(), arn.clone(), definition.clone())],
                "case {case:?}: a refused call left the webhook as it was, got {listed:?}"
            );

            // Deleting the webhook is what removes it, and deleting it again is served
            // the same way; after that the name resolves to nothing, which is the OTHER
            // error either operation models.
            for attempt in 1..=2 {
                let (status, answer) = emulator
                    .call("DeleteWebhook", &json!({"name": &name}))
                    .await;
                assert_eq!(
                    (status, answer.get("__type")),
                    (200, None),
                    "case {case:?}: DeleteWebhook attempt {attempt} is served with the empty \
                     success, got {status} with {answer}"
                );
            }
            let listed = listed_webhooks(&emulator).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: the deleted webhook is gone from the listing, got {listed:?}"
            );
            for operation in OPERATIONS {
                let (status, code, _) = emulator
                    .refusal(operation, &json!({"webhookName": &name}))
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (REFUSAL_STATUS, NOT_FOUND),
                    "case {case:?} through {operation}: a name no stored webhook carries is \
                     refused as not found, got {status} {code}"
                );
            }
        }
    }
}

/// CONTROL for the same rule: the authentication refusal is not the pair's answer to
/// everything it cannot serve. A webhook that CAN carry the connection — the GitHub HMAC
/// one — pointing at the same pipeline nothing holds is refused on that reference instead,
/// and is served with the empty 200 once a pipeline of that name is held. So the sentence
/// each refusal carries reports the condition it was reached on.
#[tokio::test]
async fn the_served_authentication_is_refused_on_its_reference_instead() {
    let emulator = Emulator::new();
    let name = "wh-github-hmac";
    let definition = definition_targeting(name, TARGET_PIPELINE, AUTHENTICATION_GITHUB_HMAC);
    emulator
        .ok("PutWebhook", &json!({"webhook": definition}))
        .await;

    let expected =
        format!("Webhook {name} references a pipeline {TARGET_PIPELINE} which does not exist");
    assert_eq!(
        webhook_target_pipeline_absent_message(name, TARGET_PIPELINE),
        expected,
        "both operations state the dangling-reference rule one way"
    );
    for operation in OPERATIONS {
        let (status, code, message) = emulator
            .refusal(operation, &json!({"webhookName": name}))
            .await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (REFUSAL_STATUS, VALIDATION, expected.as_str()),
            "through {operation}: expected {REFUSAL_STATUS} {VALIDATION} {expected:?}, got \
             {status} {code} {message:?}"
        );
    }

    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": declaration(TARGET_PIPELINE)}),
        )
        .await;
    for operation in OPERATIONS {
        let (status, answer) = emulator
            .call(operation, &json!({"webhookName": name}))
            .await;
        assert_eq!(
            (status, answer.get("__type")),
            (200, None),
            "through {operation}: a webhook that carries the connection is served with the \
             empty success, got {status} with {answer}"
        );
    }
}
