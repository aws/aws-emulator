// SPDX-License-Identifier: CC0-1.0

//! How a simulation refuses the document supplied in `ResourcePolicy`, end to end over
//! the awsQuery wire.
//!
//! The rule under test: `ResourcePolicy` takes a policy of ONE CLASS — "a resource-based
//! policy to include in the simulation provided as a string", one that can be attached to
//! a resource — so a document the simulator cannot use as one is refused
//! `InvalidInput` with AWS's own fixed sentence for that class, `Invalid Resource
//! Policy.`, whatever is wrong with it: text that is no JSON object, an object describing
//! no statement, and a policy of another class (an identity-based policy, whose statements
//! name no principal). The identity-side members keep their own refusals — the
//! parse-failure sentence for a simulated permissions boundary and the positional sentence
//! for an entry of `PolicyInputList` — because the sentence is the only thing that tells
//! the classes apart behind the one error code all of them are refused with.
//!
//! Both operations that take such a document are driven here (`SimulatePrincipalPolicy`,
//! `SimulateCustomPolicy`), and the rows span the whole class the rule generalises over:
//! the JSON shape of the document, how its statements are written (a list, a single
//! object), which element they are short of, which spelling names the principal, and the
//! version the document declares — one row per clause of the grammar the shared reader
//! states (`service_common::iam_policy::resource_policy_defect`), which is what keeps this
//! service's reading of a resource-based document the same as every other service's. A
//! usable document is DECIDED rather than refused, which is what keeps the refusal from
//! swallowing the documents the member exists for.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, and the request
//! builder plus the decision reader are `tests/all/policy_simulation.rs`'s, so a
//! simulation request is built exactly one way.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_simulation::{
    principal, reported, simulate, ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY,
};
use crate::user_lifecycle::{call, element, error_code, router};

/// AWS's fixed sentence for a document supplied where a resource-based policy belongs that
/// the simulator cannot use as one (`crate::state::INVALID_RESOURCE_POLICY`).
const INVALID_RESOURCE_POLICY: &str = "Invalid Resource Policy.";

/// AWS's fixed sentence for an IDENTITY-side document that is no policy
/// (`crate::state::SYNTAX_ERRORS_IN_POLICY`) — the one this suite pins is NOT answered for
/// the resource-based member.
const SYNTAX_ERRORS_IN_POLICY: &str = "Syntax errors in policy.";

/// The action every row asks about, and the one resource it is asked on: a resource-based
/// policy is attached to a resource, so each resource the request names has to name one.
const ACTION: &str = "s3:GetObject";
const ONE_RESOURCE: &str = "arn:aws:s3:::reports/q1.csv";

/// The identity-based document the custom simulation supplies: it states nothing about the
/// action under test, so what either operation decides comes from the resource-based policy
/// alone and the two agree.
const STATES_NOTHING: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#;

/// An account that is not this emulator's, for the row whose statement grants a principal
/// the simulation is not of.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// One `SimulateCustomPolicy` call carrying `extra`, with the identity-based document, the
/// action, the resource and the simulated caller every row shares.
async fn simulate_custom(
    router: &MultiServiceRouter,
    caller_arn: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut owned: Vec<(String, String)> = vec![
        (
            "PolicyInputList.member.1".to_string(),
            STATES_NOTHING.to_string(),
        ),
        ("ActionNames.member.1".to_string(), ACTION.to_string()),
        ("ResourceArns.member.1".to_string(), ONE_RESOURCE.to_string()),
        ("CallerArn".to_string(), caller_arn.to_string()),
    ];
    owned.extend(extra.iter().cloned());
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call(router, "SimulateCustomPolicy", &params).await
}

/// One form parameter, owned.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// RULE: a document supplied in `ResourcePolicy` that the simulator cannot use as a
/// resource-based policy is refused HTTP 400 `InvalidInput` with the fixed sentence
/// `Invalid Resource Policy.` — on either simulation operation, whatever is wrong with the
/// document.
#[tokio::test]
async fn a_resource_policy_the_simulator_cannot_use_is_refused_with_one_sentence() {
    let router = router();
    let caller = principal(&router, "resource policy refusals", "unusable", &[]).await;

    // The rows span the whole class: text that is no JSON object at all (not JSON, an
    // unterminated object, a list, a number, a string), an object that describes no
    // statement (empty, every top-level element but `Statement`, the element in another
    // case, an empty statement list, a statement that is not an object), a policy of
    // ANOTHER class — statements that name no principal, in either spelling and in either
    // of the two forms the grammar admits for `Statement` — and a statement or a document
    // short of an element that grammar requires of any policy (an effect, an action, a
    // version that exists).
    let unusable: &[(&str, String)] = &[
        ("not JSON at all", "trust me".to_string()),
        (
            "an unterminated object",
            r#"{"Version":"2012-10-17""#.to_string(),
        ),
        ("a JSON list", r#"[{"Effect":"Allow"}]"#.to_string()),
        ("a JSON number", "17".to_string()),
        ("a JSON string", r#""Statement""#.to_string()),
        ("an empty object", "{}".to_string()),
        (
            "every top-level element but Statement",
            r#"{"Version":"2012-10-17","Id":"a-policy"}"#.to_string(),
        ),
        (
            "the statement element in another case",
            r#"{"Version":"2012-10-17","statement":[]}"#.to_string(),
        ),
        (
            "an empty statement list",
            r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
        ),
        (
            "a statement that is not an object",
            r#"{"Version":"2012-10-17","Statement":["Allow"]}"#.to_string(),
        ),
        (
            "an identity-style statement, naming no principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a single statement object naming no principal",
            r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}}"#
                .to_string(),
        ),
        (
            "one of two statements naming no principal",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{caller}"}},"Action":"{ACTION}","Resource":"*"}},{{"Effect":"Allow","Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
        ),
        (
            "a statement whose principal is null",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":null,"Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
        ),
        (
            "a statement whose principal names nothing",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":[],"Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
        ),
        (
            "a statement declaring no effect",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Principal":{{"AWS":"{caller}"}},"Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
        ),
        (
            "a statement naming no action",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{caller}"}},"Resource":"*"}}]}}"#
            ),
        ),
        (
            "a policy version that does not exist",
            format!(
                r#"{{"Version":"2012-10-99","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{caller}"}},"Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
        ),
    ];

    for (what, document) in unusable {
        let extra = vec![param("ResourcePolicy", document)];
        for operation in ["SimulatePrincipalPolicy", "SimulateCustomPolicy"] {
            let case = format!("{operation} with {what}");
            let (status, body) = match operation {
                "SimulatePrincipalPolicy" => {
                    simulate(&router, &caller, &[ACTION], &[ONE_RESOURCE], &extra).await
                }
                _ => simulate_custom(&router, &caller, &extra).await,
            };
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?}: expected InvalidInput, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, INVALID_RESOURCE_POLICY,
                "case {case:?}: expected AWS's resource-policy sentence, got {message:?}"
            );
        }
    }
}

/// RULE (the other half): a document the simulator CAN use as a resource-based policy is
/// decided rather than refused — whichever of the two forms `Statement` takes and whichever
/// spelling names the principal — and the decision is the one that document states about
/// the simulated caller.
#[tokio::test]
async fn a_usable_resource_policy_is_decided_rather_than_refused() {
    let router = router();
    let caller = principal(&router, "resource policy decisions", "usable", &[]).await;
    let other = format!("arn:aws:iam::{ANOTHER_ACCOUNT}:root");

    // The rows differ along the form of the document and what it says about the caller: a
    // statement list and a single statement object, the principal named by ARN and by the
    // wildcard, a principal that is not the caller, and the deny side.
    let usable: &[(&str, String, &str)] = &[
        (
            "a bucket-style statement granting the caller",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"grant","Effect":"Allow","Principal":{{"AWS":"{caller}"}},"Action":"{ACTION}","Resource":"{ONE_RESOURCE}"}}]}}"#
            ),
            ALLOWED,
        ),
        (
            "a single statement object granting the caller",
            format!(
                r#"{{"Version":"2012-10-17","Statement":{{"Effect":"Allow","Principal":{{"AWS":"{caller}"}},"Action":"{ACTION}","Resource":"*"}}}}"#
            ),
            ALLOWED,
        ),
        (
            "a statement granting every principal",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":"*","Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
            ALLOWED,
        ),
        (
            "a statement granting another account's principal",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{other}"}},"Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
            IMPLICIT_DENY,
        ),
        (
            "a statement denying the caller",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Principal":{{"AWS":"{caller}"}},"Action":"{ACTION}","Resource":"*"}}]}}"#
            ),
            EXPLICIT_DENY,
        ),
    ];

    for (what, document, expected) in usable {
        let extra = vec![param("ResourcePolicy", document)];
        for operation in ["SimulatePrincipalPolicy", "SimulateCustomPolicy"] {
            let case = format!("{operation} with {what}");
            let (status, body) = match operation {
                "SimulatePrincipalPolicy" => {
                    simulate(&router, &caller, &[ACTION], &[ONE_RESOURCE], &extra).await
                }
                _ => simulate_custom(&router, &caller, &extra).await,
            };
            assert_eq!(
                status, 200,
                "case {case:?}: a usable resource policy must be decided, got {status}: {body}"
            );
            assert_eq!(
                reported(&body),
                vec![(ACTION.to_string(), (*expected).to_string())],
                "case {case:?}: expected the decision the document states, got {body}"
            );
        }
    }
}

/// RULE (the separation): the IDENTITY-side documents a simulation takes keep their own
/// refusals — a simulated permissions boundary that is no policy is still the
/// parse-failure sentence, and an entry of `PolicyInputList` is still refused by its
/// position — so the resource-policy sentence is answered for the resource-based member
/// alone.
#[tokio::test]
async fn the_identity_side_documents_keep_their_own_refusals() {
    let router = router();
    let caller = principal(&router, "identity-side refusals", "identity-side", &[]).await;

    // The same unusable document in each member, so what differs between the rows is the
    // member it was supplied in and nothing else.
    let rows: &[(&str, (String, String), &str)] = &[
        (
            "a simulated permissions boundary that is no policy",
            param("PermissionsBoundaryPolicyInputList.member.1", "{}"),
            SYNTAX_ERRORS_IN_POLICY,
        ),
        (
            "an entry of PolicyInputList that is no policy",
            param("PolicyInputList.member.1", "{}"),
            "Policy input list item 1 has invalid content",
        ),
        (
            "a second entry of PolicyInputList that is no policy",
            param("PolicyInputList.member.2", "{}"),
            "Policy input list item 2 has invalid content",
        ),
    ];

    for (what, supplied, expected) in rows {
        let case = format!("SimulatePrincipalPolicy with {what}");
        let mut extra = vec![supplied.clone()];
        if supplied.0 == "PolicyInputList.member.2" {
            extra.push(param("PolicyInputList.member.1", STATES_NOTHING));
        }
        let (status, body) = simulate(&router, &caller, &[ACTION], &[ONE_RESOURCE], &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, *expected,
            "case {case:?}: expected that member's own sentence, got {message:?}"
        );
    }
}
