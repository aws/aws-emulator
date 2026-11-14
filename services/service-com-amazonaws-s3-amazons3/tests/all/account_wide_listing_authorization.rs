// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what authorizes the account's own BUCKET LISTING — an action that
//! addresses no resource of its own.
//!
//! RULE: a signed request is authorized, per action, against the identity-based documents in
//! force on the principal that signed it, and `ListBuckets` asks for `s3:ListAllMyBuckets`
//! ("to grant IAM permission to use this operation, you must add the `s3:ListAllMyBuckets`
//! policy action", `ListBuckets`'s own documentation). That action supports no resource types,
//! so the only `Resource` element that can grant it is IAM's wildcard — a grant scoped to a
//! bucket's ARN does not reach it — and a request no document allows is refused 403
//! `AccessDenied` naming the principal, the action and IAM's reason clause, with NO `on
//! resource:` clause, there being no resource the request addressed.
//!
//! The rows differ along what the rule generalises over: which actions the caller's own
//! document grants (none, this one, every S3 action, another action, this one scoped to a
//! bucket ARN) and whether a document explicitly denies it.
//!
//! The harness is `identity_authorization`'s: S3 beside IAM over ONE shared engine, with the
//! user, its access key and its policies created through IAM's own API, so the documents the
//! decision reads are the ones `iam:PutUserPolicy` wrote.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, send_bytes_signed};
use super::identity_authorization::{element, router, s3, user_arn, user_with_grant};

/// The IAM action the account's own bucket listing asks for.
const LIST_ALL_MY_BUCKETS: &str = "s3:ListAllMyBuckets";

/// The IAM action the account's DIRECTORY-bucket listing asks for: "you must have the
/// `s3express:ListAllMyDirectoryBuckets` permission in an IAM identity-based policy instead
/// of a bucket policy" (`ListDirectoryBuckets`'s own documentation).
const LIST_ALL_MY_DIRECTORY_BUCKETS: &str = "s3express:ListAllMyDirectoryBuckets";

/// The host the S3 Express control endpoint — the one `ListDirectoryBuckets` is served on —
/// is addressed as.
const S3_EXPRESS_CONTROL_HOST: &str = "s3express-control.us-east-1.amazonaws.com";

/// The code S3 answers an authorization refusal with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The bucket the listing reports for a caller the documents in force allow.
const BUCKET: &str = "account-wide-listing";

/// `GET /?x-id=ListBuckets` as `akid`.
async fn list_buckets(router: &MultiServiceRouter, akid: &str) -> super::bucket_lifecycle::Answer {
    s3(router, Some(akid), "GET", "/?x-id=ListBuckets", b"").await
}

/// `GET /` on the S3 Express control endpoint as `akid` — `ListDirectoryBuckets`, the
/// account's OTHER listing that addresses no resource, signed for that endpoint's own signing
/// name.
async fn list_directory_buckets(
    router: &MultiServiceRouter,
    akid: &str,
) -> super::bucket_lifecycle::Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1/s3express/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    send_bytes_signed(
        router,
        &authorization,
        "GET",
        S3_EXPRESS_CONTROL_HOST,
        "/?x-id=ListDirectoryBuckets",
        &[],
        b"",
    )
    .await
}

/// The sentence S3 refuses an action that addresses NO resource with: the principal that
/// signed it, the action it asked for and IAM's clause for an absent grant. Written out here
/// rather than built from the service's own helper, so the wording has something to be wrong
/// about.
fn no_grant_message(principal: &str, action: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} because no identity-based \
         policy allows the {action} action"
    )
}

/// The sentence it refuses one an identity-based document explicitly DENIES with.
fn explicit_deny_message(principal: &str, action: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} with an explicit deny in an \
         identity-based policy"
    )
}

/// What a row's listing must answer.
enum Expected {
    /// Served, and the listing names the account's bucket.
    Served,
    /// Refused 403 `AccessDenied` because no document in force allows the action.
    NoGrant,
    /// Refused 403 `AccessDenied` because a document in force denies it.
    Denied,
}

/// One row: the actions the row's caller is granted (with the resources they are granted on),
/// the actions it is denied, and what its listing must answer.
struct Row<'a> {
    case: &'a str,
    granted: &'a [&'a str],
    /// The resources the grant names; empty for a grant on IAM's wildcard.
    resources: &'a [&'a str],
    denied: &'a [&'a str],
    expected: Expected,
}

#[tokio::test]
async fn the_documents_in_force_decide_the_accounts_own_bucket_listing() {
    let router = router();
    // The account's own credential creates the bucket the served rows must see: it speaks for
    // the account, which no identity-based policy bounds.
    let created = create_bucket(&router, BUCKET, None, None).await;
    assert_eq!(
        created.status, 200,
        "setup: the bucket must be created, got {}",
        created.status
    );

    let rows: &[Row<'_>] = &[
        Row {
            case: "no document in force at all",
            granted: &[],
            resources: &[],
            denied: &[],
            expected: Expected::NoGrant,
        },
        Row {
            case: "granted exactly this action",
            granted: &[LIST_ALL_MY_BUCKETS],
            resources: &[],
            denied: &[],
            expected: Expected::Served,
        },
        Row {
            case: "granted every action of this service",
            granted: &["s3:*"],
            resources: &[],
            denied: &[],
            expected: Expected::Served,
        },
        Row {
            case: "granted another bucket-listing action only",
            granted: &["s3:ListBucket"],
            resources: &[],
            denied: &[],
            expected: Expected::NoGrant,
        },
        Row {
            case: "granted this action, but only on one bucket's ARN",
            granted: &[LIST_ALL_MY_BUCKETS],
            resources: &["arn:aws:s3:::account-wide-listing"],
            denied: &[],
            expected: Expected::NoGrant,
        },
        Row {
            case: "granted every action and denied this one",
            granted: &["s3:*"],
            resources: &[],
            denied: &[LIST_ALL_MY_BUCKETS],
            expected: Expected::Denied,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("listing-probe-{index}");
        let resources: Vec<String> = if row.resources.is_empty() {
            vec!["*".to_string()]
        } else {
            row.resources.iter().map(|arn| (*arn).to_string()).collect()
        };
        let akid = user_with_grant(&router, &name, row.granted, &resources).await;
        if !row.denied.is_empty() {
            let document = format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Deny\",\
                 \"Action\":\"{}\",\"Resource\":\"*\"}}]}}",
                row.denied.join("\",\"")
            );
            let stored = super::identity_authorization::put_user_policy(
                &router, &name, "denied", &document,
            )
            .await;
            assert!(
                stored,
                "case {case:?}: the deny document must be stored for {name}"
            );
        }
        let principal = user_arn(&name);
        let answered = list_buckets(&router, &akid).await;
        let body = String::from_utf8_lossy(&answered.body).to_string();
        match row.expected {
            Expected::Served => {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: the listing must be served, got {} with {body}",
                    answered.status
                );
                assert!(
                    body.contains(&format!("<Name>{BUCKET}</Name>")),
                    "case {case:?}: the listing must name the account's bucket, got {body}"
                );
            }
            Expected::NoGrant | Expected::Denied => {
                assert_eq!(
                    answered.status, 403,
                    "case {case:?}: the listing must be refused 403, got {} with {body}",
                    answered.status
                );
                assert_eq!(
                    element(&body, "Code"),
                    ACCESS_DENIED,
                    "case {case:?}: expected the code {ACCESS_DENIED}, got {body}"
                );
                let expected = match row.expected {
                    Expected::Denied => explicit_deny_message(&principal, LIST_ALL_MY_BUCKETS),
                    _ => no_grant_message(&principal, LIST_ALL_MY_BUCKETS),
                };
                let actual = element(&body, "Message");
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected the refusal {expected:?}, got {actual:?}"
                );
                assert!(
                    !actual.contains("on resource"),
                    "case {case:?}: an action that addresses no resource names none, got \
                     {actual:?}"
                );
            }
        }
    }
}

/// RULE, the same decision for the account's OTHER listing that addresses no resource: the
/// directory-bucket listing asks for its own action, so a grant of the general purpose
/// listing does not reach it and a grant of its own action does.
#[tokio::test]
async fn each_account_wide_listing_asks_for_its_own_action() {
    let router = router();

    // (case, the action the caller is granted, the listings it may make)
    let rows: &[(&str, Option<&str>, bool, bool)] = &[
        ("no document in force at all", None, false, false),
        (
            "granted the general purpose listing alone",
            Some(LIST_ALL_MY_BUCKETS),
            true,
            false,
        ),
        (
            "granted the directory listing alone",
            Some(LIST_ALL_MY_DIRECTORY_BUCKETS),
            false,
            true,
        ),
    ];
    for (index, (case, granted, may_list_buckets, may_list_directory)) in
        rows.iter().enumerate()
    {
        let name = format!("listing-action-probe-{index}");
        let actions: Vec<&str> = granted.iter().copied().collect();
        let akid =
            user_with_grant(&router, &name, &actions, &["*".to_string()]).await;
        let principal = user_arn(&name);

        let general = list_buckets(&router, &akid).await;
        let expected = if *may_list_buckets { 200 } else { 403 };
        assert_eq!(
            general.status,
            expected,
            "case {case:?}: ListBuckets for {principal} expected HTTP {expected}, got {} with \
             {}",
            general.status,
            String::from_utf8_lossy(&general.body)
        );

        let directory = list_directory_buckets(&router, &akid).await;
        let expected = if *may_list_directory { 200 } else { 403 };
        assert_eq!(
            directory.status,
            expected,
            "case {case:?}: ListDirectoryBuckets for {principal} expected HTTP {expected}, \
             got {} with {}",
            directory.status,
            String::from_utf8_lossy(&directory.body)
        );
        if !*may_list_directory {
            let message = element(
                String::from_utf8_lossy(&directory.body).as_ref(),
                "Message",
            );
            let wanted = no_grant_message(&principal, LIST_ALL_MY_DIRECTORY_BUCKETS);
            assert_eq!(
                message, wanted,
                "case {case:?}: expected the refusal {wanted:?}, got {message:?}"
            );
        }
    }
}
