// SPDX-License-Identifier: CC0-1.0

//! The ONE sentence this service states an input-validation refusal in, driven over HTTP
//! through the same `MultiServiceRouter` the emulator serves with, down BOTH paths that
//! can reach it.
//!
//! A refusal's words are built the same way whichever path raised it: `<N> validation
//! error(s) detected: ` over one `<subject> at '<member path>' failed to satisfy
//! constraint: <requirement>` clause per offending value, joined with `; `. Two paths
//! reach that sentence:
//!
//! * the framework's own boundary walk, which collects what the model's constraint traits
//!   declare and hands the violations to this service to word
//!   (`src/service/validation.rs`);
//! * a HANDLER refusing a condition the model cannot declare —
//!   `BatchUpdateRecommendationResourceExclusion`'s repeated-ARN check, which the modelled
//!   `@uniqueItems` cannot express because it compares whole
//!   `RecommendationResourceExclusion` structures.
//!
//! Each row below is refused down one of those two paths and its message is parsed back
//! through the SAME reader: the envelope's count and singular/plural form, then the clause
//! skeleton. A path that worded either half itself would have to agree byte for byte to
//! pass, so this suite is what pins that the two share one builder.
//!
//! The handler-raised refusal of a `nextToken` that is not a cursor the read it was sent
//! to issued goes through the SAME two readers below, from
//! `tests/all/page_token_refusal_sentence.rs`, which is where its rows live because they
//! span every paginated operation rather than one.
//!
//! The rows differ along the path that words them, the operation, the number of clauses
//! (one, two and three on each path), the subject form (a value echoed, and the null an
//! absent member was) and the requirement class (an enum value set, a range bound, a
//! length bound, a `@required` member and the handler's own uniqueness rule).
//!
//! Every status, error code and modelled constraint asserted here comes from the model:
//! `smithy_query(query="shape",shape="ValidationException",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — `@error("client")`, `@httpError(400)`, one `@required` member `message` — and
//! `smithy_query(query="operation",operation="ListChecks")` /
//! `smithy_query(query="operation",operation="BatchUpdateRecommendationResourceExclusion")`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{dispatch, emulator_on_plan, error_code, resource_arn};

/// The path `BatchUpdateRecommendationResourceExclusion` binds
/// (`PUT /v1/batch-update-recommendation-resource-exclusion`).
const BATCH_PATH: &str = "/v1/batch-update-recommendation-resource-exclusion";

/// The member path the handler's repeated-ARN clause names, and the requirement it
/// states — unchanged by which module words the sentence around them.
const EXCLUSIONS_MEMBER: &str = "recommendationResourceExclusions";
const UNIQUE_ARNS_REQUIREMENT: &str = "Member must have unique arn values";

/// `RecommendationPillar`'s value set as this service reports it, from
/// `tests/all/validation_message_rendering.rs`'s recorded order (the SET is the model's,
/// `smithy_query(query="shape",shape="RecommendationPillar")`).
const PILLAR_REQUIREMENT: &str = "Member must satisfy enum value set: [security, \
                                  service_limits, cost_optimizing, operational_excellence, \
                                  fault_tolerance, performance]";

/// `ListChecks$maxResults` declares `@range(1, 200)`, so `0` breaks the minimum.
const MAX_RESULTS_REQUIREMENT: &str = "Member must have value greater than or equal to 1";

/// `ListChecks$awsService` declares `@length(2, 30)`, so a one-character value breaks the
/// minimum.
const AWS_SERVICE_REQUIREMENT: &str = "Member must have length greater than or equal to 2";

/// Which of the two paths a row is refused down. Both must state the one sentence; the
/// field is what each assertion names, so a failure says which path disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// The framework's constraint walk collected it and this service worded it.
    FrameworkCollected,
    /// The handler refused a condition the model's constraint traits cannot express.
    HandlerRaised,
}

/// One clause, taken apart into the three things the skeleton states.
pub(crate) type Clause = (String, String, String);

/// One row: what it is about, which path words it, the request to make, and the clauses
/// the refusal must carry — compared as a SET, because the clause ORDER of a
/// framework-collected refusal is the boundary walk's and is not part of this rule. The
/// handler-raised path does state its clauses in the order in which each ARN first
/// repeats, and that order is pinned by `tests/all/batch_exclusion_unique_arns.rs`.
struct Row {
    name: String,
    path: Path,
    method: &'static str,
    path_and_query: String,
    body: Option<Value>,
    clauses: Vec<Clause>,
}

fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// A clause stating that `arn` was named more than once.
fn repeated_arn_clause(arn: &str) -> Clause {
    (
        format!("Value '{arn}'"),
        EXCLUSIONS_MEMBER.to_string(),
        UNIQUE_ARNS_REQUIREMENT.to_string(),
    )
}

/// The request body for a list of `(arn, isExcluded)` entries.
fn batch_body(entries: &[(String, bool)]) -> Value {
    json!({
        "recommendationResourceExclusions": entries
            .iter()
            .map(|(arn, is_excluded)| json!({"arn": arn, "isExcluded": is_excluded}))
            .collect::<Vec<Value>>(),
    })
}

/// The clauses a refusal `message` carries, read back out of the envelope: the count it
/// declares in the form the count calls for, then the clauses it joined with `; `.
///
/// Every row goes through this one reader, so a path wording its own envelope would have
/// to word it identically to get here at all.
pub(crate) fn clauses_of(case: &str, message: &str) -> Vec<String> {
    if let Some(only) = message.strip_prefix("1 validation error detected: ") {
        assert!(
            !only.contains("; "),
            "case {case}: one clause is stated in the singular envelope with nothing \
             joined to it, got {message:?}"
        );
        return vec![only.to_string()];
    }
    let (count, joined) = message
        .split_once(" validation errors detected: ")
        .unwrap_or_else(|| {
            panic!("case {case}: expected the counted refusal envelope, got {message:?}")
        });
    let declared: usize = count.parse().unwrap_or_else(|_| {
        panic!("case {case}: expected a clause count before the envelope, got {message:?}")
    });
    let clauses: Vec<String> = joined.split("; ").map(str::to_string).collect();
    assert_eq!(
        declared,
        clauses.len(),
        "case {case}: the envelope declares {declared} clauses but joins {}, in {message:?}",
        clauses.len()
    );
    assert!(
        declared > 1,
        "case {case}: a count of {declared} is stated in the singular envelope, got \
         {message:?}"
    );
    clauses
}

/// One clause taken apart into its subject, the member path it names and the requirement
/// it states. Parsing is the skeleton assertion: a clause spelled any other way has no
/// three parts to read.
pub(crate) fn clause_parts(case: &str, clause: &str) -> Clause {
    let (subject, rest) = clause.split_once(" at '").unwrap_or_else(|| {
        panic!("case {case}: expected a clause naming the member it is about, got {clause:?}")
    });
    let (member_path, requirement) = rest
        .split_once("' failed to satisfy constraint: ")
        .unwrap_or_else(|| {
            panic!("case {case}: expected a clause stating the requirement, got {clause:?}")
        });
    (
        subject.to_string(),
        member_path.to_string(),
        requirement.to_string(),
    )
}

#[tokio::test]
async fn both_refusal_paths_state_the_one_sentence() {
    let router = emulator();

    let first = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");
    let second = resource_arn("amazon-ebs-snapshots", "res-ebs-0002");
    let third = resource_arn("iam-access-key-rotation", "res-iam-0001");

    let rows: Vec<Row> = vec![
        // ---- the handler's own condition, at one, two and three clauses ----
        Row {
            name: "one ARN repeated".to_string(),
            path: Path::HandlerRaised,
            method: "PUT",
            path_and_query: BATCH_PATH.to_string(),
            body: Some(batch_body(&[(first.clone(), true), (first.clone(), false)])),
            clauses: vec![repeated_arn_clause(&first)],
        },
        Row {
            name: "two ARNs repeated".to_string(),
            path: Path::HandlerRaised,
            method: "PUT",
            path_and_query: BATCH_PATH.to_string(),
            body: Some(batch_body(&[
                (first.clone(), true),
                (second.clone(), true),
                (first.clone(), false),
                (second.clone(), false),
            ])),
            clauses: vec![repeated_arn_clause(&first), repeated_arn_clause(&second)],
        },
        Row {
            name: "three ARNs repeated".to_string(),
            path: Path::HandlerRaised,
            method: "PUT",
            path_and_query: BATCH_PATH.to_string(),
            body: Some(batch_body(&[
                (first.clone(), true),
                (second.clone(), true),
                (third.clone(), true),
                (third.clone(), false),
                (second.clone(), false),
                (first.clone(), false),
            ])),
            clauses: vec![
                repeated_arn_clause(&first),
                repeated_arn_clause(&second),
                repeated_arn_clause(&third),
            ],
        },
        // ---- the framework's violations, at one, two and three clauses ----
        Row {
            name: "a pillar outside its enum".to_string(),
            path: Path::FrameworkCollected,
            method: "GET",
            path_and_query: "/v1/checks?pillar=not_a_real_pillar".to_string(),
            body: None,
            clauses: vec![(
                "Value 'not_a_real_pillar'".to_string(),
                "pillar".to_string(),
                PILLAR_REQUIREMENT.to_string(),
            )],
        },
        Row {
            name: "a pillar outside its enum and a maxResults below its range".to_string(),
            path: Path::FrameworkCollected,
            method: "GET",
            path_and_query: "/v1/checks?pillar=not_a_real_pillar&maxResults=0".to_string(),
            body: None,
            clauses: vec![
                (
                    "Value 'not_a_real_pillar'".to_string(),
                    "pillar".to_string(),
                    PILLAR_REQUIREMENT.to_string(),
                ),
                (
                    "Value '0'".to_string(),
                    "maxResults".to_string(),
                    MAX_RESULTS_REQUIREMENT.to_string(),
                ),
            ],
        },
        Row {
            name: "three members of one request broken at once".to_string(),
            path: Path::FrameworkCollected,
            method: "GET",
            path_and_query: "/v1/checks?pillar=not_a_real_pillar&maxResults=0&awsService=s"
                .to_string(),
            body: None,
            clauses: vec![
                (
                    "Value 'not_a_real_pillar'".to_string(),
                    "pillar".to_string(),
                    PILLAR_REQUIREMENT.to_string(),
                ),
                (
                    "Value '0'".to_string(),
                    "maxResults".to_string(),
                    MAX_RESULTS_REQUIREMENT.to_string(),
                ),
                (
                    "Value 's'".to_string(),
                    "awsService".to_string(),
                    AWS_SERVICE_REQUIREMENT.to_string(),
                ),
            ],
        },
        // The SAME operation the handler refuses above, refused down the framework's path
        // instead: one sentence, and a subject that states the null an absent member was.
        Row {
            name: "the batch operation with its required list absent".to_string(),
            path: Path::FrameworkCollected,
            method: "PUT",
            path_and_query: BATCH_PATH.to_string(),
            body: Some(json!({})),
            clauses: vec![(
                "Value null".to_string(),
                EXCLUSIONS_MEMBER.to_string(),
                "Member must not be null".to_string(),
            )],
        },
    ];

    for Row {
        name,
        path,
        method,
        path_and_query,
        body,
        clauses,
    } in rows
    {
        let case = format!("{name} ({path:?})");
        let (status, errortype, response) =
            dispatch(&router, method, &path_and_query, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected ValidationException, got {response}"
        );
        assert!(
            errortype
                .as_deref()
                .is_some_and(|header| header.starts_with("ValidationException")),
            "case {case}: expected the x-amzn-errortype header to resolve \
             ValidationException, got {errortype:?}"
        );
        let message = response["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the refusal carries no message: {response}"));

        let mut actual: Vec<Clause> = clauses_of(&case, message)
            .iter()
            .map(|clause| clause_parts(&case, clause))
            .collect();
        let mut expected = clauses;
        actual.sort();
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {case}: expected the clauses {expected:?}, got {actual:?} from {message:?}"
        );
    }
}
