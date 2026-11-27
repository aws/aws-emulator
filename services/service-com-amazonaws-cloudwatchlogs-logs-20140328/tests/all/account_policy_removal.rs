// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DeleteAccountPolicy` removes, and for what a refused delete
//! leaves behind.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a delete removes exactly the ONE account-level policy the
//! request's `(policyType, policyName)` pair names — both members identify it, matched
//! whole — leaving every other stored policy reported exactly as it was; a pair the
//! account does not hold is refused with `ResourceNotFoundException` (HTTP 400) and
//! changes nothing.
//!
//! The tables below span what that rule generalises over: the policy type deleted, how
//! many policies of that type the account holds, and the ways a named pair can be absent
//! (never put, already deleted, held under another type, a name that differs only in case,
//! in surrounding whitespace, or in being a prefix or an extension of a held one). The
//! single nonexistent policy an issue happens to cite is one row.
//!
//! The suites next door cover the rest of this loop: `tests/all/account_policies.rs` the
//! put / describe / delete round trip and the per-type quotas,
//! `tests/all/account_policy_release.rs` what a delete gives back to a later put, and
//! `tests/all/account_policy_selection_criteria.rs` the criteria grammar.

use serde_json::{json, Value};

use crate::account_policies::{
    delete_body, describe, member, put_body, scoped_criteria, POLICY_TYPES,
};
use crate::account_policy_selection_criteria::document_for;
use crate::harness::Logs;

/// The policy name every type in the fixture holds a policy under, so a delete that keyed
/// on the name alone would take the other types' policies with it.
const SHARED: &str = "shared";

/// The names the fixture holds of one type. The types whose quota admits more than one
/// policy — "as many as 20 account-level transformer policies", "as many as 20 account-level
/// field index policies", "up to 5 policies that are each scoped to a subset of log groups"
/// for a metric extraction policy and "up to 20 account-level retention policies in each
/// Region" — hold three policies each, so a delete of one of them has siblings of its own
/// type to leave alone. The other two have no scoped half ("Each account can have only one
/// account-level data protection policy", "Each account can have one account-level
/// subscription filter policy per Region"), so the account holds exactly one of each.
/// Source: `PutAccountPolicy`'s quota sentences.
fn fixture_names(policy_type: &str) -> Vec<&'static str> {
    match policy_type {
        "FIELD_INDEX_POLICY" | "TRANSFORMER_POLICY" | "METRIC_EXTRACTION_POLICY"
        | "RETENTION_POLICY" => {
            vec![SHARED, "keep-1", "keep-2"]
        }
        _ => vec![SHARED],
    }
}

/// An account holding a policy of every modelled type, each type's policies scoped to
/// disjoint subsets of log groups where its quota lets it hold more than one.
async fn account_holding_every_type() -> Logs {
    let logs = Logs::new();
    for policy_type in POLICY_TYPES {
        let document = document_for(policy_type);
        let names = fixture_names(policy_type);
        let scoped = names.len() > 1;
        for (index, name) in names.iter().enumerate() {
            let criteria = if scoped {
                Some(scoped_criteria(policy_type, index))
            } else {
                None
            };
            logs.ok(
                "PutAccountPolicy",
                &put_body(name, policy_type, &document, criteria.as_deref()),
            )
            .await;
        }
    }
    logs
}

/// Every policy the account holds, by type, as `DescribeAccountPolicies` reports it —
/// member for member, so a delete that restamped or rewrote a policy it was not asked
/// about is visible.
async fn snapshot(logs: &Logs) -> Vec<(&'static str, Vec<Value>)> {
    let mut held = Vec::new();
    for policy_type in POLICY_TYPES {
        held.push((
            policy_type,
            describe(logs, &json!({"policyType": policy_type})).await,
        ));
    }
    held
}

#[tokio::test]
async fn a_delete_removes_exactly_the_pair_it_names() {
    for deleted_type in POLICY_TYPES {
        let case = deleted_type;
        let logs = account_holding_every_type().await;
        let before = snapshot(&logs).await;

        let response = logs
            .ok("DeleteAccountPolicy", &delete_body(SHARED, deleted_type))
            .await;
        // A delete that is not a dry run reports no `dryRunResult`, which is the only
        // member this operation's output carries ("This field is returned only when the
        // request specifies dryRun as true").
        assert!(
            response.is_null() || response.as_object().is_some_and(|body| body.is_empty()),
            "case {case}: DeleteAccountPolicy answers no members, got {response}"
        );

        let after = snapshot(&logs).await;
        for ((policy_type, held_before), (_, held_after)) in before.iter().zip(after.iter()) {
            let expected: Vec<Value> = held_before
                .iter()
                .filter(|policy| {
                    !(*policy_type == deleted_type && member(policy, "policyName") == SHARED)
                })
                .cloned()
                .collect();
            assert_eq!(
                held_after, &expected,
                "case {case}: expected {policy_type} to report {expected:?} after the delete, got {held_after:?}"
            );
        }

        // The policy is gone from the store rather than hidden from the reader: the very
        // same delete no longer finds a policy to remove.
        let code = logs
            .err("DeleteAccountPolicy", &delete_body(SHARED, deleted_type))
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException deleting it again, got {code}"
        );
    }
}

#[tokio::test]
async fn a_delete_of_a_pair_the_account_does_not_hold_is_refused_and_changes_nothing() {
    let logs = account_holding_every_type().await;
    // One pair the account held and holds no longer, so the table has an already-deleted
    // row beside the never-stored ones.
    logs.ok(
        "DeleteAccountPolicy",
        &delete_body("keep-2", "FIELD_INDEX_POLICY"),
    )
    .await;
    let before = snapshot(&logs).await;

    // Each row names a pair the account does not hold. The rows differ in WHY it is
    // absent: the name was never put, the name is held only under other types, or the name
    // differs from a held one in case, in surrounding whitespace, or in being a prefix or
    // an extension of it — `policyName` carries no case- or space-folding, so each of
    // those is a different policy.
    let mut rows: Vec<(String, String, &str)> = vec![
        (
            "FIELD_INDEX_POLICY already-deleted".to_string(),
            "keep-2".to_string(),
            "FIELD_INDEX_POLICY",
        ),
        (
            "DATA_PROTECTION_POLICY a-name-held-only-under-the-scoped-types".to_string(),
            "keep-1".to_string(),
            "DATA_PROTECTION_POLICY",
        ),
        (
            "SUBSCRIPTION_FILTER_POLICY a-name-held-only-under-the-scoped-types".to_string(),
            "keep-1".to_string(),
            "SUBSCRIPTION_FILTER_POLICY",
        ),
    ];
    for policy_type in POLICY_TYPES {
        for (case, name) in [
            ("never-put", "never-put".to_string()),
            ("upper-cased", SHARED.to_ascii_uppercase()),
            ("leading-space", format!(" {SHARED}")),
            ("trailing-space", format!("{SHARED} ")),
            ("a-prefix-of-a-held-name", SHARED[..SHARED.len() - 1].to_string()),
            ("an-extension-of-a-held-name", format!("{SHARED}-more")),
            ("empty", String::new()),
        ] {
            rows.push((format!("{policy_type} {case}"), name, policy_type));
        }
    }

    for (case, policy_name, policy_type) in &rows {
        let (code, message) = logs
            .refusal("DeleteAccountPolicy", &delete_body(policy_name, policy_type))
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {code} ({message})"
        );
        assert!(
            message.contains("does not exist"),
            "case {case}: the refusal says the resource is not there, got {message}"
        );
    }

    // Not one of those refusals — nor the reads between them — moved anything: every type
    // reports the policies it held before, member for member.
    let after = snapshot(&logs).await;
    assert_eq!(
        after, before,
        "expected {} refused deletes to leave every policy as it was, got {after:?}",
        rows.len()
    );
}
