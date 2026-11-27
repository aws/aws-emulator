// SPDX-License-Identifier: CC0-1.0

//! This service's per-request IAM AUTHORIZATION, driven end to end over the wire
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSFreeTierService.<Operation>`).
//!
//! RULE under test: an operation whose model declares `AccessDeniedException` (HTTP 403,
//! "You don't have sufficient access to perform this action" —
//! `smithy_query(query="digest")` lists it for `GetAccountPlanState` and
//! `UpgradeAccountPlan` and for no other operation of this service) serves only a caller
//! whose identity-based IAM policies allow `freetier:<OperationName>` on that operation's
//! own API resource `arn:aws:freetier:us-east-1:<account>:/<OperationName>`, and refuses
//! every other caller with that modelled error BEFORE reading or writing any plan state,
//! in IAM's own sentence:
//!
//! ```text
//! User: <principal ARN> is not authorized to perform: freetier:<Op> on resource:
//! arn:aws:freetier:us-east-1:<account>:/<Op> <reason>
//! ```
//!
//! Everything outside the angle brackets is fixed text, including the reason clause; the
//! principal ARN comes from the credential the request signed with and the account id from
//! the account contract. An operation whose model declares no 403 is NOT judged, because
//! answering a caller with an error its operation does not model would be a different
//! divergence — the last test below pins that.
//!
//! The decision is the real one: every principal below is made with IAM's and STS's OWN
//! operations over the same engine — `CreateUser` + `PutUserPolicy` / `AttachUserPolicy` /
//! `PutUserPermissionsBoundary` + `CreateAccessKey`, and `CreateRole` + `PutRolePolicy` +
//! `AssumeRole` — so the documents this service reads are the ones those services wrote.
//!
//! The rows differ along what the rule generalises over: which of the two gated operations
//! is asked for, which kind of principal asks (the account's own published credential, an
//! IAM user signing with a long-term key, an assumed-role session), WHERE the documents in
//! force live (an inline policy, a customer-managed attachment, an AWS-managed attachment,
//! a permissions boundary on top of them), what those documents say (nothing at all, this
//! action on every resource, this action on this operation's own resource, this action on
//! the OTHER operation's resource, another action, a service-wide wildcard, a bare `*`, and
//! an explicit `Deny` beside a wildcard `Allow`), and which reason clause the refusal
//! therefore carries. The recovery leg — grant the action and call again over the same
//! credential — is a test of its own, as is the modelled length bound of the refusal under
//! the longest principal ARN IAM can spell.
//!
//! The last two tests are about WHEN the decision is taken: AWS takes it at the IAM layer
//! before the request's own input is read, so the answer to an unauthorized caller is that
//! modelled 403 whatever body it sent — a body breaking a modelled constraint of the input
//! shape included — and the answer to an authorized one is still whatever the model says
//! about that body. Their rows span the body side of the rule: well-formed, the `@required`
//! member absent, a value outside the enum, the empty string, a value of another JSON type,
//! a member the shape does not declare, and a body that is not a structure at all.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    ComAmazonawsFreetierAwsfreetierserviceState, PLAN_TYPE_PAID,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the resource ARN of a
/// decision carries: Free Tier is a global API served from this Region.
const REGION: &str = "us-east-1";

/// The `@length` ceiling of `AccessDeniedException$message` — a `GenericString` of
/// `@length(min: 0, max: 1024)` (`smithy_query(query="shape",
/// shape="AccessDeniedException")`). Stated from the MODEL so the expectation is not read
/// from the code under test.
const MESSAGE_MAX_CHARS: usize = 1024;

/// The `@length` ceiling of an IAM `pathType` ("The path to the user", `CreateUser$Path`:
/// `@length(min: 1, max: 512)`), and of a `userNameType` (`@length(min: 1, max: 64)`) —
/// the two things that bound how long a principal ARN a refusal echoes can be.
const MAX_PATH_CHARS: usize = 512;
/// See [`MAX_PATH_CHARS`].
const MAX_USER_NAME_CHARS: usize = 64;

/// An access-key id neither STS nor IAM holds a record for: the account's own published
/// credential, which holds no IAM policies and is allowed, exactly as AWS account root
/// credentials are.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The two operations of this service whose model declares `AccessDeniedException`.
const GATED_OPERATIONS: [&str; 2] = ["GetAccountPlanState", "UpgradeAccountPlan"];

/// The operations whose model declares no `AccessDeniedException`
/// (`smithy_query(query="digest")`), so this gate does not judge them.
const UNGATED_OPERATIONS: [&str; 3] = [
    "GetAccountActivity",
    "ListAccountActivities",
    "GetFreeTierUsage",
];

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with — the same assembly
/// `core/router`'s `build_router` performs, narrowed to what this rule touches.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router whose account already holds plan data and a vetted payment instrument, so
    /// a caller the gate ADMITS is answered a success by both gated operations rather than
    /// by a precondition refusal.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests sign nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_freetier_awsfreetierservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        engine
            .state::<ComAmazonawsFreetierAwsfreetierserviceState>()
            .lock()
            .vet_payment_instrument(engine::emulator_account_id());
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope and
    /// the request body each operation's model requires of it.
    async fn call(&self, access_key_id: &str, operation: &str) -> (u16, Value) {
        let body = match operation {
            "UpgradeAccountPlan" => json!({ "accountPlanType": PLAN_TYPE_PAID }),
            _ => json!({}),
        };
        self.call_with_body(access_key_id, operation, &body).await
    }

    /// One call to this service carrying `body`, with `access_key_id` in the SigV4
    /// credential scope.
    async fn call_with_body(
        &self,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://freetier.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AWSFreeTierService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     freetier/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// What the account's plan state is right now, read as the ambient caller — so a
    /// refused call that touched the record would show up here whatever it wrote.
    async fn plan_snapshot(&self) -> (u16, Value) {
        self.call(ACCOUNT_KEY, "GetAccountPlanState").await
    }

    /// One awsQuery call to IAM or STS, signed as the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the awsQuery request builds");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One IAM call that must succeed.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let (status, body) = self.query("iam", "2010-05-08", action, params).await;
        assert_eq!(
            status, 200,
            "case {action} {params:?}: the setup call must succeed, got {status} {body}"
        );
        body
    }

    /// Create an IAM user under `path` and answer its long-term access key id.
    async fn user_key_at(&self, user_name: &str, path: &str) -> String {
        self.iam(
            "CreateUser",
            &[
                ("UserName", user_name.to_string()),
                ("Path", path.to_string()),
            ],
        )
        .await;
        let minted = self
            .iam("CreateAccessKey", &[("UserName", user_name.to_string())])
            .await;
        xml_element(&minted, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {minted}"))
    }

    /// Create an IAM user at the account root carrying `policy` as its inline policy (when
    /// one is given), and answer its long-term access key id.
    async fn user_key(&self, user_name: &str, policy: Option<&str>) -> String {
        let key = self.user_key_at(user_name, "/").await;
        if let Some(policy) = policy {
            self.put_user_policy(user_name, policy).await;
        }
        key
    }

    /// Put (or replace) the inline policy of `user_name`.
    async fn put_user_policy(&self, user_name: &str, policy: &str) {
        self.iam(
            "PutUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", INLINE_POLICY_NAME.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }

    /// Delete the inline policy of `user_name`, so the grant it carried is gone.
    async fn delete_user_policy(&self, user_name: &str) {
        self.iam(
            "DeleteUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", INLINE_POLICY_NAME.to_string()),
            ],
        )
        .await;
    }

    /// Create a customer-managed policy carrying `document` and answer its ARN.
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
            .unwrap_or_else(|| panic!("CreatePolicy must answer the policy's Arn, got {created}"))
    }

    /// Attach the managed policy `policy_arn` to `user_name`.
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

    /// Set the permissions boundary of `user_name` to the managed policy `policy_arn`.
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

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the session's access key id.
    async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            engine::emulator_account_id()
        );
        self.iam(
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.iam(
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", INLINE_POLICY_NAME.to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let (status, assumed) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            engine::emulator_account_id()
                        ),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {role_name}: assuming the role must succeed, got {status} {assumed}"
        );
        xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {assumed}"))
    }

    /// The principal ARN `sts:GetCallerIdentity` reports for `access_key_id` — the value a
    /// refusal must name, taken from STS rather than from the service under test.
    async fn caller_arn(&self, access_key_id: &str) -> String {
        let body = "Action=GetCallerIdentity&Version=2011-06-15".to_string();
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sts.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     sts/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the awsQuery request builds");
        let response = self.router.dispatch(request).await;
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {access_key_id}: GetCallerIdentity must answer, got {body}"
        );
        xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("GetCallerIdentity must answer an Arn, got {body}"))
    }
}

/// The name every inline policy this suite writes is stored under.
const INLINE_POLICY_NAME: &str = "inline";

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
fn form_encode(value: &str) -> String {
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
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every Free Tier action on everything, with an explicit
/// `Deny` of `denied` beside it — so the deny decides, whichever order they are read in.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"freetier:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The IAM action `operation` is authorized under, spelled from the IAM Service
/// Authorization Reference's `freetier:<OperationName>` grammar rather than read off the
/// code under test.
fn action_of(operation: &str) -> String {
    format!("freetier:{operation}")
}

/// The resource a decision about `operation` is taken against, spelled from the ARN
/// grammar: `arn:aws:freetier:us-east-1:<account>:/<OperationName>`.
fn resource_of(operation: &str) -> String {
    format!(
        "arn:aws:freetier:{REGION}:{}:/{operation}",
        engine::emulator_account_id()
    )
}

/// The other gated operation — the one a policy scoped to this operation must NOT grant.
fn other_gated(operation: &str) -> &'static str {
    if operation == GATED_OPERATIONS[0] {
        GATED_OPERATIONS[1]
    } else {
        GATED_OPERATIONS[0]
    }
}

/// The wire error code (`__type`) of an error body, without its namespace.
fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The refusal's `message`, under either casing the JSON envelopes use.
fn message_of(payload: &Value) -> String {
    payload["message"]
        .as_str()
        .or_else(|| payload["Message"].as_str())
        .unwrap_or_else(|| panic!("a refusal must carry a message, got {payload}"))
        .to_string()
}

/// Why a refusal happened, as IAM's sentence states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// Nothing the principal holds allows the action.
    NothingAllows,
    /// A readable document explicitly denies it.
    ExplicitDeny,
    /// The permissions boundary in force does not allow it.
    BoundaryWithholds,
}

impl Reason {
    /// The clause the refusal carries for `action`.
    fn clause(self, action: &str) -> String {
        match self {
            Self::NothingAllows => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::BoundaryWithholds => {
                format!("because no permissions boundary allows the {action} action")
            }
        }
    }
}

/// What the gate must do with one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The call is authorized, so the operation answers it.
    Served,
    /// The call is refused with the modelled 403 and this reason.
    Refused(Reason),
}

/// Assert one answer against what the rule requires of it.
fn check(case: &str, operation: &str, expect: Expect, status: u16, payload: &Value) {
    let action = action_of(operation);
    match expect {
        Expect::Served => {
            assert_eq!(
                (status, error_code(payload).as_str()),
                (200, ""),
                "case {case:?}: an authorized caller must be served {operation}, got {status} \
                 with {payload}"
            );
        }
        Expect::Refused(reason) => {
            assert_eq!(
                (status, error_code(payload).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: an unauthorized caller must get the modelled 403 on \
                 {operation}, got {status} with {payload}"
            );
            let message = message_of(payload);
            assert!(
                message.chars().count() <= MESSAGE_MAX_CHARS,
                "case {case:?}: the message must satisfy the modelled @length(0, \
                 {MESSAGE_MAX_CHARS}), got {} characters: {message:?}",
                message.chars().count()
            );
            assert!(
                message.contains(&format!(
                    "is not authorized to perform: {action} on resource: {} {}",
                    resource_of(operation),
                    reason.clause(&action)
                )),
                "case {case:?}: expected IAM's sentence about {action}, got {message:?}"
            );
        }
    }
}

/// RULE: both gated operations serve only a caller whose identity-based documents allow
/// that operation's action on that operation's resource, over every kind of principal and
/// every place those documents can live — and a refusal is the modelled 403 stating why.
#[tokio::test]
async fn a_gated_operation_is_served_only_to_a_principal_whose_policy_allows_it() {
    let emulator = Emulator::new();
    // The account's own credential establishes the plan record, so a caller the gate
    // ADMITS is answered a success rather than the no-plan-data refusal.
    let (status, payload) = emulator.call(ACCOUNT_KEY, "UpgradeAccountPlan").await;
    assert_eq!(
        status, 200,
        "the account's own credential must be served the upgrade, got {status} with {payload}"
    );

    for operation in GATED_OPERATIONS {
        let action = action_of(operation);
        let suffix = operation.to_lowercase();
        let managed_allow = emulator
            .managed_policy(
                &format!("freetier-managed-allow-{suffix}"),
                &allow(&action, "*"),
            )
            .await;
        // A boundary that allows a DIFFERENT Free Tier action: a ceiling that does not
        // cover this one, whatever the principal's own policies grant.
        let narrow_boundary = emulator
            .managed_policy(
                &format!("freetier-boundary-other-{suffix}"),
                &allow(&action_of(other_gated(operation)), "*"),
            )
            .await;
        let wide_boundary = emulator
            .managed_policy(
                &format!("freetier-boundary-wide-{suffix}"),
                &allow("freetier:*", "*"),
            )
            .await;

        // (case, access key id, what the rule requires)
        let mut rows: Vec<(String, String, Expect)> = vec![
            (
                "the account's own published credential".to_string(),
                ACCOUNT_KEY.to_string(),
                Expect::Served,
            ),
            (
                "an IAM user with no policy at all".to_string(),
                emulator
                    .user_key(&format!("freetier-bare-{suffix}"), None)
                    .await,
                Expect::Refused(Reason::NothingAllows),
            ),
            (
                "an IAM user allowed this action on every resource".to_string(),
                emulator
                    .user_key(
                        &format!("freetier-allow-any-{suffix}"),
                        Some(&allow(&action, "*")),
                    )
                    .await,
                Expect::Served,
            ),
            (
                "an IAM user allowed this action on this operation's own resource".to_string(),
                emulator
                    .user_key(
                        &format!("freetier-allow-exact-{suffix}"),
                        Some(&allow(&action, &resource_of(operation))),
                    )
                    .await,
                Expect::Served,
            ),
            (
                "an IAM user allowed this action on the OTHER operation's resource".to_string(),
                emulator
                    .user_key(
                        &format!("freetier-allow-other-resource-{suffix}"),
                        Some(&allow(&action, &resource_of(other_gated(operation)))),
                    )
                    .await,
                Expect::Refused(Reason::NothingAllows),
            ),
            (
                "an IAM user allowed only the OTHER Free Tier action".to_string(),
                emulator
                    .user_key(
                        &format!("freetier-allow-other-action-{suffix}"),
                        Some(&allow(&action_of(other_gated(operation)), "*")),
                    )
                    .await,
                Expect::Refused(Reason::NothingAllows),
            ),
            (
                "an IAM user allowed every action of another service".to_string(),
                emulator
                    .user_key(
                        &format!("freetier-allow-elsewhere-{suffix}"),
                        Some(&allow("s3:*", "*")),
                    )
                    .await,
                Expect::Refused(Reason::NothingAllows),
            ),
            (
                "an IAM user allowed every Free Tier action".to_string(),
                emulator
                    .user_key(
                        &format!("freetier-allow-service-{suffix}"),
                        Some(&allow("freetier:*", "*")),
                    )
                    .await,
                Expect::Served,
            ),
            (
                "an IAM user allowed every action there is".to_string(),
                emulator
                    .user_key(&format!("freetier-allow-star-{suffix}"), Some(&allow("*", "*")))
                    .await,
                Expect::Served,
            ),
            (
                "an IAM user whose wildcard allow carries an explicit deny of this action"
                    .to_string(),
                emulator
                    .user_key(
                        &format!("freetier-denied-{suffix}"),
                        Some(&allow_all_but(&action)),
                    )
                    .await,
                Expect::Refused(Reason::ExplicitDeny),
            ),
            (
                "an assumed-role session with no policy".to_string(),
                emulator
                    .assumed_role_key(&format!("freetier-role-bare-{suffix}"), None)
                    .await,
                Expect::Refused(Reason::NothingAllows),
            ),
            (
                "an assumed-role session whose role allows this action".to_string(),
                emulator
                    .assumed_role_key(
                        &format!("freetier-role-allow-{suffix}"),
                        Some(&allow(&action, "*")),
                    )
                    .await,
                Expect::Served,
            ),
            (
                "an assumed-role session whose role is explicitly denied this action".to_string(),
                emulator
                    .assumed_role_key(
                        &format!("freetier-role-denied-{suffix}"),
                        Some(&allow_all_but(&action)),
                    )
                    .await,
                Expect::Refused(Reason::ExplicitDeny),
            ),
        ];

        // A customer-managed policy ATTACHED to the user: the same grant, held somewhere
        // else, so the gate must read both sources.
        let attached = format!("freetier-attached-{suffix}");
        let attached_key = emulator.user_key(&attached, None).await;
        emulator.attach_user_policy(&attached, &managed_allow).await;
        rows.push((
            "an IAM user carrying the grant in an attached customer-managed policy".to_string(),
            attached_key,
            Expect::Served,
        ));

        // An AWS-MANAGED policy attached: a document AWS owns, which this emulator carries
        // in IAM's own catalog, so what it grants is read the same way.
        let aws_managed = format!("freetier-aws-managed-{suffix}");
        let aws_managed_key = emulator.user_key(&aws_managed, None).await;
        emulator
            .attach_user_policy(&aws_managed, "arn:aws:iam::aws:policy/AdministratorAccess")
            .await;
        rows.push((
            "an IAM user carrying AWS's own AdministratorAccess policy".to_string(),
            aws_managed_key,
            Expect::Served,
        ));

        // A permissions boundary caps what the identity-based policies can grant: one that
        // does not cover this action withholds the grant, one that does leaves it standing.
        let capped = format!("freetier-capped-{suffix}");
        let capped_key = emulator.user_key(&capped, Some(&allow(&action, "*"))).await;
        emulator.put_user_boundary(&capped, &narrow_boundary).await;
        rows.push((
            "an IAM user granted this action under a boundary that does not allow it".to_string(),
            capped_key,
            Expect::Refused(Reason::BoundaryWithholds),
        ));

        let within = format!("freetier-within-boundary-{suffix}");
        let within_key = emulator.user_key(&within, Some(&allow(&action, "*"))).await;
        emulator.put_user_boundary(&within, &wide_boundary).await;
        rows.push((
            "an IAM user granted this action under a boundary that allows it".to_string(),
            within_key,
            Expect::Served,
        ));

        for (case, access_key_id, expect) in &rows {
            let case = format!("{case} asking {operation}");
            let before = emulator.plan_snapshot().await;
            let (status, payload) = emulator.call(access_key_id, operation).await;
            check(&case, operation, *expect, status, &payload);
            if let Expect::Refused(_) = expect {
                // The decision is taken BEFORE anything is read or written, so a refused
                // call leaves the account's plan record exactly as it was.
                let after = emulator.plan_snapshot().await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused call must change no plan state, got {after:?} \
                     after {before:?}"
                );
                // A refusal names the principal the credential speaks for, as STS reports
                // it for that same credential.
                let principal = emulator.caller_arn(access_key_id).await;
                assert_eq!(
                    message_of(&payload),
                    format!(
                        "User: {principal} is not authorized to perform: {} on resource: {} {}",
                        action_of(operation),
                        resource_of(operation),
                        match expect {
                            Expect::Refused(reason) => reason.clause(&action_of(operation)),
                            Expect::Served => unreachable!("this branch is a refusal"),
                        }
                    ),
                    "case {case:?}: expected IAM's whole sentence, got {payload}"
                );
            }
        }
    }
}

/// RULE, its recovery leg: the denial lifts the moment a policy grants the action to that
/// same principal, over the same credential, and comes back when the grant is removed —
/// so the gate reads the documents in force at the instant of the call.
#[tokio::test]
async fn granting_the_action_lifts_the_denial_on_the_same_credential() {
    let emulator = Emulator::new();
    let (status, payload) = emulator.call(ACCOUNT_KEY, "UpgradeAccountPlan").await;
    assert_eq!(
        status, 200,
        "the account's own credential must establish the plan record, got {status} with {payload}"
    );

    for operation in GATED_OPERATIONS {
        let user = format!("freetier-recovery-{}", operation.to_lowercase());
        let key = emulator.user_key(&user, None).await;

        let (status, payload) = emulator.call(&key, operation).await;
        check(
            &format!("before any grant, {operation}"),
            operation,
            Expect::Refused(Reason::NothingAllows),
            status,
            &payload,
        );

        emulator
            .put_user_policy(&user, &allow(&action_of(operation), "*"))
            .await;
        let (status, payload) = emulator.call(&key, operation).await;
        check(
            &format!("after the grant, {operation}"),
            operation,
            Expect::Served,
            status,
            &payload,
        );

        emulator.delete_user_policy(&user).await;
        let (status, payload) = emulator.call(&key, operation).await;
        check(
            &format!("after the grant is removed, {operation}"),
            operation,
            Expect::Refused(Reason::NothingAllows),
            status,
            &payload,
        );
    }
}

/// RULE: the refusal's `message` stays inside the modelled `@length(0, 1024)` and keeps
/// IAM's whole sentence even for the longest principal ARN IAM can spell — a user at a
/// `pathType` of 512 characters with a `userNameType` of 64.
#[tokio::test]
async fn the_refusal_states_the_whole_sentence_for_the_longest_principal_arn() {
    let emulator = Emulator::new();

    // (case, the path the user sits under, the length of its name)
    let rows: [(&str, String, usize); 3] = [
        ("a user at the account root", "/".to_string(), 1),
        (
            "a user at an ordinary path",
            "/freetier/probes/".to_string(),
            MAX_USER_NAME_CHARS / 2,
        ),
        (
            "a user at the longest path IAM admits, with the longest name",
            format!("/{}/", "p".repeat(MAX_PATH_CHARS - 2)),
            MAX_USER_NAME_CHARS,
        ),
    ];

    for (index, (case, path, name_chars)) in rows.iter().enumerate() {
        assert!(
            path.chars().count() <= MAX_PATH_CHARS && *name_chars <= MAX_USER_NAME_CHARS,
            "case {case:?}: the probe must stay inside IAM's own bounds, got a path of {} \
             characters and a name of {name_chars}",
            path.chars().count()
        );
        // The name is unique per row and exactly `name_chars` long.
        let suffix = format!("-{index}");
        let user = format!(
            "{}{suffix}",
            "u".repeat(name_chars.saturating_sub(suffix.len()).max(1))
        );
        let key = emulator.user_key_at(&user, path).await;
        let principal = emulator.caller_arn(&key).await;

        for operation in GATED_OPERATIONS {
            let action = action_of(operation);
            let (status, payload) = emulator.call(&key, operation).await;
            let case = format!("{case} asking {operation}");
            check(
                &case,
                operation,
                Expect::Refused(Reason::NothingAllows),
                status,
                &payload,
            );
            let message = message_of(&payload);
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: {} {}",
                    resource_of(operation),
                    Reason::NothingAllows.clause(&action)
                ),
                "case {case:?}: expected IAM's whole sentence, got {message:?}"
            );
        }
    }
}

/// RULE's boundary: an operation whose model declares no `AccessDeniedException` is NOT
/// judged by this gate — answering a caller with an error its operation does not model
/// would be a different divergence — so a principal with no policy at all is still served
/// by every one of them.
#[tokio::test]
async fn an_operation_that_models_no_denial_is_not_judged() {
    let emulator = Emulator::new();
    let key = emulator.user_key("freetier-ungated-probe", None).await;

    for operation in UNGATED_OPERATIONS {
        // `GetAccountActivity` needs an activity to address; the listing reports one.
        let (status, payload) = if operation == "GetAccountActivity" {
            let (status, listed) = emulator.call(&key, "ListAccountActivities").await;
            assert_eq!(
                status, 200,
                "case {operation}: the listing must answer, got {status} with {listed}"
            );
            let activity_id = listed["activities"][0]["activityId"]
                .as_str()
                .unwrap_or_else(|| panic!("the catalog must report an activity, got {listed}"))
                .to_string();
            emulator
                .call_with_body(
                    &key,
                    operation,
                    &json!({ "activityId": activity_id, "languageCode": "en-US" }),
                )
                .await
        } else {
            emulator.call(&key, operation).await
        };
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (200, ""),
            "case {operation}: an operation that models no 403 must not be judged by the \
             authorization gate, got {status} with {payload}"
        );
    }
}

/// RULE: the sentence either gated operation refuses with — and every reason clause it can
/// end in — is the ONE statement of IAM's wording the shared crate holds
/// (`service_common::iam_authorization`'s `not_authorized_by_identity_policy` and the
/// clauses beside it), so what this service puts on the wire is RENDERED by that statement
/// rather than by a copy of it kept in this crate.
///
/// The rows are the four clauses a decision taken here can rest on, each driven over the
/// wire through BOTH gated operations: no readable document allows the action, a readable
/// document explicitly denies it, the permissions boundary states nothing about it, and the
/// boundary explicitly denies it. The expectation is the shared statement's own output for
/// the principal STS reports and the action and resource the request named — so a reworded
/// clause or a repunctuated sentence in that one place is still exactly what this service
/// answers.
#[tokio::test]
async fn the_refusal_is_worded_by_the_shared_statement_of_iams_sentence() {
    use service_common::iam_authorization as iam_wording;

    let emulator = Emulator::new();

    for operation in GATED_OPERATIONS {
        let action = action_of(operation);
        let suffix = operation.to_lowercase();
        // A boundary allowing a DIFFERENT Free Tier action states nothing about this one;
        // a boundary carrying an explicit `Deny` of it denies it outright.
        let silent_boundary = emulator
            .managed_policy(
                &format!("freetier-wording-boundary-silent-{suffix}"),
                &allow(&action_of(other_gated(operation)), "*"),
            )
            .await;
        let denying_boundary = emulator
            .managed_policy(
                &format!("freetier-wording-boundary-deny-{suffix}"),
                &allow_all_but(&action),
            )
            .await;

        // (case, the access key id that asks, the clause the shared statement words it with)
        let mut rows: Vec<(&str, String, String)> = vec![
            (
                "no readable document allows the action",
                emulator
                    .user_key(&format!("freetier-wording-bare-{suffix}"), None)
                    .await,
                iam_wording::no_identity_policy_allows(&action),
            ),
            (
                "a readable document explicitly denies the action",
                emulator
                    .user_key(
                        &format!("freetier-wording-denied-{suffix}"),
                        Some(&allow_all_but(&action)),
                    )
                    .await,
                iam_wording::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
            ),
        ];

        let capped = format!("freetier-wording-capped-{suffix}");
        let capped_key = emulator.user_key(&capped, Some(&allow(&action, "*"))).await;
        emulator.put_user_boundary(&capped, &silent_boundary).await;
        rows.push((
            "the permissions boundary states nothing about the action",
            capped_key,
            iam_wording::no_permissions_boundary_allows(&action),
        ));

        let boundary_denied = format!("freetier-wording-boundary-denied-{suffix}");
        let boundary_denied_key = emulator
            .user_key(&boundary_denied, Some(&allow(&action, "*")))
            .await;
        emulator
            .put_user_boundary(&boundary_denied, &denying_boundary)
            .await;
        rows.push((
            "the permissions boundary explicitly denies the action",
            boundary_denied_key,
            iam_wording::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        ));

        for (case, access_key_id, clause) in &rows {
            let case = format!("{case}, asking {operation}");
            let (status, payload) = emulator.call(access_key_id, operation).await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected the modelled 403, got {status} with {payload}"
            );
            // The principal comes from STS's own reading of that credential, as everywhere
            // else in this suite.
            let principal = emulator.caller_arn(access_key_id).await;
            let message = message_of(&payload);
            assert_eq!(
                message,
                iam_wording::not_authorized_by_identity_policy(
                    &principal,
                    &action,
                    &resource_of(operation),
                    clause
                ),
                "case {case:?}: expected the shared statement's own sentence, got {message:?}"
            );
        }
    }
}

/// One request body a gated operation can be asked with, and what the model says about it.
struct Body {
    /// What this row varies: how the body relates to the operation's input shape.
    case: &'static str,
    /// The body itself.
    body: Value,
    /// Whether it breaks a modelled constraint of `UpgradeAccountPlanRequest` — its one
    /// member `accountPlanType` is `@required` and targets the enum `AccountPlanType`
    /// (`smithy_query(query="shape",shape="UpgradeAccountPlanRequest")`), so a body
    /// carrying no value for it, or one outside that enum, is refused the modelled
    /// `ValidationException` once the caller is authorized to make the call at all.
    breaks_a_constraint: bool,
    /// A value of this body no refusal of the IDENTITY may echo, when the row carries a
    /// distinctive one: a 403 states who was refused, which action and on which resource,
    /// and nothing about the request's own input.
    echo: Option<&'static str>,
}

/// The bodies both legs below are driven over: the rows differ along HOW the body relates
/// to the operation's input shape — well-formed, the `@required` member absent, a value
/// outside the enum, the empty string, a value of another JSON type (number, list, object,
/// null), a member the shape does not declare, and a body that is not a structure at all.
fn bodies() -> Vec<Body> {
    vec![
        Body {
            case: "a well-formed request",
            body: json!({ "accountPlanType": PLAN_TYPE_PAID }),
            breaks_a_constraint: false,
            echo: None,
        },
        Body {
            case: "the required member absent",
            body: json!({}),
            breaks_a_constraint: true,
            echo: None,
        },
        Body {
            case: "a value outside the enum",
            body: json!({ "accountPlanType": "GOLD" }),
            breaks_a_constraint: true,
            echo: Some("GOLD"),
        },
        Body {
            case: "the empty string",
            body: json!({ "accountPlanType": "" }),
            breaks_a_constraint: true,
            echo: None,
        },
        Body {
            case: "a number where a string is modelled",
            body: json!({ "accountPlanType": 7 }),
            breaks_a_constraint: false,
            echo: None,
        },
        Body {
            case: "a list where a string is modelled",
            body: json!({ "accountPlanType": [PLAN_TYPE_PAID] }),
            breaks_a_constraint: false,
            echo: None,
        },
        Body {
            case: "an object where a string is modelled",
            body: json!({ "accountPlanType": { "value": PLAN_TYPE_PAID } }),
            breaks_a_constraint: false,
            echo: None,
        },
        Body {
            case: "an explicit null",
            body: json!({ "accountPlanType": Value::Null }),
            breaks_a_constraint: false,
            echo: None,
        },
        Body {
            case: "a member the shape does not declare",
            body: json!({ "accountPlanKind": "GOLDEN" }),
            breaks_a_constraint: true,
            echo: Some("GOLDEN"),
        },
        Body {
            case: "a body that is not a structure",
            body: json!([PLAN_TYPE_PAID]),
            breaks_a_constraint: false,
            echo: None,
        },
    ]
}

/// RULE: the authorization decision is taken BEFORE anything of the request's own input is
/// read, so a caller whose identity-based policies allow it nothing is refused the modelled
/// 403 — in IAM's own sentence about the identity, the action and the resource, echoing no
/// value of the request — whatever the body it sent carries, and the call writes nothing.
///
/// The rows span the body side of that rule ([`bodies`]): the ordering is the same decision
/// for a well-formed request, for one that breaks a modelled constraint of the input shape,
/// and for one the codec cannot even read as that shape. Both gated operations are driven,
/// because the rule is the gate's and not one operation's.
#[tokio::test]
async fn the_decision_is_taken_before_the_requests_own_input_is_read() {
    let emulator = Emulator::new();
    let refused_key = emulator.user_key("freetier-ordering-bare", None).await;
    let principal = emulator.caller_arn(&refused_key).await;

    for operation in GATED_OPERATIONS {
        let action = action_of(operation);
        let before = emulator.plan_snapshot().await;
        for row in bodies() {
            let case = format!("{} asking {operation}", row.case);
            let (status, payload) = emulator
                .call_with_body(&refused_key, operation, &row.body)
                .await;
            check(
                &case,
                operation,
                Expect::Refused(Reason::NothingAllows),
                status,
                &payload,
            );
            let message = message_of(&payload);
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: {} {}",
                    resource_of(operation),
                    Reason::NothingAllows.clause(&action)
                ),
                "case {case:?}: expected IAM's whole sentence about the identity, got {message:?}"
            );
            if let Some(echo) = row.echo {
                assert!(
                    !message.contains(echo),
                    "case {case:?}: a refusal of the identity must echo no value of the \
                     request, but {message:?} carries {echo:?}"
                );
            }
            let after = emulator.plan_snapshot().await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused call must leave the account's plan record exactly \
                 as it was, got {after:?} after {before:?}"
            );
        }
    }
}

/// RULE's other side: the gate decides the ORDER and not the outcome, so the same bodies
/// sent by a caller whose policies DO allow the action are answered by the operation — and
/// one that breaks a modelled constraint of the input shape is still refused the modelled
/// `ValidationException` (HTTP 400) rather than passed through.
#[tokio::test]
async fn an_authorized_caller_is_still_answered_by_the_model_about_its_input() {
    let emulator = Emulator::new();
    let allowed_key = emulator
        .user_key("freetier-ordering-allowed", Some(&allow("freetier:*", "*")))
        .await;

    for operation in GATED_OPERATIONS {
        for row in bodies() {
            let case = format!("{} asking {operation}", row.case);
            let (status, payload) = emulator
                .call_with_body(&allowed_key, operation, &row.body)
                .await;
            assert_ne!(
                (status, error_code(&payload).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: a caller the policies allow must not be refused by the gate, \
                 got {status} with {payload}"
            );
            if row.breaks_a_constraint && operation == "UpgradeAccountPlan" {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: a body that breaks a modelled constraint must still be \
                     refused the modelled ValidationException, got {status} with {payload}"
                );
            }
        }
    }
}
