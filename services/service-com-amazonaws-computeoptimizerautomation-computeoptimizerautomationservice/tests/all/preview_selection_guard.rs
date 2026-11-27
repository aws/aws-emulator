// SPDX-License-Identifier: CC0-1.0

//! The ONE judgement both preview operations make about a preview request's
//! selection.
//!
//! `ListAutomationRulePreview` reports the selected actions one by one and
//! `ListAutomationRulePreviewSummaries` aggregates the very same set, so the
//! rule under test is that one request shape is judged identically by both: a
//! selection one of them cannot evaluate is refused by the other with the same
//! status, the same error code and the same message, a selection either serves
//! is served by both, and a page token binds the selection it was taken with to
//! the operation that issued it.
//!
//! The table below drives each request shape through BOTH operations over the
//! wire — an awsJson1_0 `POST /` through the same router the emulator binary
//! mounts — and the rows differ along what the judgement spans: the required
//! action-type list, the organization scope read with the rule type, and each
//! form of criteria condition the protocol boundary admits (a condition with no
//! operator, with no values, with an operator from the wrong family, past the
//! documented size cap, and a tag condition with no key).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    Criteria, RecommendedAction,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use super::automation_rule_lifecycle::{call, call_ok, emulator_with_engine, error_code, member};
use super::error_payloads::error_message;

/// The two operations that read a preview request's selection
/// (`smithy_query(query="digest")`: the operations taking `ruleType`,
/// `organizationScope`, `recommendedActionTypes` and `criteria`).
const PREVIEW_OPERATIONS: [&str; 2] = [
    "ListAutomationRulePreview",
    "ListAutomationRulePreviewSummaries",
];

/// The two `RecommendedActionType` values, from
/// `smithy_query(query="shape",shape="RecommendedActionType")`.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// A second, member account id for the organization-scope rows: a literal
/// 12-digit id, as the `AccountId` pattern writes one.
const MEMBER_ACCOUNT: &str = "210987654321";

/// What a preview request's selection is judged to be.
#[derive(Debug)]
enum Verdict {
    /// The selection is evaluable, so both operations answer HTTP 200.
    Served,
    /// The selection cannot be evaluated, so both operations refuse it with
    /// `InvalidParameterValueException` reporting `requirement` on `member`.
    Refused {
        member: String,
        requirement: String,
    },
}

/// The crate's validation report for one member a handler refused.
fn constraint_report(member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         {requirement}"
    )
}

fn refused(member: &str, requirement: &str) -> Verdict {
    Verdict::Refused {
        member: member.to_string(),
        requirement: requirement.to_string(),
    }
}

/// A criteria condition repeated `count` times, for the size-cap rows.
fn repeated_conditions(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "comparison": "StringEquals", "values": [format!("us-east-{index}")] }))
            .collect(),
    )
}

/// `count` values inside ONE condition, for the per-condition size-cap rows.
fn condition_with_values(count: usize) -> Value {
    json!([{
        "comparison": "StringEquals",
        "values": (0..count).map(|index| format!("us-east-{index}")).collect::<Vec<_>>(),
    }])
}

/// One request shape per row, with the verdict both preview operations must
/// reach on it.
///
/// The `criteria` size caps are asserted AT the documented bound and one past
/// it, so a row that is served and a row that is refused differ only in the
/// count.
fn selection_cases() -> Vec<(String, Value, Verdict)> {
    let conditions_cap = state::MAX_CONDITIONS_PER_CRITERION;
    let values_cap = state::MAX_VALUES_PER_CONDITION;
    vec![
        (
            "an account-scoped selection of one action type".to_string(),
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [UPGRADE] }),
            Verdict::Served,
        ),
        (
            "an organization-wide selection naming its accounts".to_string(),
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
            Verdict::Served,
        ),
        (
            "an organization-wide selection naming no accounts".to_string(),
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [SNAPSHOT_AND_DELETE],
            }),
            Verdict::Served,
        ),
        (
            "a selection with evaluable criteria of every criterion kind".to_string(),
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }],
                    "resourceTag": [{
                        "key": "stack", "comparison": "StringLike", "values": ["prod*"],
                    }],
                    "ebsVolumeSizeInGib": [{
                        "comparison": "NumericGreaterThan", "values": [10],
                    }],
                    "estimatedMonthlySavings": [{
                        "comparison": "NumericLessThanEquals", "values": [100.5],
                    }],
                },
            }),
            Verdict::Served,
        ),
        (
            "no action type to include".to_string(),
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [] }),
            refused(
                "recommendedActionTypes",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            "an organization scope on an account-scoped selection".to_string(),
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
            refused(
                "organizationScope",
                "Member is valid only for the OrganizationRule rule type",
            ),
        ),
        (
            "an empty action-type list sent WITH a refusable scope".to_string(),
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
            // The clause order is part of the judgement: the required member is
            // reported first, so both operations name the same one member.
            refused(
                "recommendedActionTypes",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            "a text condition with no comparison operator".to_string(),
            preview_with_criteria(json!({ "region": [{ "values": ["us-east-1"] }] })),
            refused("criteria.region.member.comparison", "Member is required"),
        ),
        (
            "a numeric condition with no comparison operator".to_string(),
            preview_with_criteria(json!({ "ebsVolumeSizeInGib": [{ "values": [10] }] })),
            refused(
                "criteria.ebsVolumeSizeInGib.member.comparison",
                "Member is required",
            ),
        ),
        (
            "a numeric operator on a text criterion".to_string(),
            preview_with_criteria(json!({
                "resourceArn": [{ "comparison": "NumericGreaterThan", "values": ["arn"] }]
            })),
            refused(
                "criteria.resourceArn.member.comparison",
                "Member must be a String comparison operator for this criterion",
            ),
        ),
        (
            "a text operator on a numeric criterion".to_string(),
            preview_with_criteria(json!({
                "lookBackPeriodInDays": [{ "comparison": "StringEquals", "values": [14] }]
            })),
            refused(
                "criteria.lookBackPeriodInDays.member.comparison",
                "Member must be a Numeric comparison operator for this criterion",
            ),
        ),
        (
            "a condition with no values to compare against".to_string(),
            preview_with_criteria(json!({
                "ebsVolumeType": [{ "comparison": "StringEquals", "values": [] }]
            })),
            refused(
                "criteria.ebsVolumeType.member.values",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            "a tag condition with no key".to_string(),
            preview_with_criteria(json!({
                "resourceTag": [{ "comparison": "StringEquals", "values": ["prod"] }]
            })),
            refused("criteria.resourceTag.member.key", "Member is required"),
        ),
        (
            format!("a criterion of exactly {conditions_cap} conditions"),
            preview_with_criteria(json!({ "region": repeated_conditions(conditions_cap) })),
            Verdict::Served,
        ),
        (
            format!("a criterion of {} conditions", conditions_cap + 1),
            preview_with_criteria(json!({ "region": repeated_conditions(conditions_cap + 1) })),
            refused(
                "criteria.region",
                &format!("Member must have length less than or equal to {conditions_cap}"),
            ),
        ),
        (
            format!("a condition of exactly {values_cap} values"),
            preview_with_criteria(json!({ "region": condition_with_values(values_cap) })),
            Verdict::Served,
        ),
        (
            format!("a condition of {} values", values_cap + 1),
            preview_with_criteria(json!({ "region": condition_with_values(values_cap + 1) })),
            refused(
                "criteria.region.member.values",
                &format!("Member must have length less than or equal to {values_cap}"),
            ),
        ),
    ]
}

/// An account-scoped selection of both action types carrying `criteria`.
fn preview_with_criteria(criteria: Value) -> Value {
    json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "criteria": criteria,
    })
}

/// One recommended action Compute Optimizer's analysis would have produced, in
/// the calling account. No operation of this API mints one, so the inventory a
/// preview reports over is seeded where the analysis leaves it.
fn seed_action(engine: &Arc<engine::InMemoryEngine>, id: &str, action_type: &str) {
    let action = RecommendedAction {
        recommended_action_id: Some(id.to_string()),
        resource_id: Some(format!("vol-{}", id.to_ascii_lowercase())),
        account_id: Some(state::caller_account_id()),
        region: Some("us-east-1".to_string()),
        resource_type: Some("EbsVolume".to_string()),
        recommended_action_type: Some(action_type.to_string()),
        ..Default::default()
    };
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    service_state
        .lock()
        .recommended_actions
        .insert(id.to_string(), action);
}

/// Both preview operations reach the SAME verdict on one request shape: the
/// same refusal, down to its message, or the same service of it.
#[tokio::test]
async fn both_preview_operations_judge_one_selection_the_same_way() {
    let (router, engine) = emulator_with_engine();
    seed_action(&engine, "RA00000000000001", UPGRADE);
    seed_action(&engine, "RA00000000000002", SNAPSHOT_AND_DELETE);

    for (case, request, verdict) in selection_cases() {
        let mut answers: Vec<(u16, String, String)> = Vec::new();
        for operation in PREVIEW_OPERATIONS {
            let (status, body) = call(&router, operation, &request).await;
            let case = format!("{case} via {operation}");
            match &verdict {
                Verdict::Served => assert_eq!(
                    status, 200,
                    "case {case:?}: an evaluable selection must be served, got {status} and {body}"
                ),
                Verdict::Refused {
                    member,
                    requirement,
                } => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} and {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "InvalidParameterValueException",
                        "case {case:?}: expected InvalidParameterValueException, got {body}"
                    );
                    let expected = constraint_report(member, requirement);
                    assert_eq!(
                        error_message(&body),
                        expected,
                        "case {case:?}: expected message {expected:?}, got {:?}",
                        error_message(&body)
                    );
                }
            }
            answers.push((status, error_code(&body), error_message(&body)));
        }
        let [first, second] = [&answers[0], &answers[1]];
        assert_eq!(
            first, second,
            "case {case:?}: both preview operations must judge one selection alike, got \
             {first:?} from {} and {second:?} from {}",
            PREVIEW_OPERATIONS[0], PREVIEW_OPERATIONS[1]
        );
    }
}

/// A preview page token carries the operation that issued it and the whole
/// selection it was taken with: each preview operation resumes its own token
/// and refuses the other's, with the report that names what a token has to
/// match.
#[tokio::test]
async fn a_preview_page_token_is_bound_to_its_operation_and_selection() {
    let (router, engine) = emulator_with_engine();
    seed_action(&engine, "RA00000000000011", UPGRADE);
    seed_action(&engine, "RA00000000000012", SNAPSHOT_AND_DELETE);

    let selection = json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "maxResults": 1,
    });
    // The unsummarised preview pages two selected actions at one action, so its
    // first page mints a token. The summarising one reports a single
    // account-wide aggregate row, so a page is never cut and what its walk can
    // carry is a cursor naming the END of that one-row sequence — built here
    // from the service's own fingerprint of the very same selection, so it is a
    // cursor that listing could have issued.
    let both_types = vec![UPGRADE.to_string(), SNAPSHOT_AND_DELETE.to_string()];
    let mut tokens: Vec<(&str, String)> = Vec::new();
    for operation in PREVIEW_OPERATIONS {
        if operation == PREVIEW_OPERATIONS[1] {
            tokens.push((
                operation,
                state::encode_page_token(&state::PageCursor {
                    offset: 1,
                    filter_fingerprint: state::preview_selection_fingerprint(
                        operation,
                        &state::preview_account_scope("AccountRule", None),
                        &both_types,
                        &Criteria::default(),
                    ),
                }),
            ));
            continue;
        }
        let first = call_ok(&router, operation, &selection, operation).await;
        let token = member(&first, "nextToken")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {operation:?}: the first page must carry a token, got {first}")
            })
            .to_string();
        tokens.push((operation, token));
    }

    for (issuer, token) in &tokens {
        let mut resumed = selection.clone();
        resumed["nextToken"] = json!(token);

        // Its own operation resumes the walk.
        let (status, body) = call(&router, issuer, &resumed).await;
        assert_eq!(
            status, 200,
            "case {issuer:?}: the issuing operation must resume its own token, got {status} and \
             {body}"
        );

        // The sibling operation refuses it, and so does the issuer under a
        // selection the token was not taken with.
        let other = if *issuer == PREVIEW_OPERATIONS[0] {
            PREVIEW_OPERATIONS[1]
        } else {
            PREVIEW_OPERATIONS[0]
        };
        let mut narrowed = resumed.clone();
        narrowed["recommendedActionTypes"] = json!([UPGRADE]);
        let elsewhere: Vec<(String, &str, Value)> = vec![
            (
                format!("a {issuer} token presented to {other}"),
                other,
                resumed.clone(),
            ),
            (
                format!("a {issuer} token presented with a narrower selection"),
                *issuer,
                narrowed,
            ),
        ];
        for (case, operation, request) in elsewhere {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} and {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            let expected = constraint_report(
                "nextToken",
                "Member must be a token returned for the same rule type, action types and criteria",
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected message {expected:?}, got {:?}",
                error_message(&body)
            );
        }
    }
}
