// SPDX-License-Identifier: CC0-1.0

//! The principal an authorization refusal over a recovery point blames when the
//! request is signed with a LONG-TERM access key of an IAM user.
//!
//! The rule under test: the refusal's `Message` names the principal the request's
//! credentials speak for, as `sts:GetCallerIdentity` reports it — so the name in the
//! sentence is read off the credential the request presented, never written in. A
//! long-term access key is the third kind of credential a request can present here,
//! and the only one resolved through the IAM store: the account's own published
//! credential and an assumed-role session are the two
//! `recovery_point_denial_sentence.rs` drives, so this suite adds the row that key
//! kind contributes rather than restating that table.
//!
//! The user and its key are created through IAM's OWN API on the same shared engine
//! Backup resolves its callers from, so the principal the refusal names is the one
//! the account really holds that key under. The operation table, the dispatch and
//! the sentence frame are the shared ones in
//! `recovery_point_cross_account_authorization.rs`; the rows span the operations
//! that address a point by the ARN in their path, crossed with foreign-account ARNs
//! that differ in account id, Region and point-id form. The controls are the other
//! half of the rule: for that same caller the account's OWN stored point is
//! answered, and an own-account ARN that names nothing is the read's not-found
//! refusal — so the refusals are about the ARN's account and not about who asked.

use smithy_server_runtime::MultiServiceRouter;

use crate::caller_authorization::{call_as, emulator, ACCOUNT_KEY_ID};
use crate::recovery_point_cross_account_authorization::{
    address_as, denial_as, operations_addressing_by_path, stored_point, VAULT,
};
use crate::vault_and_backup_job::{error_code, string_at, ACCOUNT, REGION};

/// The IAM API version an `awsQuery` request to IAM carries (that service's
/// `version`), and the name of the user whose key the cases sign with.
const IAM_VERSION: &str = "2010-05-08";
const USER_NAME: &str = "recovery-point-reader";

/// The status and error shape an authorization refusal of this service carries.
const DENIED_STATUS: u16 = 403;
const DENIED_CODE: &str = "AccessDeniedException";

/// One `awsQuery` request to IAM on the same router, answered as its raw body.
async fn iam(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("host", "localhost")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup {case:?}: IAM answered {status} {answered}"
    );
    answered
}

/// The text of the `<{tag}>` element of an `awsQuery` answer of this emulator's own.
fn tag_value(case: &str, body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("setup {case:?}: no {open} in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("setup {case:?}: no {close} in {body}"))
        + start;
    body[start..end].to_string()
}

/// Create the user [`USER_NAME`] and one long-term access key for it through IAM's
/// own API, and answer the access key id a request of that user signs with.
async fn given_iam_user(router: &MultiServiceRouter) -> String {
    iam(
        router,
        "CreateUser",
        format!("Action=CreateUser&Version={IAM_VERSION}&UserName={USER_NAME}"),
    )
    .await;
    let created = iam(
        router,
        "CreateAccessKey",
        format!("Action=CreateAccessKey&Version={IAM_VERSION}&UserName={USER_NAME}"),
    )
    .await;
    tag_value("CreateAccessKey", &created, "AccessKeyId")
}

/// The ARN the account holds that user under, which a refusal of its requests names.
fn user_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{USER_NAME}")
}

/// The emulator every case runs against: the vaults and the one settled recovery
/// point of the shared setup, so the rows address a LIVE vault, plus the IAM user
/// whose long-term key they sign with and the ARN of the point that account holds.
async fn given_emulator() -> (MultiServiceRouter, String, String) {
    let (router, _engine) = emulator();
    // AWS Backup assumes the role a start names before it mints a job, so the role the
    // shared setup's backup is performed under is provisioned in the IAM mounted here,
    // carrying the action that creates a table's recovery point so the point is written.
    crate::plan_and_selection_refusals::given_backup_job_role(
        &router,
        "/service-role/",
        "AWSBackupDefaultServiceRole",
        "dynamodb:CreateBackup",
    )
    .await;
    let stored = stored_point(&router).await;
    let key_id = given_iam_user(&router).await;
    (router, key_id, stored)
}

/// Recovery-point ARNs of accounts other than the one serving the request, differing
/// in the parts the sentence quotes back.
fn foreign_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "another account, this Region",
            format!("arn:aws:backup:{REGION}:111122223333:recovery-point:1EB3B5E7"),
        ),
        (
            "a different other account",
            format!("arn:aws:backup:{REGION}:210987654321:recovery-point:9fd3b1c4"),
        ),
        (
            "another account in another Region",
            "arn:aws:backup:eu-central-1:999988887777:recovery-point:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                .to_string(),
        ),
    ]
}

/// A request signed with an IAM user's long-term access key is blamed by THAT user's
/// ARN — not by the account's own identity — for every operation that addresses a
/// recovery point by ARN and every foreign-account ARN form.
#[tokio::test]
async fn a_long_term_key_is_blamed_by_the_arn_of_the_user_that_owns_it() {
    let (router, key_id, _stored) = given_emulator().await;
    let principal = user_arn();

    for operation in operations_addressing_by_path() {
        for (arn_case, arn) in foreign_arns() {
            let case = format!("{} with {arn_case}", operation.operation);
            let (status, refused) = address_as(&router, &operation, &arn, &key_id).await;
            assert_eq!(
                (
                    status,
                    error_code(&refused).as_str(),
                    string_at(&refused, "Message")
                ),
                (
                    DENIED_STATUS,
                    DENIED_CODE,
                    denial_as(&principal, operation.operation, &arn)
                ),
                "case {case:?}: expected the refusal to name {principal:?}, got {status} \
                 {refused:?}"
            );
        }
    }
}

/// The same call signed with the account's own published credential is blamed by the
/// OTHER principal, so the name in the sentence tracks the credential rather than
/// the call: one request, two credentials, two sentences.
#[tokio::test]
async fn the_same_call_under_two_credentials_names_two_principals() {
    let (router, key_id, _stored) = given_emulator().await;
    let arn = format!("arn:aws:backup:{REGION}:111122223333:recovery-point:1EB3B5E7");
    let path = format!("/backup-vaults/{VAULT}/recovery-points/{arn}");

    let (status, as_user) = call_as(&router, &key_id, "GET", &path, None).await;
    assert_eq!(
        (status, string_at(&as_user, "Message")),
        (
            DENIED_STATUS,
            denial_as(&user_arn(), "DescribeRecoveryPoint", &arn)
        ),
        "case \"the IAM user's own key\": expected the refusal to name {:?}, got {status} \
         {as_user:?}",
        user_arn()
    );

    let (status, as_account) = call_as(&router, ACCOUNT_KEY_ID, "GET", &path, None).await;
    let account_message = string_at(&as_account, "Message");
    assert_eq!(
        status, DENIED_STATUS,
        "case \"the account's own credential\": expected {DENIED_STATUS}, got {status} \
         {as_account:?}"
    );
    assert_ne!(
        account_message,
        string_at(&as_user, "Message"),
        "case \"the account's own credential\": expected a sentence naming its own \
         principal, got the IAM user's"
    );
    assert!(
        !account_message.contains(&user_arn()),
        "case \"the account's own credential\": the refusal must not name another \
         principal, got {account_message:?}"
    );
}

/// The controls: for that same caller, the account's OWN stored point is answered
/// and an own-account ARN that names nothing is the read's not-found refusal — so
/// the refusals above turn on the ARN's account and not on who asked.
#[tokio::test]
async fn an_own_account_point_is_answered_for_that_caller() {
    let (router, key_id, stored) = given_emulator().await;

    let path = format!("/backup-vaults/{VAULT}/recovery-points/{stored}");
    let (status, answered) = call_as(&router, &key_id, "GET", &path, None).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (200, "<none>"),
        "case \"the account's own stored point\": expected the read to answer it, got \
         {status} {answered:?}"
    );
    assert_eq!(
        string_at(&answered, "RecoveryPointArn"),
        stored,
        "case \"the account's own stored point\": expected the point the request named, \
         got {answered:?}"
    );

    let absent = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist");
    let path = format!("/backup-vaults/{VAULT}/recovery-points/{absent}");
    let (status, refused) = call_as(&router, &key_id, "GET", &path, None).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceNotFoundException"),
        "case \"an own-account ARN naming nothing\": expected the read's not-found \
         refusal, got {status} {refused:?}"
    );
    assert!(
        !refused.to_string().contains("not authorized to perform"),
        "case \"an own-account ARN naming nothing\": expected no authorization refusal, \
         got {refused:?}"
    );
}
