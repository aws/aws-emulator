// SPDX-License-Identifier: CC0-1.0

//! WHO may ask AWS Backup to START a job, driven end to end over the real wire path (a
//! `restJson1` request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: authorization is decided at the IAM layer ahead of the request's
//! own input, so a call that starts a job evaluates the identity-based policies in force on
//! the principal its credentials name against `backup:<OperationName>` on the resource the
//! request ADDRESSES — the vault a backup is taken into, the recovery point a copy or a
//! restore is of — BEFORE it reads or writes anything: a request no statement allows (IAM's
//! default deny), or one an explicit `Deny` covers, is refused with the access-denied class
//! AWS Backup answers such a caller with and performs NOTHING, while a request a matching
//! `Allow` covers reaches the operation's own answer. The refusal names the principal that
//! signed, the action it asked for, the resource it addressed and which of IAM's rules
//! withheld the grant.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test are
//! the ones a client's `iam:PutRolePolicy` stores and the principal a refusal blames is the
//! one `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (the backup start, which
//! addresses a vault, and the copy and restore starts, which address a recovery point), the
//! document in force (none at all, another action of this service, another service's
//! actions, an explicit deny beside an allow, an allow of exactly the action, of the action
//! in another case, of the whole service, of everything, one scoped to the resource
//! addressed and one scoped to another resource) and the principal (an assumed-role session
//! and the account's own published credential, which holds no IAM policies).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{error_code, string_at, ACCOUNT, REGION};

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
pub(crate) const ACCOUNT_KEY_ID: &str = "AKIDTESTONLY";

/// The access key id of the assumed-role session the cases sign with.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "backup-operator";
const SESSION: &str = "backup-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The vault every case addresses, and the resource its backups protect.
const VAULT: &str = "caller-authorization";

/// The status an access-denied refusal of AWS Backup answers with, and the error shape a
/// client reads it as.
const DENIED_STATUS: u16 = 403;
const DENIED_CODE: &str = "AccessDeniedException";

/// Backup, IAM and STS on ONE shared engine, with the engine kept so a case can seed the
/// role, its policy and the session those services hold.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One Backup request signed with `access_key_id` — what names the principal the request
/// speaks for.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/backup/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        );
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |raw| {
        bytes::Bytes::copy_from_slice(raw.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
pub(crate) fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// The ARN of the vault the cases address.
fn vault_arn() -> String {
    format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{VAULT}")
}

/// The protected resource the cases back up.
fn table_arn() -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/orders")
}

/// The role a job is run with, which this suite never varies: what it tests is the
/// CALLER's own permissions.
fn job_role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
pub(crate) fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEBACKUP0001".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy allowing `actions` on `resource`.
fn allow(actions: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }]
    })
}

/// Every resource, as IAM writes it for a statement that scopes to none.
fn any_resource() -> Value {
    json!("*")
}

/// Create the vault and one completed recovery point in it, as the ACCOUNT's own
/// credential — the setup every case shares, which is not what any case is about.
async fn given_vault_with_point(router: &MultiServiceRouter) -> String {
    // AWS Backup ASSUMES the role a start names before it mints a job, so the role every
    // start of this suite names is provisioned in the IAM mounted beside this service,
    // carrying the action that creates a table's recovery point so the point is written.
    // SOURCE: the per-resource-type grants AWS publishes for the backup role (AWS Backup,
    // *Managed policies*).
    crate::plan_and_selection_refusals::given_backup_job_role(
        router,
        "/service-role/",
        "AWSBackupDefaultServiceRole",
        "dynamodb:CreateBackup",
    )
    .await;
    let (status, created) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "PUT",
        &format!("/backup-vaults/{VAULT}"),
        Some("{}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateBackupVault answered {status} {created} for the account's own credential"
    );
    let (status, started) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "PUT",
        "/backup-jobs",
        Some(
            &json!({
                "BackupVaultName": VAULT,
                "ResourceArn": table_arn(),
                "IamRoleArn": job_role_arn(),
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartBackupJob answered {status} {started} for the account's own credential"
    );
    string_at(&started, "RecoveryPointArn")
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The documents a row puts in force on the role, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    resource: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!("backup:DescribeBackupJob"), any_resource())),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("dynamodb:*"), any_resource())),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of this action on ANOTHER resource of this service",
            Some(allow(
                json!(action),
                json!(format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:elsewhere")),
            )),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "backup:*", "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action), any_resource())),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow
            // is not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()), any_resource())),
            None,
        ),
        (
            "an allow of this action on exactly the resource addressed",
            Some(allow(json!(action), json!(resource))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(
                json!(["dynamodb:CreateBackup", action]),
                any_resource(),
            )),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("backup:*"), any_resource())),
            None,
        ),
        (
            "an allow of every action",
            Some(allow(json!("*"), any_resource())),
            None,
        ),
    ]
}

/// `StartBackupJob` is authorized against the caller's own policies on the VAULT the
/// request names, and a refused call stores no job and no recovery point.
#[tokio::test]
async fn starting_a_backup_is_judged_on_the_vault_the_request_names() {
    let action = "backup:StartBackupJob";
    let resource = vault_arn();
    for (case, document, refusal) in documents_in_force(action, &resource) {
        let (router, engine) = emulator();
        given_session(&engine);
        let _point = given_vault_with_point(&router).await;
        given_role_policy(&engine, document.as_ref());

        let body = json!({
            "BackupVaultName": VAULT,
            "ResourceArn": table_arn(),
            "IamRoleArn": job_role_arn(),
        })
        .to_string();
        let (status, answered) =
            call_as(&router, SESSION_KEY_ID, "PUT", "/backup-jobs", Some(&body)).await;

        let Some(refusal) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the operation's own answer, got {status} {answered}"
            );
            continue;
        };
        assert_eq!(
            status, DENIED_STATUS,
            "case {case:?}: expected {DENIED_STATUS}, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            DENIED_CODE,
            "case {case:?}: expected {DENIED_CODE}, got {answered}"
        );
        let expected = format!(
            "User: {} is not authorized to perform: {action} on resource: {resource} {}",
            session_arn(),
            refusal.reason(action)
        );
        assert_eq!(
            answered["Message"],
            json!(expected),
            "case {case:?}: expected IAM's own sentence, got {answered}"
        );
        // A refused call performs NOTHING: the vault still holds only the point the
        // shared setup wrote, and no second job was stored.
        let (status, listed) =
            call_as(&router, ACCOUNT_KEY_ID, "GET", "/backup-jobs", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListBackupJobs answered {status} {listed}"
        );
        let jobs = listed["BackupJobs"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            jobs.len(),
            1,
            "case {case:?}: expected the refused call to store no job, got {listed}"
        );
    }
}

/// One row of the second table: a start operation that addresses a RECOVERY POINT, the
/// request it is given and the action it is judged on.
struct PointCase {
    case: &'static str,
    action: &'static str,
    path: &'static str,
    /// Built from the recovery point the shared setup wrote.
    body: fn(&str) -> Value,
}

/// The two start operations that address a recovery point rather than a vault.
fn point_cases() -> Vec<PointCase> {
    vec![
        PointCase {
            case: "starting a copy of a recovery point",
            action: "backup:StartCopyJob",
            path: "/copy-jobs",
            body: |point| {
                json!({
                    "RecoveryPointArn": point,
                    "SourceBackupVaultName": VAULT,
                    "DestinationBackupVaultArn":
                        format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{VAULT}"),
                    "IamRoleArn": job_role_arn(),
                })
            },
        },
        PointCase {
            case: "starting a restore from a recovery point",
            action: "backup:StartRestoreJob",
            path: "/restore-jobs",
            body: |point| {
                json!({
                    "RecoveryPointArn": point,
                    "IamRoleArn": job_role_arn(),
                    "Metadata": {},
                })
            },
        },
    ]
}

/// The copy and restore starts are judged on the RECOVERY POINT the request names: a
/// caller no statement allows is refused with IAM's own sentence over that point, and one
/// allowed the action reaches the operation's own answer.
#[tokio::test]
async fn starting_a_copy_or_a_restore_is_judged_on_the_point_the_request_names() {
    for row in point_cases() {
        let case = row.case;
        for (document_case, document, denied) in [
            ("no policy at all", None, true),
            (
                "an allow of exactly this action",
                Some(allow(json!(row.action), any_resource())),
                false,
            ),
        ] {
            let (router, engine) = emulator();
            given_session(&engine);
            let point = given_vault_with_point(&router).await;
            given_role_policy(&engine, document.as_ref());

            let body = (row.body)(&point).to_string();
            let (status, answered) =
                call_as(&router, SESSION_KEY_ID, "PUT", row.path, Some(&body)).await;
            if !denied {
                assert_ne!(
                    status, DENIED_STATUS,
                    "case {case:?} with {document_case:?}: expected the operation's own \
                     answer, got {status} {answered}"
                );
                continue;
            }
            assert_eq!(
                status, DENIED_STATUS,
                "case {case:?} with {document_case:?}: expected {DENIED_STATUS}, got \
                 {status} {answered}"
            );
            let expected = format!(
                "User: {} is not authorized to perform: {} on resource: {point} {}",
                session_arn(),
                row.action,
                Refusal::DefaultDeny.reason(row.action)
            );
            assert_eq!(
                answered["Message"],
                json!(expected),
                "case {case:?} with {document_case:?}: expected IAM's own sentence, got \
                 {answered}"
            );
        }
    }
}

/// The account's own published credential holds no IAM policies, so it is not bounded by
/// this gate: the same call the session is refused is served for it.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bounded_by_the_gate() {
    let case = "the account's own published credential, with no policy on the role";
    let (router, engine) = emulator();
    given_session(&engine);
    let _point = given_vault_with_point(&router).await;
    given_role_policy(&engine, None);

    let body = json!({
        "BackupVaultName": VAULT,
        "ResourceArn": table_arn(),
        "IamRoleArn": job_role_arn(),
    })
    .to_string();
    let (status, answered) =
        call_as(&router, ACCOUNT_KEY_ID, "PUT", "/backup-jobs", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the operation's own answer, got {status} {answered}"
    );
    let (status, refused) =
        call_as(&router, SESSION_KEY_ID, "PUT", "/backup-jobs", Some(&body)).await;
    assert_eq!(
        status, DENIED_STATUS,
        "case {case:?}: expected the session to be refused the same call, got {status} \
         {refused}"
    );
}
