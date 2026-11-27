// SPDX-License-Identifier: CC0-1.0

//! `PutEvaluations`: the verdicts a Config rule's own evaluation delivers, and every
//! compliance read that then reports them.
//!
//! The rule pinned here: a verdict delivered for a Config rule becomes that rule's
//! recorded evaluation result, and from then on EVERY read of that rule's compliance
//! reports it — the rule's own `GetComplianceDetailsByConfigRule`, the aggregate
//! `GetAggregateComplianceDetailsByConfigRule` for the source account and Region the
//! data was aggregated from, and the counts
//! `GetAggregateConfigRuleComplianceSummary` rolls those results up into. A delivery is
//! associated with its rule by the `ResultToken` Config minted when it invoked that
//! rule: a token Config did not mint is refused, a token Config minted for ANY invocation
//! of the rule is accepted — a rule has several invocations outstanding at a time, one per
//! event that triggered one — and a token naming a rule the account no longer holds is
//! refused as that.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along how many verdicts one delivery
//! carries, which `ComplianceType` each one is, which resource type and id it names,
//! whether it carries an annotation, and whether the delivery is a test run.
//!
//! The `ResultToken` is the one value a client cannot read back over this API: Config
//! hands it to whatever it invoked to perform the evaluation. These cases read it from
//! the same shared store the handler mints it into, which is where that invocation
//! would carry it from.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::ComAmazonawsConfigserviceStarlingdoveserviceState as ConfigState;

use crate::config_setup_and_rule_loop::{with_recorder_and_channel, Emulator};

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Resource types a delivered verdict names, from
/// `smithy_query(query="shape", shape="ResourceType")`; `AWS::::Account` is the
/// account-level type Config accepts for an evaluation of the account itself
/// (`ExternalEvaluation$ComplianceResourceType`).
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const ACCOUNT: &str = "AWS::::Account";

/// The aggregator every case that reads an aggregate surface uses.
const AGGREGATOR: &str = "org-wide";

/// The Lambda function a Custom Lambda rule points at; its value is read back, never
/// invoked.
const FUNCTION_ARN: &str = "arn:aws:lambda:us-east-1:000000000000:function:config-rule";

/// This emulator's account id, the account whose Config data the store holds.
fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// One verdict a delivery carries: the resource it is about, the compliance it reports
/// and the annotation it carries, if any.
#[derive(Debug, Clone, Copy)]
struct Verdict {
    resource_type: &'static str,
    resource_id: &'static str,
    compliance_type: &'static str,
    annotation: Option<&'static str>,
}

impl Verdict {
    /// The modelled `Evaluation` this verdict is delivered as, ordered at `ordered_at`
    /// epoch seconds.
    fn to_evaluation(self, ordered_at: u64) -> Value {
        let mut evaluation = json!({
            "ComplianceResourceType": self.resource_type,
            "ComplianceResourceId": self.resource_id,
            "ComplianceType": self.compliance_type,
            "OrderingTimestamp": ordered_at,
        });
        if let Some(annotation) = self.annotation {
            evaluation["Annotation"] = json!(annotation);
        }
        evaluation
    }
}

/// A Config rule whose evaluation Config performs by invoking a function of the
/// account's own, which is the rule class that delivers its verdicts with
/// `PutEvaluations` (`Source$Owner` `CUSTOM_LAMBDA`).
///
/// `pub(crate)` because every suite that drives a DELIVERED verdict over the wire needs
/// exactly this rule (`compliance_summary_by_resource_type` is the other one).
pub(crate) fn custom_lambda_rule(name: &str) -> Value {
    json!({
        "ConfigRule": {
            "ConfigRuleName": name,
            "Source": {
                "Owner": "CUSTOM_LAMBDA",
                "SourceIdentifier": FUNCTION_ARN,
                "SourceDetails": [{
                    "EventSource": "aws.config",
                    "MessageType": "ScheduledNotification",
                    "MaximumExecutionFrequency": "TwentyFour_Hours",
                }],
            },
        }
    })
}

/// The `ResultToken` Config minted when it invoked the rule `name` — the token the
/// invocation carries to whatever performs the evaluation, read from the shared store
/// the handler wrote it to.
///
/// `pub(crate)` for the same reason [`custom_lambda_rule`] is: a delivery cannot be
/// driven over the wire without the token that associates it with its rule.
pub(crate) fn result_token(emulator: &Emulator, name: &str) -> String {
    let state = emulator.engine.state::<ConfigState>();
    let store = state.lock();
    store
        .config_rules
        .get(name)
        .unwrap_or_else(|| panic!("the rule {name:?} must have been created first"))
        .evaluation
        .result_token
        .clone()
        .unwrap_or_else(|| panic!("Config must mint a ResultToken for the rule {name:?}"))
}

/// An emulator holding a recorder, a delivery channel, an aggregator over this account
/// and the Custom Lambda rule `name`.
async fn emulator_with_rule(name: &str) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "AccountAggregationSources": [{
                    "AccountIds": [account()],
                    "AllAwsRegions": true,
                }],
            }),
        )
        .await;
    emulator.ok("PutConfigRule", &custom_lambda_rule(name)).await;
    emulator
}

/// The `(resourceId, ComplianceType, Annotation)` of every result
/// `GetComplianceDetailsByConfigRule` reports for `name`, in the order it reported them.
async fn reported_details(
    case: &str,
    emulator: &Emulator,
    name: &str,
) -> Vec<(String, String, Option<String>)> {
    let body = emulator
        .ok(
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": name }),
        )
        .await;
    body["EvaluationResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected EvaluationResults, got {body}"))
        .iter()
        .map(|result| {
            let qualifier = &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
            (
                qualifier["ResourceId"].as_str().unwrap_or_default().to_string(),
                result["ComplianceType"].as_str().unwrap_or_default().to_string(),
                result["Annotation"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// The same triples the AGGREGATE read reports for `name`, each checked to carry the
/// source account and Region it was aggregated from.
async fn reported_aggregate_details(
    case: &str,
    emulator: &Emulator,
    name: &str,
) -> Vec<(String, String, Option<String>)> {
    let body = emulator
        .ok(
            "GetAggregateComplianceDetailsByConfigRule",
            &json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "ConfigRuleName": name,
                "AccountId": account(),
                "AwsRegion": emulator.region,
            }),
        )
        .await;
    body["AggregateEvaluationResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected AggregateEvaluationResults, got {body}"))
        .iter()
        .map(|result| {
            assert_eq!(
                (result["AccountId"].as_str(), result["AwsRegion"].as_str()),
                (Some(account().as_str()), Some(emulator.region.as_str())),
                "case {case:?}: every aggregate result names the source it came from, got {result}"
            );
            let qualifier = &result["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
            (
                qualifier["ResourceId"].as_str().unwrap_or_default().to_string(),
                result["ComplianceType"].as_str().unwrap_or_default().to_string(),
                result["Annotation"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// How many rules the aggregate summary counts as noncompliant for this account.
async fn aggregate_noncompliant_count(case: &str, emulator: &Emulator) -> i64 {
    let body = emulator
        .ok(
            "GetAggregateConfigRuleComplianceSummary",
            &json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "Filters": { "AccountId": account(), "AwsRegion": emulator.region },
            }),
        )
        .await;
    let counts = body["AggregateComplianceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected AggregateComplianceCounts, got {body}"));
    counts
        .iter()
        .map(|group| {
            group["ComplianceSummary"]["NonCompliantResourceCount"]["CappedCount"]
                .as_i64()
                .unwrap_or_default()
        })
        .sum()
}

#[tokio::test]
async fn a_delivered_verdict_is_reported_by_every_compliance_read() {
    // (case, the verdicts one delivery carries). The rows differ along how many
    // verdicts a delivery carries, which compliance each reports, which resource type
    // and id it names and whether it carries an annotation.
    let cases: [(&str, &[Verdict]); 6] = [
        (
            "one noncompliant resource, annotated",
            &[Verdict {
                resource_type: EC2_VOLUME,
                resource_id: "vol-0001",
                compliance_type: NON_COMPLIANT,
                annotation: Some("encryption is not enabled"),
            }],
        ),
        (
            "one compliant resource with no annotation",
            &[Verdict {
                resource_type: S3_BUCKET,
                resource_id: "config-bucket",
                compliance_type: COMPLIANT,
                annotation: None,
            }],
        ),
        (
            "the account itself, the account-level resource type",
            &[Verdict {
                resource_type: ACCOUNT,
                resource_id: "000000000000",
                compliance_type: NON_COMPLIANT,
                annotation: Some("the account has no trail"),
            }],
        ),
        (
            "a verdict that applies to nothing",
            &[Verdict {
                resource_type: EC2_VOLUME,
                resource_id: "vol-0002",
                compliance_type: NOT_APPLICABLE,
                annotation: None,
            }],
        ),
        (
            "several resources of one type",
            &[
                Verdict {
                    resource_type: EC2_VOLUME,
                    resource_id: "vol-0003",
                    compliance_type: COMPLIANT,
                    annotation: None,
                },
                Verdict {
                    resource_type: EC2_VOLUME,
                    resource_id: "vol-0004",
                    compliance_type: NON_COMPLIANT,
                    annotation: Some("unencrypted"),
                },
            ],
        ),
        (
            "resources of several types, every accepted compliance",
            &[
                Verdict {
                    resource_type: S3_BUCKET,
                    resource_id: "logs-bucket",
                    compliance_type: NOT_APPLICABLE,
                    annotation: None,
                },
                Verdict {
                    resource_type: EC2_VOLUME,
                    resource_id: "vol-0005",
                    compliance_type: COMPLIANT,
                    annotation: Some("encrypted"),
                },
                Verdict {
                    resource_type: ACCOUNT,
                    resource_id: "000000000000",
                    compliance_type: NON_COMPLIANT,
                    annotation: None,
                },
            ],
        ),
    ];

    for (case, verdicts) in cases {
        let name = "custom-lambda-rule";
        let emulator = emulator_with_rule(name).await;

        // Nothing has been delivered yet, so no read reports a verdict and the summary
        // counts this rule as neither compliant nor noncompliant.
        let baseline = aggregate_noncompliant_count(case, &emulator).await;
        assert!(
            reported_details(case, &emulator, name).await.is_empty()
                && reported_aggregate_details(case, &emulator, name)
                    .await
                    .is_empty(),
            "case {case:?}: a rule nothing has evaluated reports no result"
        );

        let answered = emulator
            .ok(
                "PutEvaluations",
                &json!({
                    "ResultToken": result_token(&emulator, name),
                    "Evaluations": verdicts
                        .iter()
                        .enumerate()
                        .map(|(index, verdict)| verdict.to_evaluation(1_700_000_000 + index as u64))
                        .collect::<Vec<Value>>(),
                }),
            )
            .await;
        assert_eq!(
            answered["FailedEvaluations"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: every delivered verdict was recorded, got {answered}"
        );

        // What each read reports: the delivered verdicts, keyed by the resource each
        // was about, in the store's own ordering.
        let mut expected: Vec<(String, String, Option<String>)> = verdicts
            .iter()
            .map(|verdict| {
                (
                    verdict.resource_id.to_string(),
                    verdict.compliance_type.to_string(),
                    verdict.annotation.map(str::to_string),
                )
            })
            .collect();
        expected.sort();

        let mut reported = reported_details(case, &emulator, name).await;
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the rule's own details to report {expected:?}, got \
             {reported:?}"
        );

        let mut aggregated = reported_aggregate_details(case, &emulator, name).await;
        aggregated.sort();
        assert_eq!(
            aggregated, expected,
            "case {case:?}: expected the aggregate details to report {expected:?}, got \
             {aggregated:?}"
        );

        // A rule any verdict places as noncompliant moves the aggregate summary by
        // exactly one; a rule no verdict places that way leaves it where it was.
        let expected_summary = baseline
            + i64::from(
                verdicts
                    .iter()
                    .any(|verdict| verdict.compliance_type == NON_COMPLIANT),
            );
        let summarised = aggregate_noncompliant_count(case, &emulator).await;
        assert_eq!(
            summarised, expected_summary,
            "case {case:?}: expected the aggregate summary to count {expected_summary} \
             noncompliant rule(s), got {summarised}"
        );
    }
}

#[tokio::test]
async fn a_second_delivery_for_one_resource_revises_the_verdict_in_place() {
    // A rule reports its LAST verdict per resource: delivering again for the same
    // resource replaces the result rather than adding one, which is what moves the
    // summary back off noncompliant.
    let name = "revised-rule";
    let emulator = emulator_with_rule(name).await;
    let token = result_token(&emulator, name);

    for (case, compliance, expected_noncompliant) in [
        ("first verdict", NON_COMPLIANT, 1),
        ("revised to compliant", COMPLIANT, 0),
        ("revised back", NON_COMPLIANT, 1),
    ] {
        emulator
            .ok(
                "PutEvaluations",
                &json!({
                    "ResultToken": token,
                    "Evaluations": [{
                        "ComplianceResourceType": EC2_VOLUME,
                        "ComplianceResourceId": "vol-0001",
                        "ComplianceType": compliance,
                        "OrderingTimestamp": 1_700_000_000,
                    }],
                }),
            )
            .await;

        let reported = reported_details(case, &emulator, name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: one resource has one result, got {reported:?}"
        );
        let summarised = aggregate_noncompliant_count(case, &emulator).await;
        assert_eq!(
            summarised, expected_noncompliant,
            "case {case:?}: expected {expected_noncompliant} noncompliant rule(s), got \
             {summarised}"
        );
    }
}

#[tokio::test]
async fn a_test_run_validates_the_delivery_and_records_nothing() {
    let name = "test-mode-rule";
    let emulator = emulator_with_rule(name).await;

    // A test run does not need a valid token ("TestMode ... doesn't require a valid
    // value for the ResultToken parameter, but the value cannot be null") and records
    // nothing.
    for (case, token) in [
        ("the rule's own token", result_token(&emulator, name)),
        ("a token Config never minted", "not-a-token".to_string()),
    ] {
        let answered = emulator
            .ok(
                "PutEvaluations",
                &json!({
                    "ResultToken": token,
                    "TestMode": true,
                    "Evaluations": [{
                        "ComplianceResourceType": EC2_VOLUME,
                        "ComplianceResourceId": "vol-0001",
                        "ComplianceType": NON_COMPLIANT,
                        "OrderingTimestamp": 1_700_000_000,
                    }],
                }),
            )
            .await;
        assert_eq!(
            answered["FailedEvaluations"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a test run reports no failed evaluation, got {answered}"
        );
        assert!(
            reported_details(case, &emulator, name).await.is_empty(),
            "case {case:?}: a test run records no evaluation result"
        );
        assert_eq!(
            aggregate_noncompliant_count(case, &emulator).await,
            0,
            "case {case:?}: a test run moves no summary"
        );
    }

    // It is validated exactly as a live delivery is, so a compliance Config does not
    // accept from a caller is refused even in test mode.
    let (status, code) = emulator
        .refused(
            "PutEvaluations",
            &json!({
                "ResultToken": result_token(&emulator, name),
                "TestMode": true,
                "Evaluations": [{
                    "ComplianceResourceType": EC2_VOLUME,
                    "ComplianceResourceId": "vol-0001",
                    "ComplianceType": INSUFFICIENT_DATA,
                    "OrderingTimestamp": 1_700_000_000,
                }],
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidParameterValueException"),
        "a test run of a delivery Config does not accept is refused as the live one is, got \
         {status} {code}"
    );
}

#[tokio::test]
async fn refuses_a_delivery_no_invocation_of_an_existing_rule_asked_for() {
    let name = "token-rule";
    let emulator = emulator_with_rule(name).await;
    let minted = result_token(&emulator, name);

    // A token Config did not mint names no rule and no evaluation event.
    for (case, token) in [
        ("a forged token", "not-a-token-config-minted".to_string()),
        ("an empty-looking token", " ".to_string()),
        (
            "a token that decodes to nothing Config issued",
            aws_smithy_types::base64::encode("0:"),
        ),
    ] {
        let (status, code) = emulator
            .refused(
                "PutEvaluations",
                &json!({ "ResultToken": token, "Evaluations": [] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidResultTokenException"),
            "case {case:?}: expected HTTP 400 InvalidResultTokenException, got {status} {code}"
        );
    }

    // Config invoking the rule again mints a NEW token, and the one an earlier invocation
    // was made with is still a token Config accepts a delivery on: a rule has several
    // invocations outstanding at a time — a change-triggered rule is invoked once per
    // configuration item — and each evaluator delivers what it found on the token of the
    // event it was handed, whichever invocation Config made last.
    emulator
        .ok(
            "StartConfigRulesEvaluation",
            &json!({ "ConfigRuleNames": [name] }),
        )
        .await;
    let reinvoked = result_token(&emulator, name);
    assert_ne!(
        reinvoked, minted,
        "a new invocation mints a new ResultToken, got {reinvoked:?} twice"
    );
    let delivered = emulator
        .ok(
            "PutEvaluations",
            &json!({ "ResultToken": minted, "Evaluations": [] }),
        )
        .await;
    assert_eq!(
        delivered["FailedEvaluations"],
        json!([]),
        "a delivery on the token of an earlier invocation is accepted, got {delivered}"
    );

    // A token that names a rule the account no longer holds is refused as that rule
    // being gone, which is the error a caller retries differently on.
    emulator
        .ok("DeleteConfigRule", &json!({ "ConfigRuleName": name }))
        .await;
    let (status, code) = emulator
        .refused(
            "PutEvaluations",
            &json!({ "ResultToken": reinvoked, "Evaluations": [] }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoSuchConfigRuleException"),
        "a ResultToken for a deleted rule is refused as no such rule, got {status} {code}"
    );
}

#[tokio::test]
async fn refuses_a_verdict_config_does_not_accept_from_a_caller() {
    let name = "refusal-rule";
    let emulator = emulator_with_rule(name).await;
    let token = result_token(&emulator, name);

    // (case, the evaluation delivered, the expected error code)
    let cases: [(&str, Value, &str); 3] = [
        (
            "INSUFFICIENT_DATA, which Config derives and never accepts",
            json!({
                "ComplianceResourceType": EC2_VOLUME,
                "ComplianceResourceId": "vol-0001",
                "ComplianceType": INSUFFICIENT_DATA,
                "OrderingTimestamp": 1_700_000_000,
            }),
            "InvalidParameterValueException",
        ),
        (
            "a compliance no published value names",
            json!({
                "ComplianceResourceType": EC2_VOLUME,
                "ComplianceResourceId": "vol-0001",
                "ComplianceType": "MOSTLY_COMPLIANT",
                "OrderingTimestamp": 1_700_000_000,
            }),
            "ValidationException",
        ),
        (
            "an ordering timestamp before the epoch",
            json!({
                "ComplianceResourceType": EC2_VOLUME,
                "ComplianceResourceId": "vol-0001",
                "ComplianceType": NON_COMPLIANT,
                "OrderingTimestamp": -1,
            }),
            "InvalidParameterValueException",
        ),
    ];

    for (case, evaluation, expected) in cases {
        let (status, code) = emulator
            .refused(
                "PutEvaluations",
                &json!({ "ResultToken": token, "Evaluations": [evaluation] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
        );
        assert!(
            reported_details(case, &emulator, name).await.is_empty(),
            "case {case:?}: a refused delivery records nothing"
        );
    }
}

#[tokio::test]
async fn the_aggregate_details_read_answers_only_for_the_source_it_holds() {
    let name = "aggregate-rule";
    let emulator = emulator_with_rule(name).await;
    emulator
        .ok(
            "PutEvaluations",
            &json!({
                "ResultToken": result_token(&emulator, name),
                "Evaluations": [{
                    "ComplianceResourceType": EC2_VOLUME,
                    "ComplianceResourceId": "vol-0001",
                    "ComplianceType": NON_COMPLIANT,
                    "OrderingTimestamp": 1_700_000_000,
                }],
            }),
        )
        .await;

    // (case, the source and rule the request asks about, how many results it holds)
    let cases: [(&str, Value, usize); 5] = [
        (
            "this account in this Region",
            json!({ "AccountId": account(), "AwsRegion": emulator.region, "ConfigRuleName": name }),
            1,
        ),
        (
            "another account",
            json!({
                "AccountId": "111122223333",
                "AwsRegion": emulator.region,
                "ConfigRuleName": name,
            }),
            0,
        ),
        (
            "another Region",
            json!({ "AccountId": account(), "AwsRegion": "eu-west-1", "ConfigRuleName": name }),
            0,
        ),
        (
            "a rule this account does not hold",
            json!({
                "AccountId": account(),
                "AwsRegion": emulator.region,
                "ConfigRuleName": "no-such-rule",
            }),
            0,
        ),
        (
            "filtered to the compliance the verdict reports",
            json!({
                "AccountId": account(),
                "AwsRegion": emulator.region,
                "ConfigRuleName": name,
                "ComplianceType": NON_COMPLIANT,
            }),
            1,
        ),
    ];

    for (case, mut request, expected) in cases {
        request["ConfigurationAggregatorName"] = json!(AGGREGATOR);
        let body = emulator
            .ok("GetAggregateComplianceDetailsByConfigRule", &request)
            .await;
        let results = body["AggregateEvaluationResults"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected AggregateEvaluationResults, got {body}")
            });
        assert_eq!(
            results.len(),
            expected,
            "case {case:?}: expected {expected} result(s), got {body}"
        );
    }

    // (case, the request, the expected error code) — the refusals this read models.
    let refusals: [(&str, Value, &str); 3] = [
        (
            "an aggregator that does not exist",
            json!({
                "ConfigurationAggregatorName": "no-such-aggregator",
                "ConfigRuleName": name,
                "AccountId": account(),
                "AwsRegion": emulator.region,
            }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "a compliance this read does not support",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "ConfigRuleName": name,
                "AccountId": account(),
                "AwsRegion": emulator.region,
                "ComplianceType": NOT_APPLICABLE,
            }),
            "ValidationException",
        ),
        (
            "a token this operation did not issue",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "ConfigRuleName": name,
                "AccountId": account(),
                "AwsRegion": emulator.region,
                "NextToken": "!!! not a token !!!",
            }),
            "InvalidNextTokenException",
        ),
    ];

    for (case, request, expected) in refusals {
        let (status, code) = emulator
            .refused("GetAggregateComplianceDetailsByConfigRule", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn the_aggregate_details_read_pages_over_the_results_it_reports() {
    let name = "paged-rule";
    let emulator = emulator_with_rule(name).await;
    let delivered: Vec<Value> = (0..5)
        .map(|index| {
            json!({
                "ComplianceResourceType": EC2_VOLUME,
                "ComplianceResourceId": format!("vol-{index:04}"),
                "ComplianceType": NON_COMPLIANT,
                "OrderingTimestamp": 1_700_000_000,
            })
        })
        .collect();
    emulator
        .ok(
            "PutEvaluations",
            &json!({
                "ResultToken": result_token(&emulator, name),
                "Evaluations": delivered,
            }),
        )
        .await;

    // Every page size from one per page to more than there are: the walk reports each
    // result exactly once, whatever the page size.
    for limit in [1, 2, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut request = json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "ConfigRuleName": name,
                "AccountId": account(),
                "AwsRegion": emulator.region,
                "Limit": limit,
            });
            if let Some(token) = next.clone() {
                request["NextToken"] = json!(token);
            }
            let body = emulator
                .ok("GetAggregateComplianceDetailsByConfigRule", &request)
                .await;
            let page = body["AggregateEvaluationResults"]
                .as_array()
                .unwrap_or_else(|| panic!("case {limit}: expected a page, got {body}"))
                .clone();
            assert!(
                page.len() <= limit.min(5),
                "case {limit}: a page carries at most the Limit asked for, got {body}"
            );
            for result in page {
                seen.push(
                    result["EvaluationResultIdentifier"]["EvaluationResultQualifier"]["ResourceId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next = body["NextToken"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
        }
        seen.sort();
        let expected: Vec<String> = (0..5).map(|index| format!("vol-{index:04}")).collect();
        assert_eq!(
            seen, expected,
            "case {limit}: expected every result once, got {seen:?}"
        );
    }
}
