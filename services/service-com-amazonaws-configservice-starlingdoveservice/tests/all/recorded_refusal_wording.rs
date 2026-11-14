// SPDX-License-Identifier: CC0-1.0

//! The wording of three refusals a client branches on, end to end over the wire.
//!
//! An HTTP status and an error code are not the whole contract of a refusal: the
//! `message` is what a client surfaces, logs and asserts on, so each refusal class has
//! ONE sentence and it is the sentence AWS words for that class. The rules pinned here:
//!
//! 1. a configuration-snapshot delivery asked for while the customer managed recorder is
//!    turned off is refused `NoRunningConfigurationRecorderException` worded as the state
//!    the recorder is in and the change the caller has to make — the same sentence
//!    whichever channel the delivery named;
//! 2. a delivery of evaluations carrying a `ResultToken` Config did not mint for the
//!    rule's current invocation is refused `InvalidResultTokenException` worded the same
//!    way whatever the token was, quoting NOTHING of it: the token is opaque to its
//!    holder;
//! 3. `PutExternalEvaluation` tells its two refusals apart — a rule name no rule answers
//!    to is `NoSuchConfigRuleException`, whose message quotes the name the request sent,
//!    while a rule that EXISTS and is not a Config Process Check rule is
//!    `InvalidParameterValueException`, worded as the condition it refuses (the
//!    `SourceIdentifier` the operation requires) and quoting nothing of the request —
//!    because the remediation differs: create the rule, or point the call at a process
//!    check.
//!
//! Each case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{
    managed_rule, recorder_request, with_recorder_and_channel, Emulator, ROLE_ARN,
};
use crate::external_evaluation::put_process_check_rule;
use crate::put_evaluations::{custom_lambda_rule, result_token};

/// The resource type an external evaluation names
/// (`ExternalEvaluation$ComplianceResourceType`: "Config accepts `AWS::::Account`
/// resource type").
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// Call `operation`, require a 400 carrying the error code `code`, and answer the message
/// the service worded the refusal with.
async fn refusal(case: &str, emulator: &Emulator, operation: &str, body: &Value, code: &str) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer HTTP 400, got {status} with {parsed}"
    );
    let actual = parsed["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
        .unwrap_or("<none>");
    assert_eq!(
        actual, code,
        "case {case:?}: expected the error code {code}, got {actual:?} in {parsed}"
    );
    parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {parsed}"))
        .to_string()
}

#[tokio::test]
async fn a_snapshot_delivery_with_the_recorder_turned_off_is_worded_as_the_recorders_state() {
    // Rows differ along the channel the delivery named and the recorder's history: a
    // channel put with its own name, the default name Config assigns, and a recorder
    // that has been started and stopped again. The sentence is the same for all of them:
    // it is about the recorder's state, and carries nothing of the request.
    let expected = "Configuration recorder is turned off. Turn on configuration recorder.";

    for (case, channel_name, start_then_stop) in [
        ("a channel put with its own name", "sim-probe-channel", false),
        ("the default channel name", "default", false),
        ("a recorder started and stopped again", "default", true),
    ] {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({ "roleARN": ROLE_ARN })),
            )
            .await;
        emulator
            .ok(
                "PutDeliveryChannel",
                &json!({
                    "DeliveryChannel": {
                        "name": channel_name,
                        "s3BucketName": "config-bucket",
                    }
                }),
            )
            .await;
        if start_then_stop {
            emulator
                .ok(
                    "StartConfigurationRecorder",
                    &json!({ "ConfigurationRecorderName": "default" }),
                )
                .await;
            emulator
                .ok(
                    "StopConfigurationRecorder",
                    &json!({ "ConfigurationRecorderName": "default" }),
                )
                .await;
        }

        let message = refusal(
            case,
            &emulator,
            "DeliverConfigSnapshot",
            &json!({ "deliveryChannelName": channel_name }),
            "NoRunningConfigurationRecorderException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to be worded {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_result_token_config_did_not_mint_is_worded_without_quoting_the_token() {
    let name = "token-rule";
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &custom_lambda_rule(name)).await;
    let minted = result_token(&emulator, name);

    let expected = "Result Token provided is invalid";

    // Rows differ along WHAT the token is: a forged value, one that is only whitespace,
    // one that decodes to nothing Config issued, and a token Config minted and then
    // superseded by invoking the rule again. None of them appears in the sentence.
    emulator
        .ok(
            "StartConfigRulesEvaluation",
            &json!({ "ConfigRuleNames": [name] }),
        )
        .await;
    let superseded = minted;

    for (case, token) in [
        ("a forged token", "not-a-token-config-minted".to_string()),
        ("a token that is only whitespace", " ".to_string()),
        (
            "a token that decodes to nothing Config issued",
            aws_smithy_types::base64::encode("0:"),
        ),
        ("a superseded token", superseded),
    ] {
        let message = refusal(
            case,
            &emulator,
            "PutEvaluations",
            &json!({ "ResultToken": token, "Evaluations": [] }),
            "InvalidResultTokenException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to be worded {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(token.trim()) || token.trim().is_empty(),
            "case {case:?}: the refusal must quote nothing of the token, got {message:?}"
        );
    }

    // The token Config minted for the rule's CURRENT invocation is accepted, so the
    // refusals above are about the token and not about the delivery.
    let current = result_token(&emulator, name);
    let answered = emulator
        .ok(
            "PutEvaluations",
            &json!({ "ResultToken": current, "Evaluations": [] }),
        )
        .await;
    assert_eq!(
        answered["FailedEvaluations"].as_array().map(Vec::len),
        Some(0),
        "the rule's current ResultToken is accepted, got {answered}"
    );
}

#[tokio::test]
async fn an_external_evaluation_tells_a_missing_rule_from_a_rule_of_the_wrong_kind() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    // A process check rule the account created while Config still offered that managed
    // rule: `PutConfigRule` no longer accepts the identifier (pinned in
    // `withdrawn_managed_rule`), and this operation still applies to the rules already
    // created under it.
    put_process_check_rule(&emulator, "process-check").await;
    emulator.ok("PutConfigRule", &managed_rule("managed")).await;
    emulator
        .ok("PutConfigRule", &custom_lambda_rule("lambda-rule"))
        .await;
    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "policy-rule",
                    "Source": {
                        "Owner": "CUSTOM_POLICY",
                        "CustomPolicyDetails": {
                            "PolicyRuntime": "guard-2.x.x",
                            "PolicyText": "rule check { true }",
                        },
                    },
                }
            }),
        )
        .await;

    let delivery = |rule: &str| {
        json!({
            "ConfigRuleName": rule,
            "ExternalEvaluation": {
                "ComplianceResourceType": ACCOUNT_RESOURCE_TYPE,
                "ComplianceResourceId": emulator_server::emulator_account_id(),
                "ComplianceType": "COMPLIANT",
                "OrderingTimestamp": 1_700_000_000,
            },
        })
    };

    // A rule name no rule answers to: the caller has to create the rule, which is what
    // `NoSuchConfigRuleException` tells it. Rows differ along how the name misses.
    for (case, rule) in [
        ("a name nothing was created under", "never-created"),
        ("a name differing only in case", "PROCESS-CHECK"),
        ("a prefix of an existing name", "process"),
    ] {
        let message = refusal(
            case,
            &emulator,
            "PutExternalEvaluation",
            &delivery(rule),
            "NoSuchConfigRuleException",
        )
        .await;
        assert!(
            message.contains(rule),
            "case {case:?}: expected the refusal to quote the rule name {rule:?}, got {message:?}"
        );
    }

    // A rule that EXISTS and is not a process check: the caller has to point the call at
    // a process check, which is a bad parameter value rather than a missing rule (RECORDED
    // behaviour: "the rule exists, so the refusal is about its type, not
    // NoSuchConfigRuleException, which is reserved for a rule name that does not exist at
    // all"). Because both refusals share one error code, the sentence is the only thing
    // that tells the two apart, so it NAMES the condition — the `SourceIdentifier` this
    // operation requires — rather than reading as the generic invalid-parameter text, and
    // it is the same for every kind of rule that is not a process check.
    let expected =
        "This API can only be applied to rules with sourceIdentifier as AWS_CONFIG_PROCESS_CHECK.";
    for (case, rule) in [
        ("a managed rule that is not the process check one", "managed"),
        ("a Custom Lambda rule", "lambda-rule"),
        ("a Custom Policy rule", "policy-rule"),
    ] {
        let message = refusal(
            case,
            &emulator,
            "PutExternalEvaluation",
            &delivery(rule),
            "InvalidParameterValueException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to be worded {expected:?}, got {message:?}"
        );
    }

    // The process check rule itself takes the same delivery, so the refusals above are
    // about the rule the call named.
    emulator
        .ok("PutExternalEvaluation", &delivery("process-check"))
        .await;
}
