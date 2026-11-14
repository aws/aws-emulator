// SPDX-License-Identifier: CC0-1.0

//! WHICH action a trust-policy refusal of an `AssumeRole` names, when the decision that
//! refused it turns on the SOURCE IDENTITY the mint would set on the new session.
//!
//! RULE: a `Condition` over `sts:SourceIdentity` in a role's trust policy gates the
//! source-identity SET rather than the assume — "you can use the `sts:SourceIdentity`
//! condition key in a role trust policy" (`AssumeRoleRequest$SourceIdentity`), and the
//! permission it narrows is the one an administrator grants beside the assume — so a mint
//! that would set a source identity and is refused by such a clause is refused
//! `AccessDenied` naming `sts:SetSourceIdentity` on the target role, whether the request
//! named the value or the calling session carried it forward ("the source identity value
//! persists across chained role sessions", same member). A refusal no clause over that key
//! decided keeps naming the action the caller asked to perform: `sts:AssumeRole` for a
//! chained mint that named no source identity of its own, and `sts:SetSourceIdentity` for a
//! request that did.
//!
//! The rows span what the rule generalises over: WHERE the clause sits (on the statement
//! admitting the assume, on a separate statement admitting the source-identity set, on
//! neither), WHETHER the carried value satisfies it, WHETHER the chained request names a
//! source identity of its own, and WHICH operator the clause is written under.

use super::sts_sessions::{call, element, emulator, emulator_account, member, seed_role};

/// The source identity the FIRST session of every chain here carries.
const CARRIED: &str = "sim2real-bob";

/// The value the target role's trust policy demands, which is not [`CARRIED`].
const DEMANDED: &str = "sim2real-alice";

/// The two actions a chained mint that sets a source identity performs on the target role.
const ASSUME: &str = "sts:AssumeRole";
const SET_SOURCE_IDENTITY: &str = "sts:SetSourceIdentity";

/// One trust-policy statement: `actions` granted to `principal`, under `condition` where a
/// row names one.
fn statement(principal: &str, actions: &[&str], condition: Option<&str>) -> String {
    let actions = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{principal}\"}},\
         \"Action\":[{actions}]{condition}}}"
    )
}

/// A trust policy holding `statements`.
fn policy(statements: &[String]) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        statements.join(",")
    )
}

/// The refusal STS words for a caller that may not perform `action` on `resource`.
fn not_authorized(principal: &str, action: &str, resource: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}")
}

/// RULE: see the module docs — the action a chained assume's refusal names is the
/// source-identity SET exactly where a `sts:SourceIdentity` clause of the target's trust
/// policy decided it, or where the request asked to set the value itself.
#[tokio::test]
async fn a_source_identity_condition_names_the_set_source_identity_action() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let first_role_arn = format!("arn:aws:iam::{account}:role/chain-first");

    /// What a row expects of the chained assume.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Expect {
        /// The target admits it, so a session is minted.
        Minted,
        /// Refused, naming this action.
        Refused(&'static str),
    }

    // (case, the TARGET role's trust-policy statements, the source identity the chained
    // request names, what it answers)
    type Row = (&'static str, Vec<String>, Option<&'static str>, Expect);
    let rows: Vec<Row> = vec![
        (
            // The documented shape: one statement granting both actions, narrowed to one
            // source-identity value. The carried value is not that one, so the clause
            // decides the refusal — and it gates the SET.
            "both actions under an unsatisfied source-identity condition",
            vec![statement(
                &first_role_arn,
                &[ASSUME, SET_SOURCE_IDENTITY],
                Some(&format!(
                    "{{\"StringEquals\":{{\"sts:SourceIdentity\":\"{DEMANDED}\"}}}}"
                )),
            )],
            None,
            Expect::Refused(SET_SOURCE_IDENTITY),
        ),
        (
            // The same shape with the clause written under another operator, over a value
            // the carried one does not match either.
            "both actions under an unsatisfied source-identity pattern",
            vec![statement(
                &first_role_arn,
                &[ASSUME, SET_SOURCE_IDENTITY],
                Some("{\"StringLike\":{\"sts:SourceIdentity\":\"sim2real-a*\"}}"),
            )],
            None,
            Expect::Refused(SET_SOURCE_IDENTITY),
        ),
        (
            // The clause narrows a SEPARATE statement, the one admitting the set: the
            // assume itself is admitted unconditionally and the refusal is still the set's.
            "the assume admitted, the set narrowed by the condition",
            vec![
                statement(&first_role_arn, &[ASSUME], None),
                statement(
                    &first_role_arn,
                    &[SET_SOURCE_IDENTITY],
                    Some(&format!(
                        "{{\"StringEquals\":{{\"sts:SourceIdentity\":\"{DEMANDED}\"}}}}"
                    )),
                ),
            ],
            None,
            Expect::Refused(SET_SOURCE_IDENTITY),
        ),
        (
            // No clause over the value anywhere: the target simply does not grant the set,
            // and a chained request that named no source identity asked only to assume.
            "no source-identity condition and no grant of the set",
            vec![statement(&first_role_arn, &[ASSUME], None)],
            None,
            Expect::Refused(ASSUME),
        ),
        (
            // The same target, asked to set a value by the REQUEST: that caller asked for
            // the set, so its refusal names it.
            "the chained request names a source identity without the grant",
            vec![statement(&first_role_arn, &[ASSUME], None)],
            Some(CARRIED),
            Expect::Refused(SET_SOURCE_IDENTITY),
        ),
        (
            // The condition holds for the value the chain carries, so the mint is served:
            // the clause refuses only what it does not admit.
            "both actions under a satisfied source-identity condition",
            vec![statement(
                &first_role_arn,
                &[ASSUME, SET_SOURCE_IDENTITY],
                Some(&format!(
                    "{{\"StringEquals\":{{\"sts:SourceIdentity\":\"{CARRIED}\"}}}}"
                )),
            )],
            None,
            Expect::Minted,
        ),
        (
            // Both actions granted with nothing narrowing them: the carried value travels.
            "both actions, unconditioned",
            vec![statement(&first_role_arn, &[ASSUME, SET_SOURCE_IDENTITY], None)],
            None,
            Expect::Minted,
        ),
    ];

    for (index, (case, target_statements, named, expected)) in rows.into_iter().enumerate() {
        let (router, engine) = emulator();
        // The FIRST role trusts the account and admits the source identity the chain starts
        // with, so every row's divergence is the TARGET's own trust policy.
        let (first_arn, _) = seed_role(
            &engine,
            &format!("AROASOURCEIDFIRST{index:04}"),
            "chain-first",
            "/",
            Some(&policy(&[statement(
                &root,
                &[ASSUME, SET_SOURCE_IDENTITY],
                None,
            )])),
            Some(43_200),
        );
        let (second_arn, _) = seed_role(
            &engine,
            &format!("AROASOURCEIDSECOND{index:03}"),
            "chain-second",
            "/",
            Some(&policy(&target_statements)),
            Some(43_200),
        );

        let (status, body) = call(
            &router,
            "AKIAIOSFODNN7EXAMPLE",
            "AssumeRole",
            &[
                ("RoleArn", first_arn),
                ("RoleSessionName", "hop-one".to_string()),
                ("SourceIdentity", CARRIED.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first assume must succeed, got {status}: {body}"
        );
        let first_session_arn = member(case, &body, "Arn");
        let first_key = member(case, &body, "AccessKeyId");

        let mut chained = vec![
            ("RoleArn", second_arn.clone()),
            ("RoleSessionName", "hop-two".to_string()),
        ];
        if let Some(named) = named {
            chained.push(("SourceIdentity", named.to_string()));
        }
        let (status, body) = call(&router, &first_key, "AssumeRole", &chained).await;
        match expected {
            Expect::Minted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the target admits this mint, got {status}: {body}"
                );
                assert_eq!(
                    element(&body, "SourceIdentity").as_deref(),
                    Some(CARRIED),
                    "case {case:?}: the carried source identity must travel the chain: {body}"
                );
            }
            Expect::Refused(action) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some("AccessDenied"),
                    "case {case:?}: expected AccessDenied, got {body}"
                );
                let message = element(&body, "Message").unwrap_or_default();
                assert_eq!(
                    message,
                    not_authorized(&first_session_arn, action, &second_arn),
                    "case {case:?}: expected the refusal to name {action}, got {message:?}"
                );
            }
        }
    }
}

/// RULE: a chained request that names a source identity DIFFERENT from the one the calling
/// session carries is refused before any trust policy is read — the value "persists across
/// chained role sessions" and a chained mint does not replace it — which is a
/// `ValidationError` naming that state rather than an authorization refusal.
#[tokio::test]
async fn a_conflicting_source_identity_is_refused_as_already_set() {
    let account = emulator_account();
    let root = format!("arn:aws:iam::{account}:root");
    let first_role_arn = format!("arn:aws:iam::{account}:role/conflict-first");
    let (router, engine) = emulator();
    let (first_arn, _) = seed_role(
        &engine,
        "AROACONFLICTFIRST0001",
        "conflict-first",
        "/",
        Some(&policy(&[statement(
            &root,
            &[ASSUME, SET_SOURCE_IDENTITY],
            None,
        )])),
        Some(43_200),
    );
    let (second_arn, _) = seed_role(
        &engine,
        "AROACONFLICTSECOND001",
        "conflict-second",
        "/",
        Some(&policy(&[statement(
            &first_role_arn,
            &[ASSUME, SET_SOURCE_IDENTITY],
            None,
        )])),
        Some(43_200),
    );

    let case = "a chained assume naming another source identity";
    let (status, body) = call(
        &router,
        "AKIAIOSFODNN7EXAMPLE",
        "AssumeRole",
        &[
            ("RoleArn", first_arn),
            ("RoleSessionName", "hop-one".to_string()),
            ("SourceIdentity", CARRIED.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first assume must succeed, got {status}: {body}"
    );
    let first_key = member(case, &body, "AccessKeyId");

    let (status, body) = call(
        &router,
        &first_key,
        "AssumeRole",
        &[
            ("RoleArn", second_arn),
            ("RoleSessionName", "hop-two".to_string()),
            ("SourceIdentity", DEMANDED.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected ValidationError, got {body}"
    );
    assert_eq!(
        element(&body, "Message").as_deref(),
        Some("The source identity is already set for this assume role session"),
        "case {case:?}: expected the already-set sentence, got {body}"
    );
}
