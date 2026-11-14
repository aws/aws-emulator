// SPDX-License-Identifier: CC0-1.0

//! The evaluation results a caller DELIVERS for a process check rule, and the rule
//! compliance they roll up to.
//!
//! `PutExternalEvaluation` is the one writer of a Config rule's evaluation results a
//! client drives itself: a process check has no evaluator inside Config, so the caller
//! delivers each verdict. Three rules are pinned here over tables of inputs:
//!
//! 1. a delivered verdict REPLACES the result recorded for the same resource, and the
//!    rule's compliance is the roll-up of the results it then holds — read back
//!    through `GetComplianceSummaryByConfigRule`, which counts a rule by its
//!    compliance, and through `GetComplianceDetailsByConfigRule`, which reports the
//!    results themselves;
//! 2. the annotation a delivery carried is reported with the result it recorded, and
//!    absent when the delivery that holds carried none;
//! 3. the two refusals a client branches on are told apart by what is wrong: a rule
//!    name no rule answers to is `NoSuchConfigRuleException`, while an existing rule
//!    that is not a process check, a resource that is not this account (its type or
//!    its id), and a verdict Config never accepts from a deliverer are each
//!    `InvalidParameterValueException`.
//!
//! The compliance-change event the same write publishes is a cross-service behaviour
//! (Config -> EventBridge -> SQS) and is pinned in
//! `core/router/tests/all/config_compliance_change_event.rs`; the notification BODY is
//! pinned by the unit tests beside the builder in `src/state.rs`.
//!
//! The harness (`Emulator`, `managed_rule`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};
use crate::external_evaluation::put_process_check_rule;

/// The resource type an external evaluation is recorded against
/// (`ExternalEvaluation$ComplianceResourceType`: "Config accepts AWS::::Account
/// resource type").
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// Every `ComplianceType` a deliverer may send
/// (`ExternalEvaluation$ComplianceType`: "The valid values are COMPLIANT,
/// NON_COMPLIANT, and NOT_APPLICABLE").
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// The roll-up value Config keeps for itself: `ComplianceType` carries it, but
/// "Config does not accept INSUFFICIENT_DATA as the value for ComplianceType from a
/// PutEvaluations request" (`Evaluation$ComplianceType`).
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// An `OrderingTimestamp` the deliverer stamps, as epoch seconds — the wire form this
/// protocol's timestamps take.
const ORDERING_TIMESTAMP: f64 = 1_700_000_000.0;

/// The account every emulated resource belongs to
/// (`emulator_server::EMULATOR_ACCOUNT_ID`), which is the resource id a process check's
/// evaluation names.
const ACCOUNT_ID: &str = "000000000000";

/// A `PutExternalEvaluation` request delivering `compliance_type` for the account.
fn external_evaluation(rule: &str, compliance_type: &str) -> Value {
    json!({
        "ConfigRuleName": rule,
        "ExternalEvaluation": {
            "ComplianceResourceType": ACCOUNT_RESOURCE_TYPE,
            "ComplianceResourceId": ACCOUNT_ID,
            "ComplianceType": compliance_type,
            "OrderingTimestamp": ORDERING_TIMESTAMP,
        },
    })
}

/// A `PutExternalEvaluation` request delivering `compliance_type` for the account,
/// with `annotation` as its `ExternalEvaluation$Annotation` when it carries one.
fn annotated_evaluation(rule: &str, compliance_type: &str, annotation: Option<&str>) -> Value {
    let mut request = external_evaluation(rule, compliance_type);
    if let Some(annotation) = annotation {
        request["ExternalEvaluation"]["Annotation"] = Value::String(annotation.to_string());
    }
    request
}

/// How many rules of the account the compliance summary counts as compliant and as
/// noncompliant.
async fn summary_counts(emulator: &Emulator) -> (i64, i64) {
    let body = emulator
        .ok("GetComplianceSummaryByConfigRule", &json!({}))
        .await;
    let count = |member: &str| {
        body["ComplianceSummary"][member]["CappedCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("expected {member}.CappedCount, got {body}"))
    };
    (
        count("CompliantResourceCount"),
        count("NonCompliantResourceCount"),
    )
}

/// One evaluation result as `GetComplianceDetailsByConfigRule` reports it: the rule
/// and the resource it is about, the verdict it carries and the annotation that came
/// with the delivery that recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recorded {
    rule: String,
    resource_type: String,
    resource_id: String,
    compliance_type: String,
    annotation: Option<String>,
}

/// The evaluation results the rule `name` holds, as
/// `GetComplianceDetailsByConfigRule` reports them, in the order the response carried
/// them.
///
/// This is the READ side of the same map a delivered evaluation writes, so it is what
/// proves a delivery was RECORDED rather than merely accepted.
async fn recorded_results(emulator: &Emulator, name: &str) -> Vec<Recorded> {
    let body = emulator
        .ok(
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": name }),
        )
        .await;
    let results = body["EvaluationResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    results
        .iter()
        .map(|result| {
            let qualifier = &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
            let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
            Recorded {
                rule: text(&qualifier["ConfigRuleName"]),
                resource_type: text(&qualifier["ResourceType"]),
                resource_id: text(&qualifier["ResourceId"]),
                compliance_type: text(&result["ComplianceType"]),
                annotation: result["Annotation"].as_str().map(ToString::to_string),
            }
        })
        .collect()
}

#[tokio::test]
async fn a_delivered_verdict_replaces_the_result_recorded_for_that_resource() {
    // Rows differ along what the rule generalises over: how many verdicts are
    // delivered in a row, which values they take (each of the three a deliverer may
    // send), whether the last one repeats the previous verdict, and so which
    // compliance the rule rolls up to afterwards.
    let cases: [(&str, &[&str], &str); 7] = [
        ("one compliant verdict", &[COMPLIANT], COMPLIANT),
        ("one noncompliant verdict", &[NON_COMPLIANT], NON_COMPLIANT),
        (
            "not applicable places the rule neither way",
            &[NOT_APPLICABLE],
            INSUFFICIENT_DATA,
        ),
        (
            "compliant then noncompliant",
            &[COMPLIANT, NON_COMPLIANT],
            NON_COMPLIANT,
        ),
        (
            "noncompliant then compliant",
            &[NON_COMPLIANT, COMPLIANT],
            COMPLIANT,
        ),
        (
            "the same verdict twice",
            &[NON_COMPLIANT, NON_COMPLIANT],
            NON_COMPLIANT,
        ),
        (
            "three verdicts, ending not applicable",
            &[COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE],
            INSUFFICIENT_DATA,
        ),
    ];

    for (case, verdicts, expected) in cases {
        let emulator = Emulator::in_region("eu-west-1");
        with_recorder_and_channel(&emulator).await;
        let rule = "process-check";
        put_process_check_rule(&emulator, rule).await;

        // Before anything is delivered the rule is placed neither way.
        let (compliant, non_compliant) = summary_counts(&emulator).await;
        assert_eq!(
            (compliant, non_compliant),
            (0, 0),
            "case {case:?}: a rule nothing has evaluated counts in neither column, \
             got ({compliant}, {non_compliant})"
        );

        // Nothing has delivered a verdict yet, so the rule holds no result at all.
        let before = recorded_results(&emulator, rule).await;
        assert!(
            before.is_empty(),
            "case {case:?}: a rule nothing has evaluated holds no result, got {before:?}"
        );

        for verdict in verdicts {
            let answered = emulator
                .ok("PutExternalEvaluation", &external_evaluation(rule, verdict))
                .await;
            assert!(
                answered.is_null() || answered.is_object(),
                "case {case:?}: delivering {verdict} must answer the modelled empty \
                 response, got {answered}"
            );
        }

        // Each delivery REPLACED the previous result for the same resource, so the
        // rule's compliance is the one the LAST verdict implies — one resource, one
        // current verdict.
        let (compliant, non_compliant) = summary_counts(&emulator).await;
        let expected_counts = match expected {
            COMPLIANT => (1, 0),
            NON_COMPLIANT => (0, 1),
            _ => (0, 0),
        };
        assert_eq!(
            (compliant, non_compliant),
            expected_counts,
            "case {case:?}: after {verdicts:?} the rule must roll up to {expected}, \
             which counts as {expected_counts:?}, got ({compliant}, {non_compliant})"
        );

        // A second read agrees: delivering an evaluation changed state, reading it
        // does not.
        let (again_compliant, again_non_compliant) = summary_counts(&emulator).await;
        assert_eq!(
            (again_compliant, again_non_compliant),
            (compliant, non_compliant),
            "case {case:?}: a second summary must report the same counts, got \
             ({again_compliant}, {again_non_compliant})"
        );

        // The SAME replacement, read back through the operation that reports a rule's
        // evaluation results: one result, about the account resource, carrying the
        // verdict the last delivery recorded.
        let recorded = recorded_results(&emulator, rule).await;
        let last = *verdicts.last().expect("each row delivers a verdict");
        assert_eq!(
            recorded,
            vec![Recorded {
                rule: rule.to_string(),
                resource_type: ACCOUNT_RESOURCE_TYPE.to_string(),
                resource_id: ACCOUNT_ID.to_string(),
                compliance_type: last.to_string(),
                annotation: None,
            }],
            "case {case:?}: after {verdicts:?} the rule must hold ONE result for the \
             account carrying {last}, got {recorded:?}"
        );
    }
}

/// One delivery of an annotation row: the verdict delivered and the annotation that
/// came with it, if any.
type Delivery = (&'static str, Option<&'static str>);

/// One row of the annotation table: the deliveries made in order, and the annotation
/// the result must then report.
struct AnnotationCase {
    case: &'static str,
    deliveries: &'static [Delivery],
    expected: Option<&'static str>,
}

#[tokio::test]
async fn the_annotation_a_delivery_carries_is_reported_with_the_result_it_recorded() {
    // `ExternalEvaluation$Annotation` is "supplementary information about the reason
    // of compliance", so it is reported exactly as delivered and absent when no
    // delivery carried one. The rows differ along what the rule generalises over:
    // whether an annotation came with the delivery and what it says — including an
    // annotation REPLACED by a later delivery that carries none.
    let cases: [AnnotationCase; 4] = [
        AnnotationCase {
            case: "no annotation at all",
            deliveries: &[(COMPLIANT, None)],
            expected: None,
        },
        AnnotationCase {
            case: "one word",
            deliveries: &[(NON_COMPLIANT, Some("overdue"))],
            expected: Some("overdue"),
        },
        AnnotationCase {
            case: "a sentence with punctuation and non-ASCII text",
            deliveries: &[(NON_COMPLIANT, Some("the review is overdue — since Jänner"))],
            expected: Some("the review is overdue — since Jänner"),
        },
        AnnotationCase {
            case: "an annotated verdict replaced by an unannotated one",
            deliveries: &[(NON_COMPLIANT, Some("overdue")), (COMPLIANT, None)],
            expected: None,
        },
    ];

    for AnnotationCase {
        case,
        deliveries,
        expected,
    } in cases
    {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        let rule = "process-check";
        put_process_check_rule(&emulator, rule).await;

        for (verdict, annotation) in deliveries {
            emulator
                .ok(
                    "PutExternalEvaluation",
                    &annotated_evaluation(rule, verdict, *annotation),
                )
                .await;
        }

        let recorded = recorded_results(&emulator, rule).await;
        let reported = recorded
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected one result, got {recorded:?}"))
            .annotation
            .clone();
        assert_eq!(
            reported.as_deref(),
            expected,
            "case {case:?}: expected the annotation {expected:?} the last delivery \
             carried, got {recorded:?}"
        );
    }
}

#[tokio::test]
async fn an_evaluation_config_cannot_record_is_refused_by_what_is_wrong_with_it() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    put_process_check_rule(&emulator, "process-check").await;
    emulator.ok("PutConfigRule", &managed_rule("managed")).await;

    // Rows differ along WHICH part of the request Config cannot record: the rule
    // (absent — `NoSuchConfigRuleException` — or existing but not a process check, which
    // is a rule of the wrong KIND for this call and so a bad parameter value: RECORDED
    // behaviour, asserted by the recorded unit itself as "the rule exists, so the refusal
    // is about its type, not NoSuchConfigRuleException, which is reserved for a rule name
    // that does not exist at all"), the resource the evaluation names (its type and its
    // id), and the verdict it carries. The RULE is resolved before the delivered
    // evaluation is judged, so a request that is wrong in both is answered for the rule
    // it named.
    let cases: [(&str, Value, &str); 8] = [
        (
            "a rule no rule answers to",
            external_evaluation("never-created", COMPLIANT),
            "NoSuchConfigRuleException",
        ),
        (
            "an existing rule that is not a process check",
            external_evaluation("managed", COMPLIANT),
            "InvalidParameterValueException",
        ),
        (
            "a resource type that is not the account",
            json!({
                "ConfigRuleName": "process-check",
                "ExternalEvaluation": {
                    "ComplianceResourceType": "AWS::EC2::Volume",
                    "ComplianceResourceId": "vol-0123456789abcdef0",
                    "ComplianceType": COMPLIANT,
                    "OrderingTimestamp": ORDERING_TIMESTAMP,
                },
            }),
            "InvalidParameterValueException",
        ),
        (
            "another resource type that is not the account",
            json!({
                "ConfigRuleName": "process-check",
                "ExternalEvaluation": {
                    "ComplianceResourceType": "AWS::IAM::Role",
                    "ComplianceResourceId": "config-role",
                    "ComplianceType": NON_COMPLIANT,
                    "OrderingTimestamp": ORDERING_TIMESTAMP,
                },
            }),
            "InvalidParameterValueException",
        ),
        (
            "a resource id that is not this account",
            json!({
                "ConfigRuleName": "process-check",
                "ExternalEvaluation": {
                    "ComplianceResourceType": ACCOUNT_RESOURCE_TYPE,
                    "ComplianceResourceId": "210987654321",
                    "ComplianceType": COMPLIANT,
                    "OrderingTimestamp": ORDERING_TIMESTAMP,
                },
            }),
            "InvalidParameterValueException",
        ),
        (
            "the roll-up value a deliverer may not send",
            external_evaluation("process-check", INSUFFICIENT_DATA),
            "InvalidParameterValueException",
        ),
        (
            "another absent rule, with a valid body",
            external_evaluation("process-check-2", NOT_APPLICABLE),
            "NoSuchConfigRuleException",
        ),
        (
            "an absent rule AND a resource that is not the account",
            json!({
                "ConfigRuleName": "never-created",
                "ExternalEvaluation": {
                    "ComplianceResourceType": "AWS::EC2::Volume",
                    "ComplianceResourceId": "vol-0123456789abcdef0",
                    "ComplianceType": INSUFFICIENT_DATA,
                    "OrderingTimestamp": ORDERING_TIMESTAMP,
                },
            }),
            "NoSuchConfigRuleException",
        ),
    ];

    for (case, request, expected_code) in cases {
        let (status, code) = emulator.refused("PutExternalEvaluation", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} {code}"
        );
    }

    // A refusal records nothing: the process check rule is still placed neither way.
    let (compliant, non_compliant) = summary_counts(&emulator).await;
    assert_eq!(
        (compliant, non_compliant),
        (0, 0),
        "a refused delivery must record no evaluation, got ({compliant}, {non_compliant})"
    );
}
