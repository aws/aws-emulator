// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's signed operations, over the awsJson1_1 wire.
//!
//! The rule under test: every operation of this API that the model requires a SigV4
//! signature for is called with IAM credentials and is served only to a caller whose
//! identity-based IAM policies allow THAT operation's own action
//! (`cognito-idp:<OperationName>`) on the resource the request addresses — the user pool's
//! ARN for a request that names a `UserPoolId`, the `ResourceArn` a tagging call addresses,
//! and the account's user pools in the request's Region for a call that addresses no single
//! pool. Every other caller is refused with the AWS-common `AccessDeniedException` BEFORE
//! the operation reads or writes anything, so a refused caller learns nothing about what the
//! account holds and a refused mutation changes nothing. The operations the model marks
//! `@optionalAuth` — the unauthenticated and access-token surface — are not judged: such a
//! request carries no IAM credential and speaks for no principal.
//!
//! The decision is the real one: the principals below are made with IAM's OWN operations
//! over the same engine (`CreateUser` + `PutUserPolicy` / `CreatePolicy` +
//! `AttachUserPolicy` / `PutUserPermissionsBoundary` + `CreateAccessKey`), so the documents
//! this service reads are the ones IAM wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (each with its own action and its own resource — a pool read, a pool
//! listing that names no pool, a tag read addressed by ARN, a create that mutates), what the
//! caller's policies say (nothing, an `Allow` for this action on this pool, a service-wide
//! `Allow`, an `Allow` for another action, an `Allow` on another pool, an explicit `Deny`
//! beside a wildcard `Allow`, a boundary that withholds the grant) and where those policies
//! LIVE (inline on the user, in an attached managed policy, in a permissions boundary).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every call below is signed for, so the ARNs the decisions are made about are
/// the ones this Region's pools carry.
const REGION: &str = "us-west-2";

/// The access-key id standing for the account's own published credential: IAM holds no
/// record of it, so it resolves to the account's own identity, which carries no IAM policies
/// and is allowed — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// The wire shape of an authorization refusal: the status this endpoint answers every
/// refusal of the caller under, and the AWS-common error code a client branches on.
const ACCESS_DENIED: (u16, &str) = (400, "AccessDeniedException");

/// One emulator: a single shared engine carrying this service beside the IAM service the
/// principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
    /// The ONE shared engine both services are mounted on — also the engine carrying the
    /// account's session directory, which is where the service that MINTS a temporary
    /// credential records the session it belongs to and where this gate reads it back
    /// ([`Emulator::session`]).
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    async fn call(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityProviderService.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     cognito-idp/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// One awsQuery setup call to IAM, signed with the account's own credential.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
    }

    /// Create a user pool with the account's own credential and answer its id and the ARN
    /// this service reports for it — the ARN a policy statement about that pool names.
    async fn pool(&self, name: &str) -> (String, String) {
        let (status, created) = self
            .call(ACCOUNT_KEY, "CreateUserPool", &json!({ "PoolName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the setup create must succeed, got {status} with {created}"
        );
        let pool = &created["UserPool"];
        (
            pool["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateUserPool answers the pool's Id: {created}"))
                .to_string(),
            pool["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateUserPool answers the pool's Arn: {created}"))
                .to_string(),
        )
    }

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint a
    /// long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .iam("CreateUser", &[("UserName", user_name.to_string())])
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.iam(
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let minted = self
            .iam("CreateAccessKey", &[("UserName", user_name.to_string())])
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// Create a customer-managed policy carrying `document` and answer the ARN IAM minted for
    /// it — the handle an attachment or a boundary names.
    async fn managed_policy(&self, policy_name: &str, document: &str) -> String {
        let created = self
            .iam(
                "CreatePolicy",
                &[
                    ("PolicyName", policy_name.to_string()),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy answers the policy's Arn, got {created}"))
    }

    /// Attach the managed policy `policy_arn` to the user `user_name`.
    async fn attach_user_policy(&self, user_name: &str, policy_arn: &str) {
        self.iam(
            "AttachUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyArn", policy_arn.to_string()),
            ],
        )
        .await;
    }

    /// Set the managed policy `policy_arn` as the permissions boundary of `user_name`.
    async fn put_user_boundary(&self, user_name: &str, policy_arn: &str) {
        self.iam(
            "PutUserPermissionsBoundary",
            &[
                ("UserName", user_name.to_string()),
                ("PermissionsBoundary", policy_arn.to_string()),
            ],
        )
        .await;
    }

    /// Create an IAM role under `path` carrying `policy` as its inline policy (when one is
    /// given), open a SESSION of it, and answer the access key id that session is presented
    /// by with the session's own ARN — the principal a refusal of such a call must blame.
    ///
    /// The role and its documents are IAM's OWN records, written through IAM's own
    /// operations. The SESSION is recorded where the service that mints a temporary credential
    /// records every session it hands out — the account's shared session directory in this
    /// same engine, keyed by the access key id the credential is presented under — which is
    /// the one place a serving service can read which principal a temporary credential speaks
    /// for. Written here rather than minted through the token service, because that service
    /// depends on THIS one.
    ///
    /// `path` is carried so the role is held under an ARN that is NOT its session's: an
    /// assumed-role session ARN carries no path, so a decision taken on the session ARN alone
    /// would find no document at all.
    async fn role_session(
        &self,
        role_name: &str,
        path: &str,
        session_name: &str,
        policies: &[String],
    ) -> Principal {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {
                    "AWS": format!("arn:aws:iam::{}:root", ::engine::emulator_account_id()),
                },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let created = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("Path", path.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        let role_arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateRole answers the role's Arn, got {created}"));
        for (index, policy) in policies.iter().enumerate() {
            self.iam(
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", format!("inline-{index}")),
                    ("PolicyDocument", policy.clone()),
                ],
            )
            .await;
        }
        self.session(role_name, session_name, Some(role_arn))
    }

    /// Record a session of `role_arn` (or of nothing at all, for a credential the directory
    /// holds no issuer for) under a fresh access key id, and answer that key id with the
    /// session's ARN.
    fn session(
        &self,
        role_name: &str,
        session_name: &str,
        role_arn: Option<String>,
    ) -> Principal {
        use ::service_common::management_events::{
            RecordedSession, RecordedSessionIssuer, SessionAuditDirectory,
            PRINCIPAL_TYPE_ASSUMED_ROLE, SESSION_ISSUER_TYPE_ROLE,
        };

        let account = ::engine::emulator_account_id();
        // The identity `sts:GetCallerIdentity` reports for such a credential: the session,
        // named for the role and the session name — and carrying no path, which is why the
        // ROLE's own ARN has to be read from the issuer.
        let arn = format!("arn:aws:sts::{account}:assumed-role/{role_name}/{session_name}");
        let access_key_id = format!("ASIA{}", session_name.to_uppercase().replace('-', ""));
        let now = ::std::time::SystemTime::now();
        let session = RecordedSession {
            principal_type: PRINCIPAL_TYPE_ASSUMED_ROLE.to_string(),
            principal_id: format!("AROAEXAMPLEROLEID:{session_name}"),
            arn: arn.clone(),
            account_id: account.to_string(),
            user_name: Some(session_name.to_string()),
            issuer: role_arn.map(|role_arn| RecordedSessionIssuer {
                principal_type: SESSION_ISSUER_TYPE_ROLE.to_string(),
                principal_id: "AROAEXAMPLEROLEID".to_string(),
                arn: role_arn,
                account_id: account.to_string(),
                user_name: role_name.to_string(),
            }),
            source_identity: None,
            created_at: now,
            expires_at: now + ::std::time::Duration::from_secs(3_600),
        };
        ::engine::InMemoryEngine::state::<SessionAuditDirectory>(&self.engine)
            .lock()
            .remember(access_key_id.clone(), session, now);
        Principal { access_key_id, arn }
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the
/// round trip.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
pub(crate) fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The IAM action `operation` is performed under: this service's ARN namespace and the
/// operation's own shape name.
fn action_of(operation: &str) -> String {
    format!("{}:{operation}", cognito_idp::state::SIGNING_NAME)
}

/// Every user pool of the account in [`REGION`], spelled with IAM's own resource wildcard:
/// the resource a call that addresses no single pool is decided against.
fn any_pool_arn() -> String {
    ::engine::emulator_arn(
        cognito_idp::state::SIGNING_NAME,
        REGION,
        &format!("{}/*", cognito_idp::state::USER_POOL_RESOURCE_TYPE),
    )
}

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every action of this service on everything, and explicitly
/// denying `denied`.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"cognito-idp:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The error code a response carries (`__type` in an awsJson envelope), and the empty string
/// for a response that is no error at all.
fn code_of(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The message an error response carries.
fn message_of(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One request of this service, as a row of the tables below drives it: the operation, the
/// body it carries, and the resource the decision about it is taken on.
struct Call {
    operation: &'static str,
    body: Value,
    resource: String,
}

/// The requests every table below is driven over: a pool read, a pool listing that addresses
/// no single pool, a tag read and a tag write addressed by ARN rather than by pool id, and a
/// pool read whose answer is a listing — each with its own action and its own resource.
fn calls(user_pool_id: &str, pool_arn: &str) -> Vec<Call> {
    vec![
        Call {
            operation: "GetCSVHeader",
            body: json!({ "UserPoolId": user_pool_id }),
            resource: pool_arn.to_string(),
        },
        Call {
            operation: "DescribeUserPool",
            body: json!({ "UserPoolId": user_pool_id }),
            resource: pool_arn.to_string(),
        },
        Call {
            operation: "ListGroups",
            body: json!({ "UserPoolId": user_pool_id }),
            resource: pool_arn.to_string(),
        },
        Call {
            operation: "ListUserPools",
            body: json!({ "MaxResults": 10 }),
            resource: any_pool_arn(),
        },
        Call {
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": pool_arn }),
            resource: pool_arn.to_string(),
        },
        Call {
            operation: "TagResource",
            body: json!({ "ResourceArn": pool_arn, "Tags": { "team": "identity" } }),
            resource: pool_arn.to_string(),
        },
    ]
}

/// RULE: a caller whose identity-based policies do not allow the action is refused, and one
/// whose policies do allow it is served — per operation, with the action and the resource
/// that operation's own request names.
#[tokio::test]
async fn a_signed_operation_is_served_only_to_a_caller_iam_allows_it() {
    let emulator = Emulator::new();
    let (user_pool_id, pool_arn) = emulator.pool("authorization-pool").await;
    let (_other_id, other_pool_arn) = emulator.pool("another-pool").await;

    let no_policy = emulator.user("no-policy", None).await;
    let service_wide = emulator
        .user("service-wide", Some(&allow("cognito-idp:*", "*")))
        .await;
    let other_pool = emulator
        .user(
            "other-pool",
            Some(&allow("cognito-idp:*", &other_pool_arn)),
        )
        .await;

    for call in calls(&user_pool_id, &pool_arn) {
        let action = action_of(call.operation);
        // A grant for exactly this operation's action on exactly the resource its request
        // addresses: a fresh principal per row, because each row's action differs.
        let exact = emulator
            .user(
                &format!("exact-{}", call.operation.to_lowercase()),
                Some(&allow(&action, &call.resource)),
            )
            .await;
        // A grant for another action of this service on the same resource.
        let another_action = emulator
            .user(
                &format!("other-action-{}", call.operation.to_lowercase()),
                Some(&allow(&action_of("DeleteUserPool"), &call.resource)),
            )
            .await;
        // Every action of this service allowed, with THIS one explicitly denied.
        let denied = emulator
            .user(
                &format!("denied-{}", call.operation.to_lowercase()),
                Some(&allow_all_but(&action)),
            )
            .await;

        // (case, the key the call is signed with, whether the call must be served)
        let rows: &[(&str, &str, bool)] = &[
            ("the account's own credential", ACCOUNT_KEY, true),
            ("a principal with no policy at all", &no_policy.access_key_id, false),
            ("a grant for this action on this resource", &exact.access_key_id, true),
            ("a service-wide grant", &service_wide.access_key_id, true),
            (
                "a grant for another action of this service",
                &another_action.access_key_id,
                false,
            ),
            ("a grant on another pool", &other_pool.access_key_id, false),
            (
                "a wildcard grant with this action explicitly denied",
                &denied.access_key_id,
                false,
            ),
        ];

        for (case, access_key_id, served) in rows {
            let (status, body) = emulator
                .call(access_key_id, call.operation, &call.body)
                .await;
            let code = code_of(&body);
            if *served {
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {case:?} calling {}: expected the call to be served, got {status} \
                     with {body}",
                    call.operation
                );
            } else {
                assert_eq!(
                    (status, code.as_str()),
                    ACCESS_DENIED,
                    "case {case:?} calling {}: expected {ACCESS_DENIED:?}, got {status} \
                     with {body}",
                    call.operation
                );
            }
        }
    }
}

/// RULE: a grant the principal's permissions boundary does not also allow is not a
/// permission it has, whether the grant is inline or in an attached managed policy.
#[tokio::test]
async fn a_boundary_caps_what_an_identity_based_grant_gives() {
    let emulator = Emulator::new();
    let (user_pool_id, pool_arn) = emulator.pool("boundary-pool").await;
    let header = json!({ "UserPoolId": &user_pool_id });
    let action = action_of("GetCSVHeader");

    // The grant lives in an ATTACHED managed policy here, so the gate is asserted over both
    // places a document can live.
    let attached = emulator.user("attached-grant", None).await;
    let granting = emulator
        .managed_policy("granting", &allow("cognito-idp:*", "*"))
        .await;
    emulator
        .attach_user_policy("attached-grant", &granting)
        .await;
    let (status, body) = emulator
        .call(&attached.access_key_id, "GetCSVHeader", &header)
        .await;
    assert_eq!(
        (status, code_of(&body).as_str()),
        (200, ""),
        "case \"a grant in an attached managed policy\": expected the call to be served, got \
         {status} with {body}"
    );

    // (case, the boundary document, whether the call must be served)
    let narrow = emulator
        .managed_policy("narrow-boundary", &allow(&action_of("DeleteUserPool"), "*"))
        .await;
    let covering = emulator
        .managed_policy("covering-boundary", &allow(&action, &pool_arn))
        .await;
    let rows: &[(&str, &str, bool)] = &[
        (
            "a boundary that does not allow this action",
            &narrow,
            false,
        ),
        ("a boundary that allows this action", &covering, true),
    ];
    for (case, boundary, served) in rows {
        emulator.put_user_boundary("attached-grant", boundary).await;
        let (status, body) = emulator
            .call(&attached.access_key_id, "GetCSVHeader", &header)
            .await;
        let code = code_of(&body);
        let expected: (u16, &str) = if *served { (200, "") } else { ACCESS_DENIED };
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {status} with {body}"
        );
    }
}

/// RULE: the refusal states IAM's own sentence — the principal the credential speaks for,
/// the action asked for, the resource the request addressed and which of IAM's rules refused
/// it — and travels with the status and the error code this endpoint words every refusal of
/// the caller with.
#[tokio::test]
async fn the_refusal_states_iams_sentence_for_the_request_it_refused() {
    let emulator = Emulator::new();
    let (user_pool_id, pool_arn) = emulator.pool("refusal-pool").await;
    let probe = emulator.user("noperm-probe", None).await;
    let boundaried = emulator
        .user("boundaried-probe", Some(&allow("cognito-idp:*", "*")))
        .await;
    let narrow = emulator
        .managed_policy("boundary", &allow(&action_of("DeleteUserPool"), "*"))
        .await;
    emulator
        .put_user_boundary("boundaried-probe", &narrow)
        .await;
    let explicit = emulator
        .user(
            "denied-probe",
            Some(&allow_all_but(&action_of("GetCSVHeader"))),
        )
        .await;

    for call in calls(&user_pool_id, &pool_arn) {
        let action = action_of(call.operation);
        let (status, body) = emulator
            .call(&probe.access_key_id, call.operation, &call.body)
            .await;
        assert_eq!(
            (status, code_of(&body).as_str()),
            ACCESS_DENIED,
            "case {}: expected {ACCESS_DENIED:?}, got {status} with {body}",
            call.operation
        );
        let expected = format!(
            "User: {} is not authorized to perform: {action} on resource: {} because no \
             identity-based policy allows the {action} action",
            probe.arn, call.resource
        );
        assert_eq!(
            message_of(&body),
            expected,
            "case {}: expected {expected:?}, got {:?}",
            call.operation,
            message_of(&body)
        );
    }

    // (case, the key the call is signed with, the principal it speaks for, the clause the
    // refusal must end with)
    let get_csv_header = action_of("GetCSVHeader");
    let rows: &[(&str, &Principal, String)] = &[
        (
            "an explicit deny in an identity-based policy",
            &explicit,
            "with an explicit deny in an identity-based policy".to_string(),
        ),
        (
            "a boundary that withholds the grant",
            &boundaried,
            format!("because no permissions boundary allows the {get_csv_header} action"),
        ),
    ];
    for (case, principal, reason) in rows {
        let (status, body) = emulator
            .call(
                &principal.access_key_id,
                "GetCSVHeader",
                &json!({ "UserPoolId": &user_pool_id }),
            )
            .await;
        assert_eq!(
            (status, code_of(&body).as_str()),
            ACCESS_DENIED,
            "case {case:?}: expected {ACCESS_DENIED:?}, got {status} with {body}"
        );
        let expected = format!(
            "User: {} is not authorized to perform: {get_csv_header} on resource: {pool_arn} \
             {reason}",
            principal.arn
        );
        assert_eq!(
            message_of(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message_of(&body)
        );
    }
}

/// RULE: the decision is taken BEFORE the operation reads or writes anything, so a refused
/// call neither reports what the account holds nor leaves anything behind.
#[tokio::test]
async fn a_refused_call_performs_nothing() {
    let emulator = Emulator::new();
    let (user_pool_id, _pool_arn) = emulator.pool("unchanged-pool").await;
    let probe = emulator.user("noperm-probe", None).await;

    // (case, operation, body, the member a served call would have changed)
    let mutations: &[(&str, &str, Value)] = &[
        (
            "a group create",
            "CreateGroup",
            json!({ "UserPoolId": &user_pool_id, "GroupName": "refused-group" }),
        ),
        (
            "a user create",
            "AdminCreateUser",
            json!({ "UserPoolId": &user_pool_id, "Username": "refused-user" }),
        ),
        (
            "a pool update",
            "UpdateUserPool",
            json!({ "UserPoolId": &user_pool_id, "UserPoolTier": "PLUS" }),
        ),
    ];
    for (case, operation, body) in mutations {
        let (status, refused) = emulator.call(&probe.access_key_id, operation, body).await;
        assert_eq!(
            (status, code_of(&refused).as_str()),
            ACCESS_DENIED,
            "case {case:?}: expected {ACCESS_DENIED:?}, got {status} with {refused}"
        );
    }

    // Read back as the account, whose credential the gate does not bound: nothing the
    // refused calls asked for happened.
    let (_, groups) = emulator
        .call(
            ACCOUNT_KEY,
            "ListGroups",
            &json!({ "UserPoolId": &user_pool_id }),
        )
        .await;
    assert_eq!(
        groups["Groups"],
        json!([]),
        "case \"a group create\": expected the refused create to have left no group, got {groups}"
    );
    let (_, users) = emulator
        .call(
            ACCOUNT_KEY,
            "ListUsers",
            &json!({ "UserPoolId": &user_pool_id }),
        )
        .await;
    assert_eq!(
        users["Users"],
        json!([]),
        "case \"a user create\": expected the refused create to have left no user, got {users}"
    );
    let (_, described) = emulator
        .call(
            ACCOUNT_KEY,
            "DescribeUserPool",
            &json!({ "UserPoolId": &user_pool_id }),
        )
        .await;
    assert_eq!(
        described["UserPool"]["UserPoolTier"],
        json!("ESSENTIALS"),
        "case \"a pool update\": expected the refused update to have changed nothing, got \
         {described}"
    );
}

/// RULE: a request signed with a TEMPORARY credential speaks for the SESSION it was minted
/// for — the principal `sts:GetCallerIdentity` reports for that credential, which is what a
/// refusal blames — and the permissions in force on it are the ISSUING ROLE's, read under the
/// ARN the account holds that role by. So a session of a role whose documents do not allow
/// the action is refused exactly as any other caller is, and one whose role allows it is
/// served.
///
/// The rows span what the rule generalises over: a role carrying no document, a role carrying
/// a grant for ANOTHER service's action alone (the least-privilege session the recorded case
/// is about), a service-wide grant, a grant scoped to the pool the request addresses, a grant
/// beside an explicit deny of the action, and a credential the directory holds no issuer for.
/// Each is driven over two operations, so the decision is the operation's own action rather
/// than one spelled for a single call.
#[tokio::test]
async fn a_session_is_judged_on_the_role_that_issued_it() {
    let emulator = Emulator::new();
    let (user_pool_id, pool_arn) = emulator.pool("session-pool").await;
    let (_other_id, other_pool_arn) = emulator.pool("session-other-pool").await;
    // A PATH on every role, so the role is held under an ARN its session's does not carry:
    // the documents are found only because the issuer's own ARN is what they are read under.
    let path = "/identity/";

    // (case, the documents the issuing role carries, whether the call must be served)
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        ("a role carrying no document at all", vec![], false),
        (
            "a role granting only another service's action",
            vec![allow("sts:GetCallerIdentity", "*")],
            false,
        ),
        (
            "a role with a service-wide grant",
            vec![allow("cognito-idp:*", "*")],
            true,
        ),
        (
            "a role granting this service on the pool addressed",
            vec![allow("cognito-idp:*", &pool_arn)],
            true,
        ),
        (
            "a role granting this service on another pool",
            vec![allow("cognito-idp:*", &other_pool_arn)],
            false,
        ),
    ];

    // Two operations, each with its own action and the same addressed resource.
    for operation in ["GetCSVHeader", "DescribeUserPool"] {
        let action = action_of(operation);
        let body = json!({ "UserPoolId": &user_pool_id });
        for (index, (case, documents, served)) in rows.iter().enumerate() {
            let role_name = format!("session-role-{}-{index}", operation.to_lowercase());
            let session_name = format!("probe-{}-{index}", operation.to_lowercase());
            let principal = emulator
                .role_session(&role_name, path, &session_name, documents)
                .await;
            let (status, answered) = emulator
                .call(&principal.access_key_id, operation, &body)
                .await;
            if *served {
                assert_eq!(
                    (status, code_of(&answered).as_str()),
                    (200, ""),
                    "case {case:?} calling {operation}: expected the session to be served, got \
                     {status} with {answered}"
                );
                continue;
            }
            assert_eq!(
                (status, code_of(&answered).as_str()),
                ACCESS_DENIED,
                "case {case:?} calling {operation}: expected {ACCESS_DENIED:?}, got {status} \
                 with {answered}"
            );
            // The refusal blames the SESSION, not the role and not the account.
            let expected = format!(
                "User: {} is not authorized to perform: {action} on resource: {pool_arn} \
                 because no identity-based policy allows the {action} action",
                principal.arn
            );
            assert_eq!(
                message_of(&answered),
                expected,
                "case {case:?} calling {operation}: expected {expected:?}, got {:?}",
                message_of(&answered)
            );
        }

        // An explicit deny in a document the issuing role carries wins over its own wildcard
        // grant, and the refusal says so.
        let denied = emulator
            .role_session(
                &format!("session-denied-{}", operation.to_lowercase()),
                path,
                &format!("denied-{}", operation.to_lowercase()),
                &[allow_all_but(&action)],
            )
            .await;
        let (status, answered) = emulator.call(&denied.access_key_id, operation, &body).await;
        assert_eq!(
            (status, code_of(&answered).as_str()),
            ACCESS_DENIED,
            "case \"an explicit deny on the issuing role\" calling {operation}: expected \
             {ACCESS_DENIED:?}, got {status} with {answered}"
        );
        let expected = format!(
            "User: {} is not authorized to perform: {action} on resource: {pool_arn} with an \
             explicit deny in an identity-based policy",
            denied.arn
        );
        assert_eq!(
            message_of(&answered),
            expected,
            "case \"an explicit deny on the issuing role\" calling {operation}: expected \
             {expected:?}, got {:?}",
            message_of(&answered)
        );
    }

    // A temporary credential the directory holds no ISSUER for speaks for the session itself,
    // and the account holds no document under a session ARN — so nothing allows the call.
    let issuerless = emulator.session("session-role-0-getcsvheader", "no-issuer", None);
    let (status, answered) = emulator
        .call(
            &issuerless.access_key_id,
            "GetCSVHeader",
            &json!({ "UserPoolId": &user_pool_id }),
        )
        .await;
    assert_eq!(
        (status, code_of(&answered).as_str()),
        ACCESS_DENIED,
        "case \"a session with no issuer\": expected {ACCESS_DENIED:?}, got {status} with \
         {answered}"
    );

    // The gate reads the documents per request, so one the role gains between two calls
    // decides the second: nothing is cached.
    let later = emulator
        .role_session("session-granted-later", path, "later", &[])
        .await;
    let (status, refused) = emulator
        .call(
            &later.access_key_id,
            "GetCSVHeader",
            &json!({ "UserPoolId": &user_pool_id }),
        )
        .await;
    assert_eq!(
        (status, code_of(&refused).as_str()),
        ACCESS_DENIED,
        "case \"before the grant is written\": expected {ACCESS_DENIED:?}, got {status} with \
         {refused}"
    );
    emulator
        .iam(
            "PutRolePolicy",
            &[
                ("RoleName", "session-granted-later".to_string()),
                ("PolicyName", "granted-later".to_string()),
                ("PolicyDocument", allow("cognito-idp:*", "*")),
            ],
        )
        .await;
    let (status, served) = emulator
        .call(
            &later.access_key_id,
            "GetCSVHeader",
            &json!({ "UserPoolId": &user_pool_id }),
        )
        .await;
    assert_eq!(
        (status, code_of(&served).as_str()),
        (200, ""),
        "case \"after the grant is written\": expected the next call to be served, got {status} \
         with {served}"
    );
}

/// RULE: an operation the model marks `@optionalAuth` is not judged here — a request to one
/// carries no IAM credential and speaks for no principal — so a caller IAM allows nothing is
/// answered by the operation itself rather than refused for want of a permission.
#[tokio::test]
async fn an_optional_auth_operation_is_not_judged() {
    let emulator = Emulator::new();
    let probe = emulator.user("noperm-probe", None).await;

    // Every row is an operation the model marks `@optionalAuth`
    // (`smithy_query(query="digest")`), driven with a client id no app client answers to, so
    // the answer is the operation's OWN refusal and never an authorization one.
    let rows: &[(&str, Value)] = &[
        ("SignUp", json!({ "ClientId": "1h57kf5cpparlgpq01cv1mcl", "Username": "u", "Password": "Passw0rd!" })),
        ("InitiateAuth", json!({ "ClientId": "1h57kf5cpparlgpq01cv1mcl", "AuthFlow": "USER_PASSWORD_AUTH" })),
        ("ForgotPassword", json!({ "ClientId": "1h57kf5cpparlgpq01cv1mcl", "Username": "u" })),
        ("GetUser", json!({ "AccessToken": "not-a-token-this-pool-issued" })),
    ];
    for (operation, body) in rows {
        let (status, answered) = emulator.call(&probe.access_key_id, operation, body).await;
        let code = code_of(&answered);
        assert_ne!(
            code, ACCESS_DENIED.1,
            "case {operation:?}: expected the operation's own answer, got {status} with \
             {answered}"
        );
    }
}
