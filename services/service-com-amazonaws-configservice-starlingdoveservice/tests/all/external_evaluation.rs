// SPDX-License-Identifier: CC0-1.0

//! What `PutExternalEvaluation` persists, end to end over the wire.
//!
//! The rule under test: a verdict delivered for a Config Process Check rule becomes
//! that rule's evaluation result for the evaluated account — one result per evaluated
//! resource and evaluation mode, and the only account one store stands for is its own,
//! so delivering again REPLACES the rule's verdict rather than adding one — reported
//! back by `GetComplianceDetailsByConfigRule` with the account resource type, the
//! detective evaluation mode, the `OrderingTimestamp` the caller delivered and the
//! annotation it carried, and rolled up into the rule's own compliance by
//! `DescribeComplianceByConfigRule`. The write refuses a rule this account does not
//! hold (`NoSuchConfigRuleException`), a rule that EXISTS but is not a process check
//! (`InvalidParameterValueException` — the refusal is about the rule's kind, not its
//! existence), and a resource
//! type, resource id, compliance type or ordering timestamp the member does not accept
//! (`InvalidParameterValueException`), persisting nothing when it refuses — and the
//! modelled lengths are refused at the request boundary before it runs at all.
//!
//! Every case drives the service the way a client does: an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    point_at_process_check, service_state,
};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// The `SourceIdentifier` of the Config Managed rule a process check IS. `Source`'s
/// `SourceIdentifier` is "For Config Managed rules, a predefined identifier from a
/// list" (`smithy_query(query="shape", shape="Source")`), and the model enumerates
/// none of that list, so the identifier is named here from *List of Config Managed
/// Rules*, the list that member points at.
const PROCESS_CHECK: &str = "AWS_CONFIG_PROCESS_CHECK";

/// The only `ComplianceResourceType` an external evaluation may name, from
/// `smithy_query(query="shape", shape="ExternalEvaluation")`
/// (`ComplianceResourceType`: "Config accepts `AWS::::Account` resource type").
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// Every `ComplianceType` an external evaluation may carry, from the same query
/// (`ComplianceType`: "The valid values are `COMPLIANT, NON_COMPLIANT,` and
/// `NOT_APPLICABLE`"). The published enum
/// (`smithy_query(query="shape", shape="ComplianceType")`) also carries
/// `INSUFFICIENT_DATA`, which this member does not accept.
const ACCEPTED_COMPLIANCE_TYPES: [&str; 3] = ["COMPLIANT", "NON_COMPLIANT", "NOT_APPLICABLE"];

/// The `ComplianceType` Config derives for a rule it could not evaluate, which is what
/// a process check with no delivered verdict rolls up to.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The `EvaluationMode` a delivered verdict is recorded in: a process check reports on
/// a process that has already run, which is what
/// `smithy_query(query="shape", shape="EvaluationMode")`'s `DETECTIVE` names.
const DETECTIVE: &str = "DETECTIVE";

/// The longest `Annotation` the model allows (`ExternalEvaluation$Annotation` targets
/// `StringWithCharLimit256`, `length 1..256`).
const ANNOTATION_MAX: usize = 256;

/// The process check rule every case delivers against.
const RULE: &str = "quarterly-access-review";

/// A managed rule that is NOT a process check, named from *List of Config Managed
/// Rules* the same way [`PROCESS_CHECK`] is.
const NOT_A_PROCESS_CHECK: &str = "not-a-process-check";

/// Put a Config Process Check rule named `name` into this account, the way an account
/// holds one it created while Config still offered that managed rule.
///
/// Config has since WITHDRAWN `AWS_CONFIG_PROCESS_CHECK` from the list of managed rules
/// `PutConfigRule` accepts (RECORDED behaviour, pinned in `withdrawn_managed_rule`), so
/// a process check rule can no longer be created over the wire — while
/// `PutExternalEvaluation` still applies to the ones already created, which is the
/// population every case here is about. The rule is therefore created through
/// `PutConfigRule` as an instance of a managed rule Config does offer, scoped the way a
/// process check is (it evaluates the account), and then pointed at the process check
/// managed rule through the crate's own seam
/// (`state::point_at_process_check`) — the way `compliance_details_by_config_rule` seeds
/// the inventory no operation of this API mints.
///
/// `pub(crate)` because every suite about what happens to a rule's DELIVERED verdicts
/// needs exactly this rule kind to seed one from outside Config.
pub(crate) async fn put_process_check_rule(emulator: &Emulator, name: &str) {
    let mut request = managed_rule(name);
    request["ConfigRule"]["Scope"] =
        json!({ "ComplianceResourceTypes": [ACCOUNT_RESOURCE_TYPE] });
    emulator.ok("PutConfigRule", &request).await;
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    let rule = store
        .config_rules
        .get_mut(name)
        .expect("the rule was created through PutConfigRule");
    point_at_process_check(&mut rule.source);
}

/// The account this emulator serves, which is the account a process check evaluates.
fn this_account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// An emulator holding the process check rule [`RULE`], ready for verdicts.
async fn emulator_with_process_check() -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    put_process_check_rule(&emulator, RULE).await;
    emulator
}

/// The `PutExternalEvaluation` body delivering `compliance_type` for `resource_id` at
/// `ordering`, carrying `annotation` when there is one.
fn delivery(
    rule: &str,
    resource_type: &str,
    resource_id: &str,
    compliance_type: &str,
    ordering: f64,
    annotation: Option<&str>,
) -> Value {
    let mut evaluation = json!({
        "ComplianceResourceType": resource_type,
        "ComplianceResourceId": resource_id,
        "ComplianceType": compliance_type,
        "OrderingTimestamp": ordering,
    });
    if let Some(annotation) = annotation {
        evaluation["Annotation"] = json!(annotation);
    }
    json!({ "ConfigRuleName": rule, "ExternalEvaluation": evaluation })
}

/// Every evaluation result `rule` holds, as `GetComplianceDetailsByConfigRule` reports
/// them.
async fn recorded(case: &str, emulator: &Emulator, rule: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": rule, "Limit": 100 }),
        )
        .await;
    body["EvaluationResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EvaluationResults list, got {body}"))
        .clone()
}

/// The one evaluation result `rule` holds.
async fn only_recorded(case: &str, emulator: &Emulator, rule: &str) -> Value {
    let results = recorded(case, emulator, rule).await;
    assert_eq!(
        results.len(),
        1,
        "case {case:?}: expected exactly one recorded result, got {results:#?}"
    );
    results[0].clone()
}

/// `value` as the epoch seconds a modelled timestamp is carried as on the wire.
fn seconds(case: &str, member: &str, value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be a timestamp, got {value}"))
}

/// Now, as the epoch seconds the wire carries a modelled timestamp as.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// The `(ComplianceType, ComplianceContributorCount.CappedCount)` that
/// `DescribeComplianceByConfigRule` rolls `rule` up to, where it reports a count.
async fn rolled_up(case: &str, emulator: &Emulator, rule: &str) -> (String, Option<i64>) {
    let body = emulator
        .ok(
            "DescribeComplianceByConfigRule",
            &json!({ "ConfigRuleNames": [rule] }),
        )
        .await;
    let entries = body["ComplianceByConfigRules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ComplianceByConfigRules, got {body}"));
    let entry = entries
        .iter()
        .find(|entry| entry["ConfigRuleName"] == json!(rule))
        .unwrap_or_else(|| panic!("case {case:?}: expected an entry for {rule:?}, got {body}"));
    let compliance_type = entry["Compliance"]["ComplianceType"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ComplianceType, got {entry}"))
        .to_string();
    let contributors = entry["Compliance"]["ComplianceContributorCount"]["CappedCount"].as_i64();
    (compliance_type, contributors)
}

#[tokio::test]
async fn a_delivered_verdict_is_persisted_and_read_back_as_the_rules_result() {
    // Rows differ along what the delivered evaluation carries: the compliance type
    // (every value the member accepts), whether it carries an annotation and how long
    // that annotation is (one character, a sentence, the modelled 256-character
    // maximum) and the ordering timestamp (the epoch, a whole second, a sub-second
    // fraction, an instant well past now).
    let long_annotation = "r".repeat(ANNOTATION_MAX);
    let cases: [(&str, &str, Option<&str>, f64); 7] = [
        (
            "a compliant verdict with no annotation",
            ACCEPTED_COMPLIANCE_TYPES[0],
            None,
            1_700_000_000.0,
        ),
        (
            "a noncompliant verdict with a sentence of annotation",
            ACCEPTED_COMPLIANCE_TYPES[1],
            Some("the quarterly access review did not happen"),
            1_700_003_600.0,
        ),
        (
            "a not-applicable verdict",
            ACCEPTED_COMPLIANCE_TYPES[2],
            None,
            1_700_000_001.0,
        ),
        (
            "an evaluation ordered at the epoch",
            ACCEPTED_COMPLIANCE_TYPES[0],
            Some("recorded against the epoch"),
            0.0,
        ),
        (
            "an evaluation ordered at a sub-second instant",
            ACCEPTED_COMPLIANCE_TYPES[1],
            None,
            1_700_000_000.25,
        ),
        (
            "an evaluation ordered well past now",
            ACCEPTED_COMPLIANCE_TYPES[2],
            Some("a"),
            4_000_000_000.0,
        ),
        (
            "an annotation at the modelled maximum length",
            ACCEPTED_COMPLIANCE_TYPES[1],
            Some(long_annotation.as_str()),
            1_700_000_123.0,
        ),
    ];

    for (case, compliance_type, annotation, ordering) in cases {
        let emulator = emulator_with_process_check().await;
        let account = this_account();

        let before = now_seconds();
        emulator
            .ok(
                "PutExternalEvaluation",
                &delivery(
                    RULE,
                    ACCOUNT_RESOURCE_TYPE,
                    &account,
                    compliance_type,
                    ordering,
                    annotation,
                ),
            )
            .await;
        let after = now_seconds();

        let result = only_recorded(case, &emulator, RULE).await;
        let qualifier = &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
        assert_eq!(
            (
                qualifier["ConfigRuleName"].as_str(),
                qualifier["ResourceType"].as_str(),
                qualifier["ResourceId"].as_str(),
                qualifier["EvaluationMode"].as_str(),
                result["ComplianceType"].as_str(),
                result["Annotation"].as_str(),
            ),
            (
                Some(RULE),
                Some(ACCOUNT_RESOURCE_TYPE),
                Some(account.as_str()),
                Some(DETECTIVE),
                Some(compliance_type),
                annotation,
            ),
            "case {case:?}: expected the delivered verdict as recorded, got {result}"
        );

        // The caller's `OrderingTimestamp` is "the time when the compliance was
        // recorded", i.e. when the evaluation itself happened, so it is stored as
        // delivered and is also when the rule was invoked.
        let reported_ordering = seconds(
            case,
            "OrderingTimestamp",
            &result["EvaluationResultIdentifier"]["OrderingTimestamp"],
        );
        let invoked = seconds(case, "ConfigRuleInvokedTime", &result["ConfigRuleInvokedTime"]);
        assert_eq!(
            (reported_ordering, invoked),
            (ordering, ordering),
            "case {case:?}: expected the delivered ordering timestamp back, got {result}"
        );

        // Config recorded the result when the call arrived, so the recorded time is
        // this emulator's clock at that moment and not a value from the request.
        let recorded_time = seconds(case, "ResultRecordedTime", &result["ResultRecordedTime"]);
        assert!(
            (before..=after).contains(&recorded_time),
            "case {case:?}: expected a ResultRecordedTime within [{before}, {after}], got \
             {recorded_time}"
        );

        // The result came from a delivery, not from a resource evaluation run, and the
        // read path hands back no result token.
        assert!(
            result["EvaluationResultIdentifier"]["ResourceEvaluationId"].is_null()
                && result["ResultToken"].is_null(),
            "case {case:?}: a delivered verdict carries no ResourceEvaluationId or \
             ResultToken, got {result}"
        );
    }
}

#[tokio::test]
async fn delivering_again_replaces_the_rules_verdict_rather_than_adding_one() {
    // Rows differ along the sequence of verdicts delivered against one rule: one
    // verdict, a revision, two revisions, a revision back to a value already
    // delivered, and every accepted value in turn. The account evaluated is the one
    // this store stands for in every row, which is the only account a process check
    // here can evaluate, so each delivery lands on the SAME key.
    let cases: [(&str, &[&str]); 5] = [
        ("one verdict", &[ACCEPTED_COMPLIANCE_TYPES[0]]),
        (
            "revised once",
            &[ACCEPTED_COMPLIANCE_TYPES[0], ACCEPTED_COMPLIANCE_TYPES[1]],
        ),
        (
            "revised twice",
            &[
                ACCEPTED_COMPLIANCE_TYPES[1],
                ACCEPTED_COMPLIANCE_TYPES[2],
                ACCEPTED_COMPLIANCE_TYPES[0],
            ],
        ),
        (
            "revised back to the verdict already delivered",
            &[
                ACCEPTED_COMPLIANCE_TYPES[2],
                ACCEPTED_COMPLIANCE_TYPES[0],
                ACCEPTED_COMPLIANCE_TYPES[2],
            ],
        ),
        (
            "every accepted value in turn",
            &[
                ACCEPTED_COMPLIANCE_TYPES[0],
                ACCEPTED_COMPLIANCE_TYPES[1],
                ACCEPTED_COMPLIANCE_TYPES[2],
                ACCEPTED_COMPLIANCE_TYPES[0],
            ],
        ),
    ];

    for (case, verdicts) in cases {
        let emulator = emulator_with_process_check().await;
        let account = this_account();

        for (index, compliance_type) in verdicts.iter().enumerate() {
            emulator
                .ok(
                    "PutExternalEvaluation",
                    &delivery(
                        RULE,
                        ACCOUNT_RESOURCE_TYPE,
                        &account,
                        compliance_type,
                        1_700_000_000.0 + index as f64,
                        Some(&format!("delivery {index}")),
                    ),
                )
                .await;
        }

        // However many verdicts were delivered, the rule holds exactly one result,
        // carrying the last verdict and the last delivery's ordering timestamp and
        // annotation.
        let result = only_recorded(case, &emulator, RULE).await;
        let last = verdicts.len() - 1;
        assert_eq!(
            (
                result["ComplianceType"].as_str(),
                result["Annotation"].as_str(),
                seconds(
                    case,
                    "OrderingTimestamp",
                    &result["EvaluationResultIdentifier"]["OrderingTimestamp"],
                ),
            ),
            (
                Some(verdicts[last]),
                Some(format!("delivery {last}").as_str()),
                1_700_000_000.0 + last as f64,
            ),
            "case {case:?}: expected the last delivered verdict to be the one held, got \
             {result}"
        );
    }
}

#[tokio::test]
async fn each_process_check_rule_holds_its_own_verdict() {
    // One account, several rules: a delivery is keyed by the rule it names, so
    // delivering against one rule leaves the others' verdicts alone. Rows differ along
    // how many process check rules the account holds and which of them is revised.
    let rules = ["review-a", "review-b", "review-c"];
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for rule in rules {
        put_process_check_rule(&emulator, rule).await;
    }
    let account = this_account();

    for (index, rule) in rules.iter().enumerate() {
        emulator
            .ok(
                "PutExternalEvaluation",
                &delivery(
                    rule,
                    ACCOUNT_RESOURCE_TYPE,
                    &account,
                    ACCEPTED_COMPLIANCE_TYPES[index],
                    1_700_000_000.0 + index as f64,
                    None,
                ),
            )
            .await;
    }

    // Revising one rule's verdict must not touch another's.
    emulator
        .ok(
            "PutExternalEvaluation",
            &delivery(
                rules[1],
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[2],
                1_700_000_100.0,
                None,
            ),
        )
        .await;

    let expected = [
        ACCEPTED_COMPLIANCE_TYPES[0],
        ACCEPTED_COMPLIANCE_TYPES[2],
        ACCEPTED_COMPLIANCE_TYPES[2],
    ];
    for (rule, compliance_type) in rules.iter().zip(expected) {
        let result = only_recorded(rule, &emulator, rule).await;
        assert_eq!(
            result["ComplianceType"].as_str(),
            Some(compliance_type),
            "case {rule:?}: expected the verdict delivered for this rule, got {result}"
        );
    }
}

#[tokio::test]
async fn the_rule_rolls_up_the_verdict_delivered_against_it() {
    // Rows differ along the verdict the rule's one result carries, which is what the
    // rule's compliance rolls up from: nothing delivered at all, and each accepted
    // value, including the not-applicable one that places the rule neither way.
    let cases: [(&str, Option<&str>, &str, Option<i64>); 4] = [
        ("nothing delivered", None, INSUFFICIENT_DATA, None),
        (
            "a compliant account",
            Some(ACCEPTED_COMPLIANCE_TYPES[0]),
            ACCEPTED_COMPLIANCE_TYPES[0],
            None,
        ),
        (
            "a noncompliant account",
            Some(ACCEPTED_COMPLIANCE_TYPES[1]),
            ACCEPTED_COMPLIANCE_TYPES[1],
            Some(1),
        ),
        (
            "a not-applicable account",
            Some(ACCEPTED_COMPLIANCE_TYPES[2]),
            INSUFFICIENT_DATA,
            None,
        ),
    ];

    for (case, verdict, expected_type, expected_contributors) in cases {
        let emulator = emulator_with_process_check().await;
        if let Some(compliance_type) = verdict {
            emulator
                .ok(
                    "PutExternalEvaluation",
                    &delivery(
                        RULE,
                        ACCOUNT_RESOURCE_TYPE,
                        &this_account(),
                        compliance_type,
                        1_700_000_000.0,
                        None,
                    ),
                )
                .await;
        }

        let rolled = rolled_up(case, &emulator, RULE).await;
        assert_eq!(
            (rolled.0.as_str(), rolled.1),
            (expected_type, expected_contributors),
            "case {case:?}: expected the rule to roll up to {expected_type} with \
             {expected_contributors:?} contributors, got {rolled:?}"
        );
    }
}

#[tokio::test]
async fn the_write_refuses_a_rule_and_an_evaluation_it_cannot_record() {
    let emulator = emulator_with_process_check().await;
    // Rules that exist but are not process checks: a managed rule that is not the
    // process check managed rule, a Custom Lambda rule and a Custom Policy rule.
    emulator
        .ok("PutConfigRule", &managed_rule(NOT_A_PROCESS_CHECK))
        .await;
    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "lambda-rule",
                    "Source": {
                        "Owner": "CUSTOM_LAMBDA",
                        "SourceIdentifier":
                            "arn:aws:lambda:us-east-1:000000000000:function:evaluate",
                    },
                }
            }),
        )
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

    let account = this_account();
    // Rows differ along what makes the delivery unrecordable: which rule it names, the
    // resource type it evaluates, the account the resource id names (malformed, and
    // well formed but not the account this store serves), the compliance type it
    // carries, and whether its ordering timestamp names an instant at all.
    //
    // A rule this account does not hold is `NoSuchConfigRuleException`; a rule that
    // EXISTS and is not a process check is `InvalidParameterValueException`, which is
    // RECORDED behaviour — the recorded unit asserts it as "the rule exists, so the
    // refusal is about its type, not NoSuchConfigRuleException, which is reserved for a
    // rule name that does not exist at all".
    let cases: [(&str, Value, &str); 18] = [
        (
            "a rule this account does not hold",
            delivery(
                "no-such-rule",
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "NoSuchConfigRuleException",
        ),
        (
            "a rule name differing only in case",
            delivery(
                &RULE.to_uppercase(),
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "NoSuchConfigRuleException",
        ),
        (
            "a managed rule that is not a process check",
            delivery(
                NOT_A_PROCESS_CHECK,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "a Custom Lambda rule",
            delivery(
                "lambda-rule",
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "a Custom Policy rule",
            delivery(
                "policy-rule",
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "a resource type that is not the account one",
            delivery(
                RULE,
                "AWS::S3::Bucket",
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "a resource type differing only in case",
            delivery(
                RULE,
                "aws::::account",
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "the account resource type with stray whitespace",
            delivery(
                RULE,
                " AWS::::Account",
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "a resource id that is not an account id",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "my-bucket",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "an account id one digit short",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "12345678901",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "an account id one digit long",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "1234567890123",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "an account id of twelve non-digits",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "00000000000a",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "a well formed account id that is not the account this store serves",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "123456789012",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "this account's id with a leading digit changed",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "100000000000",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "the compliance type Config derives rather than accepts",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                INSUFFICIENT_DATA,
                1_700_000_000.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "an ordering timestamp further behind than an instant reaches",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                -1e30,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            "an ordering timestamp one second before the epoch",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                -1.0,
                None,
            ),
            "InvalidParameterValueException",
        ),
        (
            // The RULE is resolved before any value of the delivered evaluation is
            // judged, so a request that is wrong in BOTH is answered for the rule.
            "a rule this account does not hold AND an unplaceable ordering timestamp",
            delivery(
                "no-such-rule",
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                -1e30,
                None,
            ),
            "NoSuchConfigRuleException",
        ),
    ];

    for (case, request, expected_code) in cases {
        let (status, code) = emulator.refused("PutExternalEvaluation", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
    }

    // Nothing the refusals named was recorded: every rule still holds no result at
    // all, so a refusal is a refusal and not a partial write.
    for rule in [RULE, NOT_A_PROCESS_CHECK, "lambda-rule", "policy-rule"] {
        let results = recorded("after the refusals", &emulator, rule).await;
        assert!(
            results.is_empty(),
            "a refused delivery must record nothing, got {results:#?} for {rule:?}"
        );
    }

    // A delivery this emulator CAN record is served, so the refusals above are about
    // what each row carried and not about the operation at all.
    emulator
        .ok(
            "PutExternalEvaluation",
            &delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[1],
                1_700_000_000.0,
                None,
            ),
        )
        .await;
    let result = only_recorded("a recordable delivery", &emulator, RULE).await;
    assert_eq!(
        result["ComplianceType"].as_str(),
        Some(ACCEPTED_COMPLIANCE_TYPES[1]),
        "the recordable delivery must be the verdict the rule holds, got {result}"
    );
}

#[tokio::test]
async fn the_modelled_constraints_are_refused_at_the_request_boundary() {
    let emulator = emulator_with_process_check().await;
    let account = this_account();

    // Rows differ along which modelled constraint the request breaks, from
    // `smithy_query(query="operation", operation="PutExternalEvaluation")` and
    // `query="shape", shape="ExternalEvaluation"`: `ConfigRuleName` (len 1..128),
    // `ComplianceResourceType` and `Annotation` (`StringWithCharLimit256`, len
    // 1..256) and `ComplianceResourceId` (`BaseResourceId`, len 1..768). Each row
    // sits one past a published bound or at an empty value, which the framework
    // refuses before the handler runs — so the handler's own refusals above are about
    // the values the model DOES allow.
    let over_annotation = "r".repeat(ANNOTATION_MAX + 1);
    let over_resource_type = "A".repeat(257);
    let over_resource_id = "1".repeat(769);
    let over_rule_name = "p".repeat(129);
    let cases: [(&str, Value); 8] = [
        (
            "an annotation one character past the maximum",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                Some(over_annotation.as_str()),
            ),
        ),
        (
            "an empty annotation",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                Some(""),
            ),
        ),
        (
            "a rule name one character past the maximum",
            delivery(
                &over_rule_name,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
        ),
        (
            "an empty rule name",
            delivery(
                "",
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
        ),
        (
            "an empty resource type",
            delivery(
                RULE,
                "",
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
        ),
        (
            "a resource type one character past the maximum",
            delivery(
                RULE,
                &over_resource_type,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
        ),
        (
            "an empty resource id",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                "",
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
        ),
        (
            "a resource id one character past the maximum",
            delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &over_resource_id,
                ACCEPTED_COMPLIANCE_TYPES[0],
                1_700_000_000.0,
                None,
            ),
        ),
    ];

    for (case, request) in cases {
        let (status, code) = emulator.refused("PutExternalEvaluation", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }

    let results = recorded("after the boundary refusals", &emulator, RULE).await;
    assert!(
        results.is_empty(),
        "a request the boundary refuses must record nothing, got {results:#?}"
    );
}

#[tokio::test]
async fn reading_the_delivered_verdict_back_does_not_change_it() {
    let emulator = emulator_with_process_check().await;
    let account = this_account();
    emulator
        .ok(
            "PutExternalEvaluation",
            &delivery(
                RULE,
                ACCOUNT_RESOURCE_TYPE,
                &account,
                ACCEPTED_COMPLIANCE_TYPES[1],
                1_700_000_000.0,
                Some("the review is overdue"),
            ),
        )
        .await;

    let first = recorded("a first read", &emulator, RULE).await;
    let again = recorded("a second read", &emulator, RULE).await;
    assert_eq!(
        again, first,
        "a second read must report the same delivered verdict, got {again:#?}"
    );

    // The rule itself is untouched by the delivery's read path, and the delivery did
    // not rewrite the rule's definition either.
    let rules = emulator
        .ok("DescribeConfigRules", &json!({ "ConfigRuleNames": [RULE] }))
        .await;
    assert_eq!(
        rules["ConfigRules"][0]["Source"]["SourceIdentifier"].as_str(),
        Some(PROCESS_CHECK),
        "the delivery must leave the rule's definition alone, got {rules}"
    );
}
