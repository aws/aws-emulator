// SPDX-License-Identifier: CC0-1.0

//! Writing a Config rule as an instance of a managed rule Config no longer offers, end
//! to end over the wire.
//!
//! The rule pinned here: a Config rule OWNED by Amazon Web Services is an instance of one
//! of Config's own managed rules, named by `Source$SourceIdentifier` ("For Config Managed
//! rules, a predefined identifier from a list"); an identifier WITHDRAWN from that list
//! names no managed rule Config can evaluate, so `PutConfigRule` refuses the write 400
//! `InvalidParameterValueException` stating that sourceIdentifier as invalid, and the
//! account is left holding exactly the rules it held before. The owner is what makes an
//! identifier a managed rule's at all — a Config Custom Lambda rule's identifier is the
//! ARN of the function that evaluates it, and a Custom Policy rule carries its policy
//! instead — so a rule of either kind is written as before.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// The `Source$SourceIdentifier`s Config has withdrawn from *List of Config Managed
/// Rules*, the list that member points at: the managed rule a Config process check IS,
/// which `PutExternalEvaluation` still names as the kind of rule it applies to while
/// `PutConfigRule` no longer accepts it (RECORDED behaviour).
const WITHDRAWN: [&str; 1] = ["AWS_CONFIG_PROCESS_CHECK"];

/// A managed rule Config DOES offer, from the same list — the control every case is
/// contrasted with.
const OFFERED: &str = "S3_BUCKET_VERSIONING_ENABLED";

/// Another managed rule Config offers, from the same list, which this emulator does not
/// itself evaluate: an identifier Config offers is written whether or not the emulator
/// has an implementation for it, because the list Config offers holds hundreds of
/// entries and only the ones it has WITHDRAWN are refused.
const OFFERED_AND_NOT_EVALUATED: &str = "CLOUD_TRAIL_ENABLED";

/// The owners a rule's `Source` can carry, from
/// `smithy_query(query="shape", shape="Owner")`: `CUSTOM_LAMBDA`, `AWS`,
/// `CUSTOM_POLICY`.
const OWNER_AWS: &str = "AWS";
/// See [`OWNER_AWS`].
const OWNER_CUSTOM_LAMBDA: &str = "CUSTOM_LAMBDA";
/// See [`OWNER_AWS`].
const OWNER_CUSTOM_POLICY: &str = "CUSTOM_POLICY";

/// How Config states an identifier that names no managed rule it offers: the identifier
/// the request carried is the one part of the sentence taken from the request.
fn invalid_identifier(identifier: &str) -> String {
    format!(
        "The sourceIdentifier {identifier} is invalid. Please refer to the documentation \
         for a list of valid sourceIdentifiers that can be used when AWS is the Owner."
    )
}

/// The `PutConfigRule` body naming `source` for the rule `name`, with `extras` merged
/// into the rule.
fn rule_with(name: &str, source: Value, extras: Value) -> Value {
    let mut rule = json!({ "ConfigRuleName": name, "Source": source });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            rule[member] = value.clone();
        }
    }
    json!({ "ConfigRule": rule })
}

/// The rule names `DescribeConfigRules` reports for this account.
async fn rule_names(emulator: &Emulator) -> Vec<String> {
    let described = emulator.ok("DescribeConfigRules", &json!({})).await;
    described["ConfigRules"]
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|rule| {
                    rule["ConfigRuleName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn an_aws_owned_rule_naming_a_withdrawn_managed_rule_is_refused_and_stored_nowhere() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;

    // Rows differ along what the rule carries beside the withdrawn identifier and how
    // the identifier is spelled on the wire: the bare source, the source with the other
    // members a rule can carry, and the identifier with the surrounding whitespace a
    // client is free to send. Every row names the identifier from the catalogue above,
    // so a second withdrawn identifier is covered by the same rows.
    for identifier in WITHDRAWN {
        for (case, extras, sent) in [
            ("the bare source", json!({}), identifier.to_string()),
            (
                "a rule carrying every member it can",
                json!({
                    "Description": "a process a reviewer signs off",
                    "Scope": { "ComplianceResourceTypes": ["AWS::::Account"] },
                    "MaximumExecutionFrequency": "TwentyFour_Hours",
                    "EvaluationModes": [{ "Mode": "DETECTIVE" }],
                }),
                identifier.to_string(),
            ),
            (
                "the identifier with surrounding whitespace",
                json!({}),
                format!("  {identifier} "),
            ),
        ] {
            let before = rule_names(&emulator).await;
            let request = rule_with(
                "withdrawn-rule",
                json!({ "Owner": OWNER_AWS, "SourceIdentifier": sent }),
                extras,
            );
            let (status, parsed) = emulator.call("PutConfigRule", &request).await;
            let code = parsed["__type"]
                .as_str()
                .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
                .unwrap_or("<none>");
            assert_eq!(
                (status, code),
                (400, "InvalidParameterValueException"),
                "case {case:?} ({identifier}): expected PutConfigRule to be refused 400 \
                 InvalidParameterValueException, got {status} {parsed}"
            );
            // The sentence states the identifier the REQUEST carried, which is what tells
            // the caller which member to change.
            let expected = invalid_identifier(identifier);
            assert_eq!(
                parsed["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?} ({identifier}): expected the refusal to be worded \
                 {expected:?}, got {parsed}"
            );
            // Nothing was written: the account holds exactly the rules it held before.
            let after = rule_names(&emulator).await;
            assert_eq!(
                after, before,
                "case {case:?} ({identifier}): a refused PutConfigRule must leave the \
                 account holding {before:?}, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_rule_already_created_under_a_withdrawn_identifier_is_not_rewritten_by_the_refusal() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule("review")).await;

    // An UPDATE is the same write as a create, so pointing a rule the account already
    // holds at a withdrawn identifier is refused the same way — and the rule it holds is
    // left as it was, still an instance of the managed rule Config does offer.
    for identifier in WITHDRAWN {
        let request = rule_with(
            "review",
            json!({ "Owner": OWNER_AWS, "SourceIdentifier": identifier }),
            json!({}),
        );
        let (status, parsed) = emulator.call("PutConfigRule", &request).await;
        assert_eq!(
            status, 400,
            "case {identifier}: expected the update to be refused 400, got {status} {parsed}"
        );

        let described = emulator.ok("DescribeConfigRules", &json!({})).await;
        let source = &described["ConfigRules"][0]["Source"];
        assert_eq!(
            source["SourceIdentifier"].as_str(),
            Some(OFFERED),
            "case {identifier}: the rule must still be an instance of {OFFERED:?}, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn a_rule_whose_owner_does_not_name_a_managed_rule_is_written_as_before() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;

    // The withdrawal is about the MANAGED rule an Amazon Web Services-owned rule is an
    // instance of, so the rules whose identifier is not a managed rule's at all are
    // written as before: a Custom Lambda rule identified by its function's ARN, a Custom
    // Policy rule carrying its policy, and an Amazon Web Services-owned rule naming a
    // managed rule Config does offer.
    let lambda_arn = format!(
        "arn:aws:lambda:{}:000000000000:function:evaluate",
        emulator.region
    );
    for (case, name, source) in [
        (
            "a managed rule Config offers",
            "offered",
            json!({ "Owner": OWNER_AWS, "SourceIdentifier": OFFERED }),
        ),
        (
            "a managed rule Config offers that this emulator does not evaluate",
            "offered-unevaluated",
            json!({ "Owner": OWNER_AWS, "SourceIdentifier": OFFERED_AND_NOT_EVALUATED }),
        ),
        (
            "a Custom Lambda rule",
            "lambda-rule",
            json!({ "Owner": OWNER_CUSTOM_LAMBDA, "SourceIdentifier": lambda_arn }),
        ),
        (
            "a Custom Policy rule",
            "policy-rule",
            json!({
                "Owner": OWNER_CUSTOM_POLICY,
                "CustomPolicyDetails": {
                    "PolicyRuntime": "guard-2.x.x",
                    "PolicyText": "rule check { true }",
                },
            }),
        ),
    ] {
        emulator
            .ok("PutConfigRule", &rule_with(name, source, json!({})))
            .await;
        let held = rule_names(&emulator).await;
        assert!(
            held.iter().any(|rule| rule == name),
            "case {case:?}: expected the account to hold the rule {name:?}, got {held:?}"
        );
    }
}
