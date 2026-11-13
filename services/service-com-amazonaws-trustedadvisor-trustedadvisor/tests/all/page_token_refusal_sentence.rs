// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE every paginated read of this service states when its `nextToken` is not a
//! cursor that read issued, driven over HTTP through the same `MultiServiceRouter` the
//! emulator serves with.
//!
//! The rule is not about one operation and not about one token: **a refusal this service
//! raises itself is worded through the crate's one refusal-sentence builder — the
//! `<N> validation error(s) detected: ` envelope over
//! `<subject> at '<member path>' failed to satisfy constraint: <requirement>` clauses — so
//! a `nextToken` outside the cursors a read issued is refused with ONE clause, about
//! `nextToken`, echoing the value the request held, from every paginated operation
//! alike.** The condition stays with the handlers (the model's constraint traits say how
//! long a `nextToken` may be, not which values are cursors); only the words are shared.
//!
//! So the rows below span the class along three axes rather than repeating one call:
//!
//! * the OPERATION — all seven paginated reads this service binds
//!   (`smithy_query(query="operation",operation="ListChecks,ListRecommendations,
//!   ListRecommendationResources,ListOrganizationRecommendations,
//!   ListOrganizationRecommendationAccounts,ListOrganizationRecommendationResources,
//!   ListRecommendationsForResource",
//!   model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//!   lists every one as `@paginated(inputToken: nextToken)`, each binding
//!   `ValidationException` at HTTP 400);
//! * the way the VALUE fails to be a cursor — outside the base64 alphabet, base64 of bytes
//!   that are not text, base64 of text that carries no cursor at all, a cursor another list
//!   issued, and a cursor the same list issued under filters it was not issued under;
//! * the SIZE of the echoed value, up to a value far longer than the clause's own skeleton,
//!   inside the `@length(4, 10000)` the model puts on `nextToken` (a longer one is the
//!   framework's refusal, not a handler's).
//!
//! Every message is read back through the SAME two readers
//! `tests/all/refusal_sentence_one_builder.rs` parses the other two refusal paths with, so
//! a handler wording its own envelope or its own clause skeleton has nothing to parse.

use serde_json::json;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::organization_recommendation_arn;
use crate::recommendation_loop::{
    call, emulator_on_plan, error_code, label, recommendation_arn, PRIORITIZED_CHECK_IDS,
};
use crate::refusal_sentence_one_builder::{clause_parts, clauses_of, Clause};

/// The member path every page-token clause names — `nextToken` is the `inputToken` of the
/// `@paginated` trait on all seven operations.
const TOKEN_MEMBER: &str = "nextToken";

/// What that member had to satisfy, in the requirement form this service states every
/// clause's rule half in.
const TOKEN_REQUIREMENT: &str =
    "Member must be a pagination token this service issued for this list";

/// The clause a refused `nextToken` earns: the value the request held, the member it was
/// at, and the requirement it failed.
fn token_clause(token: &str) -> Clause {
    (
        format!("Value '{token}'"),
        TOKEN_MEMBER.to_string(),
        TOKEN_REQUIREMENT.to_string(),
    )
}

/// A mounted emulator on a support plan that includes the Trusted Advisor API, so every
/// request below reaches its handler (the gate itself is pinned in
/// `tests/all/support_level_gate.rs`).
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// The seven paginated reads, as (operation, the URI with no query string). The path
/// labels name SEEDED resources, so each row reaches the pagination step rather than a
/// not-found.
fn paginated_reads() -> Vec<(&'static str, String)> {
    let recommendation = recommendation_arn("amazon-ebs-snapshots");
    // Only a prioritized check has an organization recommendation (pinned in
    // `tests/all/organization_prioritized_only.rs`).
    let organization = organization_recommendation_arn(PRIORITIZED_CHECK_IDS[0]);
    vec![
        ("ListChecks", "/v1/checks".to_string()),
        ("ListRecommendations", "/v1/recommendations".to_string()),
        (
            "ListRecommendationResources",
            format!("/v1/recommendations/{}/resources", label(&recommendation)),
        ),
        (
            "ListOrganizationRecommendations",
            "/v1/organization-recommendations".to_string(),
        ),
        (
            "ListOrganizationRecommendationAccounts",
            format!(
                "/v1/organization-recommendations/{}/accounts",
                label(&organization)
            ),
        ),
        (
            "ListOrganizationRecommendationResources",
            format!(
                "/v1/organization-recommendations/{}/resources",
                label(&organization)
            ),
        ),
        (
            "ListRecommendationsForResource",
            format!("/v1/recommendations-for-resource/{}", label(&seeded_ec2())),
        ),
    ]
}

/// An `awsResourceArn` the modelled pattern admits
/// (`^arn:aws(-\w+)*:[\w\d-]+:([\w\d-]*)?:[\w\d_-]*([:/].+)*$`, len 20..2048), in the
/// emulator's own account.
fn seeded_ec2() -> String {
    format!(
        "arn:aws:ec2:us-east-1:{}:instance/i-0abc1de23f4567890",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Drive one refused read and read the clauses its message carries back through the shared
/// readers: the envelope first, then each clause's three parts.
async fn refused_clauses(
    router: &MultiServiceRouter,
    case: &str,
    path_and_query: &str,
) -> Vec<Clause> {
    let (status, body) = call(router, "GET", path_and_query, None).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the refusal carries no message: {body}"))
        .to_string();
    clauses_of(case, &message)
        .iter()
        .map(|clause| clause_parts(case, clause))
        .collect()
}

/// A token every read will reject: base64 of `bytes`, which carries no cursor.
fn base64_of(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}

#[tokio::test]
async fn every_paginated_read_words_the_page_token_refusal_the_same_way() {
    let router = emulator();

    // A cursor each of two lists really issued, so a row can replay one list's cursor on
    // another — the only failure mode that needs a token this service DID issue.
    let (_, checks_page) = call(&router, "GET", "/v1/checks?maxResults=1", None).await;
    let checks_cursor = checks_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated ListChecks page")
        .to_string();
    let (_, recommendations_page) =
        call(&router, "GET", "/v1/recommendations?maxResults=1", None).await;
    let recommendations_cursor = recommendations_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated ListRecommendations page")
        .to_string();

    // Each row: what is wrong with the value, and the value. Every one satisfies the
    // modelled `@length(4, 10000)`, so the refusal reached is the handler's own.
    let tokens: Vec<(&str, String)> = vec![
        (
            "four characters outside the base64 alphabet",
            "....".to_string(),
        ),
        (
            "base64 of bytes that are not text",
            base64_of(&[0xff, 0xfe, 0xfd]),
        ),
        (
            "base64 of text carrying no cursor",
            base64_of(b"not-a-cursor"),
        ),
        (
            "a value far longer than the clause that echoes it",
            "A".repeat(1_000),
        ),
        ("a cursor ListChecks issued", checks_cursor.clone()),
        (
            "a cursor ListRecommendations issued",
            recommendations_cursor.clone(),
        ),
    ];

    for (operation, base) in paginated_reads() {
        for (what, token) in &tokens {
            // A list's OWN cursor is a cursor, not a refusal: the two replay rows are
            // skipped on the list that issued the token, which
            // `tests/all/recommendation_loop.rs` pins is accepted there.
            if (operation == "ListChecks" && token == &checks_cursor)
                || (operation == "ListRecommendations" && token == &recommendations_cursor)
            {
                continue;
            }
            let case = format!("{operation} with {what}");
            let path = format!("{base}?nextToken={}", label(token));
            let clauses = refused_clauses(&router, &case, &path).await;
            assert_eq!(
                clauses,
                vec![token_clause(token)],
                "case {case}: expected one clause about {TOKEN_MEMBER}, got {clauses:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_from_another_filter_set_is_refused_with_the_same_sentence() {
    let router = emulator();

    // Each row: the read that ISSUES the cursor, and the read it is replayed on — the same
    // operation and the same parent, under a filter set it was not issued under, which is a
    // different sequence and so not a cursor of the read it reaches.
    let recommendation = recommendation_arn("amazon-ebs-snapshots");
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a filtered ListChecks cursor on the unfiltered list",
            "/v1/checks?pillar=cost_optimizing&maxResults=1".to_string(),
            "/v1/checks".to_string(),
        ),
        (
            "an unfiltered ListRecommendations cursor on a filtered list",
            "/v1/recommendations?maxResults=1".to_string(),
            "/v1/recommendations?pillar=cost_optimizing".to_string(),
        ),
        (
            "an unfiltered resources cursor on a Region-filtered list",
            format!(
                "/v1/recommendations/{}/resources?maxResults=1",
                label(&recommendation)
            ),
            format!(
                "/v1/recommendations/{}/resources?regionCode=us-east-1",
                label(&recommendation)
            ),
        ),
    ];

    for (name, issuing, replay) in rows {
        let (status, page) = call(&router, "GET", &issuing, None).await;
        assert_eq!(
            status, 200,
            "case {name}: expected the issuing read to answer 200, got {status} with {page}"
        );
        let cursor = page["nextToken"].as_str().unwrap_or_else(|| {
            panic!("case {name}: expected a nextToken on the truncated page, got {page}")
        });
        let separator = if replay.contains('?') { '&' } else { '?' };
        let path = format!("{replay}{separator}nextToken={}", label(cursor));
        let clauses = refused_clauses(&router, name, &path).await;
        assert_eq!(
            clauses,
            vec![token_clause(cursor)],
            "case {name}: expected one clause about {TOKEN_MEMBER}, got {clauses:?}"
        );
    }
}

#[tokio::test]
async fn both_conditions_this_service_raises_itself_state_one_clause_skeleton() {
    let router = emulator();

    // The page-token condition, and the repeated-ARN condition of
    // `BatchUpdateRecommendationResourceExclusion` — the two refusals no constraint trait
    // of the model can express. Each is read back through the shared readers, so the
    // subject form, the member path and the requirement are all that may differ.
    let token_clauses = refused_clauses(
        &router,
        "a page token that is not a cursor",
        "/v1/checks?nextToken=....",
    )
    .await;

    let arn = format!(
        "arn:aws:trustedadvisor::{}:recommendation-resource/amazon-ebs-snapshots-recommendation/res-ebs-0001",
        engine::EMULATOR_ACCOUNT_ID
    );
    let (status, body) = call(
        &router,
        "PUT",
        "/v1/batch-update-recommendation-resource-exclusion",
        Some(json!({
            "recommendationResourceExclusions": [
                {"arn": arn, "isExcluded": true},
                {"arn": arn, "isExcluded": false},
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a repeated ARN: expected HTTP 400, got {status} with {body}"
    );
    let batch_message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case a repeated ARN: the refusal carries no message: {body}"));
    let batch_clauses: Vec<Clause> = clauses_of("a repeated ARN", batch_message)
        .iter()
        .map(|clause| clause_parts("a repeated ARN", clause))
        .collect();

    // Both state exactly one clause, and both echo the offending value the same way, which
    // is the half a handler could have worded for itself.
    for (case, clauses, expected_value) in [
        ("a page token that is not a cursor", &token_clauses, "...."),
        ("a repeated ARN", &batch_clauses, arn.as_str()),
    ] {
        assert_eq!(
            clauses.len(),
            1,
            "case {case}: expected one clause, got {clauses:?}"
        );
        assert_eq!(
            clauses[0].0,
            format!("Value '{expected_value}'"),
            "case {case}: expected the offending value echoed, got {:?}",
            clauses[0].0
        );
    }
    assert_eq!(
        token_clauses[0].1, TOKEN_MEMBER,
        "case a page token that is not a cursor: expected the clause at {TOKEN_MEMBER}, got {:?}",
        token_clauses[0].1
    );
    assert_eq!(
        batch_clauses[0].1, "recommendationResourceExclusions",
        "case a repeated ARN: expected the clause at recommendationResourceExclusions, got {:?}",
        batch_clauses[0].1
    );
}

#[tokio::test]
async fn a_token_outside_the_modelled_length_is_the_frameworks_refusal_not_this_one() {
    let router = emulator();

    // The boundary between the two paths: `nextToken` is `@length(4, 10000)`, so a value
    // shorter than four characters is collected by the framework's own constraint walk and
    // carries that requirement, while a four-character one reaches the handler. Both are
    // worded by the one builder, which is what lets this row read them with the same
    // reader.
    let cases: &[(&str, &str, &str)] = &[
        (
            "three characters",
            "...",
            "Member must have length greater than or equal to 4",
        ),
        ("four characters", "....", TOKEN_REQUIREMENT),
    ];

    for (case, token, requirement) in cases {
        let clauses =
            refused_clauses(&router, case, &format!("/v1/checks?nextToken={token}")).await;
        assert_eq!(
            clauses.len(),
            1,
            "case {case}: expected one clause, got {clauses:?}"
        );
        assert_eq!(
            clauses[0].1, TOKEN_MEMBER,
            "case {case}: expected the clause at {TOKEN_MEMBER}, got {:?}",
            clauses[0].1
        );
        assert_eq!(
            clauses[0].2, *requirement,
            "case {case}: expected the requirement {requirement:?}, got {:?}",
            clauses[0].2
        );
    }
}
