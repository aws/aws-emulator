// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for THE AUTHORITY A BATCHED READ NEEDS, driven over the real wire path
//! (an awsJson1_1 request through the multi-service router into the mounted service beside
//! the IAM and STS services on one shared engine), so what is asserted is what a client
//! holding those credentials reads.
//!
//! THE RULE: `BatchGetSecretValue` needs the action of its own operation for the call AND
//! the action of a single read of a value for EACH secret it answers for, with the filter
//! form needing the action of the account-wide listing in addition — "Required permissions:
//! secretsmanager:BatchGetSecretValue, and you must have secretsmanager:GetSecretValue for
//! each secret. If you use filters, you must also have secretsmanager:ListSecrets."
//! (`BatchGetSecretValue`, this service's Smithy model.) A per-SECRET refusal degrades that
//! one entry and nothing else: the call is still a 200 carrying the readable values in
//! `SecretValues` and one `Errors` entry whose `SecretId` is the secret as the request named
//! it, whose `ErrorCode` is the common `AccessDeniedException` and whose `Message` is IAM's
//! own refusal sentence — "If Secrets Manager encounters errors such as
//! AccessDeniedException while attempting to retrieve any of the secrets, you can see the
//! errors in Errors in the response." A refusal of the CALL's own authority, the listing
//! authority the filter form needs included, fails the whole call instead.
//!
//! The tables span what the rule generalises over: which policy withholds the read (no
//! statement allowing it, an explicit deny in an identity-based policy, an explicit deny in
//! the secret's own resource-based policy), where in the batch the withheld secret sits, the
//! form of the `SecretId` the request names each secret by, and which selector the request
//! carries.
//!
//! `tests/all/secret_batch_get.rs` spans the batched read's own request-form and selection
//! rules; `tests/all/identity_based_authorization.rs` spans the gate itself and owns the
//! harness these cases reuse.

use serde_json::{json, Value};

use crate::identity_based_authorization::{
    call_as, error_code, error_message, given_role_policy, given_secret, given_session,
    router_with_identity, session_arn, ACCESS_DENIED, ACCESS_DENIED_STATUS, ACCOUNT_KEY_ID,
    SECRET_VALUE, SESSION_KEY_ID,
};

/// The three secrets a case holds, so a withheld one can sit first, in the middle or last.
const SECRETS: [&str; 3] = ["batch-first", "batch-middle", "batch-last"];

/// A document allowing every action of this service on every resource, which the rows that
/// withhold a read with something OTHER than a missing grant put in force.
fn allow_the_service() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "secretsmanager:*",
            "Resource": "*",
        }],
    })
}

/// A document allowing the batched read on everything and the single read on `readable`
/// only, so every other secret's read falls to IAM's default deny.
fn allow_reads_of(readable: &[String]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["secretsmanager:BatchGetSecretValue", "secretsmanager:ListSecrets"],
                "Resource": "*",
            },
            {
                "Effect": "Allow",
                "Action": "secretsmanager:GetSecretValue",
                "Resource": readable,
            },
        ],
    })
}

/// A document allowing every action of this service beside an explicit deny of the single
/// read of `denied`, so that one read meets IAM's explicit deny.
fn deny_read_of(denied: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "secretsmanager:*", "Resource": "*" },
            {
                "Effect": "Deny",
                "Action": "secretsmanager:GetSecretValue",
                "Resource": denied,
            },
        ],
    })
}

/// A resource-based policy denying every principal the single read, as a secret is locked
/// down with: "By default, public policies aren't blocked"
/// (`PutResourcePolicyRequest$BlockPublicPolicy`), so the wildcard principal is admitted.
fn resource_policy_denying_reads() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": { "AWS": "*" },
            "Action": ["secretsmanager:GetSecretValue", "secretsmanager:BatchGetSecretValue"],
            "Resource": "*",
        }],
    })
}

/// The `SecretValues` of a batched read, as (ARN, SecretString) pairs in the order answered.
fn values_of(answer: &Value, case: &str) -> Vec<(String, String)> {
    answer
        .get("SecretValues")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretValues in {answer}"))
        .iter()
        .map(|entry| {
            (
                entry["ARN"].as_str().unwrap_or_default().to_string(),
                entry["SecretString"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `Errors` of a batched read, as (SecretId, ErrorCode, Message) triples.
fn errors_of(answer: &Value, case: &str) -> Vec<(String, String, String)> {
    answer
        .get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Errors in {answer}"))
        .iter()
        .map(|entry| {
            (
                entry["SecretId"].as_str().unwrap_or_default().to_string(),
                entry["ErrorCode"].as_str().unwrap_or_default().to_string(),
                entry["Message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// IAM's refusal sentence for the single read of `supplied`, closed with `reason`.
fn refusal_of_the_read(supplied: &str, reason: &str) -> String {
    format!(
        "User: {} is not authorized to perform: secretsmanager:GetSecretValue on resource: \
         {supplied} {reason}",
        session_arn()
    )
}

/// THE RULE per entry: whichever policy withholds the read of ONE selected secret, the call
/// is a 200 that answers the others and reports that one in `Errors`, carrying the id the
/// request supplied, the common access-denied code and IAM's own sentence.
#[tokio::test]
async fn a_secret_the_caller_may_not_read_is_one_errors_entry_inside_a_two_hundred() {
    // The clauses IAM closes a refusal of the single read with.
    const NO_GRANT: &str = "because no identity-based policy allows the \
                            secretsmanager:GetSecretValue action";
    const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";
    const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

    // (what withholds the read, the clause it refuses with, whether it is attached to the
    // secret rather than to the identity)
    let withholdings: [(&str, &str, bool); 3] = [
        ("no statement allowing the read", NO_GRANT, false),
        ("an explicit deny in an identity-based policy", IDENTITY_DENY, false),
        ("an explicit deny in the secret's own policy", RESOURCE_DENY, true),
    ];
    // Where the withheld secret sits in the batch.
    for withheld_at in 0..SECRETS.len() {
        for (withholding, reason, on_the_secret) in withholdings {
            let (router, engine) = router_with_identity();
            given_session(&engine);
            let mut arns: Vec<String> = Vec::new();
            for name in SECRETS {
                arns.push(given_secret(&router, name).await);
            }
            let withheld_arn = arns[withheld_at].clone();
            let readable: Vec<String> = arns
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != withheld_at)
                .map(|(_, arn)| arn.clone())
                .collect();

            if on_the_secret {
                given_role_policy(&engine, Some(&allow_the_service()));
                let (status, attached) = call_as(
                    &router,
                    ACCOUNT_KEY_ID,
                    "PutResourcePolicy",
                    &json!({
                        "SecretId": withheld_arn,
                        "ResourcePolicy": resource_policy_denying_reads().to_string(),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {withholding:?}: the policy must attach, got {status} {attached}"
                );
            } else if reason == IDENTITY_DENY {
                given_role_policy(&engine, Some(&deny_read_of(&withheld_arn)));
            } else {
                given_role_policy(&engine, Some(&allow_reads_of(&readable)));
            }

            // The form the request names each secret by: its name, or the ARN the account
            // holds it under. Both address the same secrets, and an entry reports the id as
            // the request spelled it.
            let id_forms: [(&str, Vec<String>); 2] = [
                ("their names", SECRETS.iter().map(|n| n.to_string()).collect()),
                ("their ARNs", arns.clone()),
            ];
            for (form, ids) in id_forms {
                let case =
                    format!("{withholding} on {} addressed by {form}", SECRETS[withheld_at]);
                let supplied = ids[withheld_at].clone();
                let (status, answer) = call_as(
                    &router,
                    SESSION_KEY_ID,
                    "BatchGetSecretValue",
                    &json!({ "SecretIdList": ids }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: one withheld secret must not fail the call, got {status} \
                     {answer}"
                );
                let answered = values_of(&answer, &case);
                assert_eq!(
                    answered,
                    readable
                        .iter()
                        .map(|arn| (arn.clone(), SECRET_VALUE.to_string()))
                        .collect::<Vec<(String, String)>>(),
                    "case {case:?}: expected the other secrets' values, got {answered:?}"
                );
                let reported = errors_of(&answer, &case);
                assert_eq!(
                    reported,
                    vec![(
                        supplied.clone(),
                        ACCESS_DENIED.to_string(),
                        refusal_of_the_read(&supplied, reason),
                    )],
                    "case {case:?}: expected one access-denied entry for the withheld secret, \
                     got {reported:?}"
                );
            }
        }
    }
}

/// THE POSITIVE CONTROL: lifting the denial restores the full batch — every secret is
/// answered again and `Errors` is empty, so the partition follows the policies in force
/// rather than anything the refused read left behind.
#[tokio::test]
async fn lifting_the_denial_restores_every_value_to_the_batch() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    given_role_policy(&engine, Some(&allow_the_service()));
    let mut arns: Vec<String> = Vec::new();
    for name in SECRETS {
        arns.push(given_secret(&router, name).await);
    }
    // (what the row varies, the policies in force, how many values the batch answers)
    let locked = arns[1].clone();
    let rows: [(&str, bool, usize); 3] = [
        ("before any denial", false, SECRETS.len()),
        ("with the secret's policy denying reads", true, SECRETS.len() - 1),
        ("after the policy is deleted", false, SECRETS.len()),
    ];
    for (case, deny, expected) in rows {
        let (status, written) = if deny {
            call_as(
                &router,
                ACCOUNT_KEY_ID,
                "PutResourcePolicy",
                &json!({
                    "SecretId": locked,
                    "ResourcePolicy": resource_policy_denying_reads().to_string(),
                }),
            )
            .await
        } else {
            call_as(
                &router,
                ACCOUNT_KEY_ID,
                "DeleteResourcePolicy",
                &json!({ "SecretId": locked }),
            )
            .await
        };
        assert_eq!(
            status, 200,
            "case {case:?}: the policy write must be served, got {status} {written}"
        );

        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "BatchGetSecretValue",
            &json!({ "SecretIdList": arns.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the batched read must be served, got {status} {answer}"
        );
        let answered = values_of(&answer, case);
        assert_eq!(
            answered.len(),
            expected,
            "case {case:?}: expected {expected} value(s), got {answered:?}"
        );
        let reported = errors_of(&answer, case);
        assert_eq!(
            reported.len(),
            SECRETS.len() - expected,
            "case {case:?}: expected {} error entry(ies), got {reported:?}",
            SECRETS.len() - expected
        );
    }
}

/// THE RULE for the call's own authority: the FILTER form selects over the account's
/// secrets, so it needs the listing's action as well — a caller without it is refused the
/// whole call, naming that action and no resource, while the same caller's list form is
/// served; granting the action serves the filter form too.
#[tokio::test]
async fn the_filter_form_needs_the_listing_authority() {
    const NO_LISTING: &str = "because no identity-based policy allows the \
                              secretsmanager:ListSecrets action";
    // The selections the filter form carries: a filter over the name, one over every
    // attribute (`Filter$Key`: name, description, tag-key, tag-value, primary-region,
    // owning-service, all), and the empty list, which selects the whole Region.
    let selections: [(&str, Value); 3] = [
        ("a name filter", json!([{ "Key": "name", "Values": ["filtered-"] }])),
        ("an all filter", json!([{ "Key": "all", "Values": ["filtered"] }])),
        ("no filter value", json!([])),
    ];
    for (selection, filters) in selections {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let arn = given_secret(&router, "filtered-secret").await;
        // Everything the batched read needs EXCEPT the listing: the call's own action, and
        // the single read of the one secret a filter would select.
        given_role_policy(&engine, Some(&json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": "secretsmanager:BatchGetSecretValue",
                    "Resource": "*",
                },
                {
                    "Effect": "Allow",
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": arn,
                },
            ],
        })));

        // The list form is served for that very secret — the case's own control, so the
        // refusal below cannot be read as a missing read grant.
        let case = format!("{selection}: the list form");
        let (status, served) = call_as(
            &router,
            SESSION_KEY_ID,
            "BatchGetSecretValue",
            &json!({ "SecretIdList": [arn.clone()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the list form must be served, got {status} {served}"
        );
        let answered = values_of(&served, &case);
        assert_eq!(
            answered,
            vec![(arn.clone(), SECRET_VALUE.to_string())],
            "case {case:?}: the list form must answer the stored value, got {answered:?}"
        );

        let case = format!("{selection}: the filter form without the listing authority");
        let (status, refused) = call_as(
            &router,
            SESSION_KEY_ID,
            "BatchGetSecretValue",
            &json!({ "Filters": filters.clone() }),
        )
        .await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            ACCESS_DENIED,
            "case {case:?}: expected an {ACCESS_DENIED}, got {refused}"
        );
        let expected = format!(
            "User: {} is not authorized to perform: secretsmanager:ListSecrets {NO_LISTING}",
            session_arn()
        );
        assert_eq!(
            error_message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&refused)
        );
        assert!(
            refused["SecretValues"].is_null(),
            "case {case:?}: a refused call must answer no value, got {refused}"
        );

        // Granting the listing action serves the same filter form.
        given_role_policy(&engine, Some(&allow_the_service()));
        let case = format!("{selection}: the filter form with the listing authority");
        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "BatchGetSecretValue",
            &json!({ "Filters": filters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the granted filter form must be served, got {status} {answer}"
        );
        let answered = values_of(&answer, &case);
        assert_eq!(
            answered,
            vec![(arn.clone(), SECRET_VALUE.to_string())],
            "case {case:?}: the filter form must answer the selected secret, got {answered:?}"
        );
    }
}
