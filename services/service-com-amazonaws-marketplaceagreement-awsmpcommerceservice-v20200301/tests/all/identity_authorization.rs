// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service, driven over the real wire path (an awsJson1_0 request
//! through the multi-service router into this service mounted beside IAM and STS on one
//! shared engine), so what is asserted is what a client sees.
//!
//! RULE. AWS takes IAM's decision in FRONT of the service, so a request is served only
//! where the identity-based policies in force on the principal its credentials name allow
//! one of the actions that PERMIT the operation — `aws-marketplace:<OperationName>`, and
//! for an operation that only reports an account's agreements the coarse
//! `aws-marketplace:ViewSubscriptions` as well; a request no statement allows, or one an
//! explicit `Deny` covers, is refused with the modelled `AccessDeniedException`
//! (`@httpError(403)`, a service-common error of this model, so every operation declares
//! it) BEFORE the store is read — so the refusal does not depend on what the request names,
//! and an unauthorized caller cannot tell from it whether the agreement it named exists.
//!
//! The rows span what the rule generalises over: the OPERATION asked for (a read of one
//! agreement, the create that mints a quote, a listing, a mutation, and the one operation
//! whose own answer is a modelled refusal), the DOCUMENT in force on the principal (none at
//! all, another service's actions only, another action of this service only, an allow of
//! exactly the action, an allow of the whole service, an allow of everything beside an
//! explicit deny — the shape a scoped-down role carries), and the CREDENTIAL (an
//! assumed-role session, the account's own published key). The refusal's sentence is
//! asserted whole, with every part — principal, action, resource, reason — taken from the
//! request and the stored policy.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine
//! (`CreateRole` + `PutRolePolicy` + `AssumeRole`, `DeleteRolePolicy`), so the documents
//! this service reads are the ones those services wrote; the decision itself is
//! `service_common::iam_authorization`, shared with every other service that authorizes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The awsJson1_0 target prefix this service routes on, from the crate's `OPERATIONS.md`
/// wire-protocol section.
const TARGET_PREFIX: &str = "AWSMPCommerceService_v20200301";

/// The Region every call signs for — the one this service answers in.
const REGION: &str = "us-east-1";

/// This service's sigv4 signing name, which is also the prefix of every IAM action its
/// operations are performed under.
pub(crate) const SIGNING_NAME: &str = "aws-marketplace";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
pub(crate) const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The resource an action of this service is judged against: the AWS Marketplace actions it
/// is performed under take no resource type, so the decision is made against IAM's own
/// resource wildcard, which is what the refusal names.
pub(crate) const ANY_RESOURCE: &str = "*";

/// The status and wire error code of an authorization refusal: the modelled
/// `AccessDeniedException` at its modelled 403.
pub(crate) const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with — the same set
/// `core/router`'s `build_router` assembles, narrowed to what this rule touches.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::EmulatedService::new(
                Arc::clone(&engine),
            ),
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
        // The offers these cases quote against are published into this
        // emulator's catalog by the call helper below, as
        // `agreement_lifecycle`'s harness documents: a quote resolves the
        // proposal it names.
        crate::agreement_lifecycle::remember_catalog(&engine);
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    pub(crate) async fn call(
        &self,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        crate::agreement_lifecycle::publish_named_offer(body);
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("agreement-marketplace.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {SIGNING_NAME}/aws4_request, SignedHeaders=host;x-amz-target, \
                     Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// One awsQuery call to IAM or STS, signed with the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(body))
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

    /// Create an IAM role the account may assume, assume it through STS, and answer the
    /// session's access key id with the session ARN `sts:GetCallerIdentity` reports.
    pub(crate) async fn assumed_role(&self, role_name: &str, policy: Option<&Value>) -> Principal {
        let account = ::engine::emulator_account_id();
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
            }],
        });
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust.to_string()),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.put_role_policy(role_name, policy).await;
        }
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        Principal {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
            arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
        }
    }

    /// Write (or rewrite) the one inline policy of `role_name`, under the same policy name
    /// every time, so a put REPLACES what the role carried — what `iam:PutRolePolicy` does.
    pub(crate) async fn put_role_policy(&self, role_name: &str, policy: &Value) {
        self.put_named_role_policy(role_name, POLICY_NAME, policy)
            .await;
    }

    /// Write one inline policy of `role_name` under the policy NAME given, so a role can
    /// carry SEVERAL identity-based documents at once — which is what `iam:PutRolePolicy`
    /// does: a put replaces the document of that policy name and leaves every other
    /// document the role carries in force.
    pub(crate) async fn put_named_role_policy(
        &self,
        role_name: &str,
        policy_name: &str,
        policy: &Value,
    ) {
        self.query(
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", policy_name.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }

    /// Take the inline policy `policy_name` off `role_name`, leaving the role's other
    /// documents in force — what `iam:DeleteRolePolicy` does.
    pub(crate) async fn delete_named_role_policy(&self, role_name: &str, policy_name: &str) {
        self.query(
            "iam",
            "2010-05-08",
            "DeleteRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", policy_name.to_string()),
            ],
        )
        .await;
    }

    /// Create a customer-managed policy carrying `policy` and attach it to `role_name` —
    /// `iam:CreatePolicy` followed by `iam:AttachRolePolicy`, the other way a document is
    /// put in force on a principal. The ARN the attach names is the one the create
    /// answered, so the document this service reads is the one IAM stored.
    pub(crate) async fn attach_managed_policy(
        &self,
        role_name: &str,
        policy_name: &str,
        policy: &Value,
    ) {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreatePolicy",
                &[
                    ("PolicyName", policy_name.to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        let policy_arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy answers the policy's Arn, got {created}"));
        self.query(
            "iam",
            "2010-05-08",
            "AttachRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyArn", policy_arn),
            ],
        )
        .await;
    }

    /// Take that inline policy off the role again — what `iam:DeleteRolePolicy` does.
    pub(crate) async fn delete_role_policy(&self, role_name: &str) {
        self.delete_named_role_policy(role_name, POLICY_NAME).await;
    }

    /// Create and accept one agreement with the account's own credential, answering its
    /// agreement id — the stored resource the authorization rows are aimed at.
    pub(crate) async fn given_agreement(&self, proposal: &str) -> String {
        let (status, created) = self
            .call(
                ACCOUNT_KEY,
                "CreateAgreementRequest",
                &json!({
                    "intent": "NEW",
                    "agreementProposalIdentifier": proposal,
                    "requestedTerms": [{"id": "LegalTerm"}],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "the fixture quote must be created, got {status} with {created}"
        );
        let request_id = created["agreementRequestId"]
            .as_str()
            .expect("the create answers an agreementRequestId")
            .to_string();
        let (status, accepted) = self
            .call(
                ACCOUNT_KEY,
                "AcceptAgreementRequest",
                &json!({"agreementRequestId": request_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "the fixture agreement must be accepted, got {status} with {accepted}"
        );
        accepted["agreementId"]
            .as_str()
            .expect("the accept answers an agreementId")
            .to_string()
    }
}

/// The name of the inline policy every row writes, so a put replaces the last row's.
const POLICY_NAME: &str = "inline";

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
pub(crate) struct Principal {
    pub(crate) access_key_id: String,
    pub(crate) arn: String,
}

/// A policy allowing `actions` on every resource.
pub(crate) fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}],
    })
}

/// A policy denying `actions` on every resource, with no grant of its own — the shape a
/// scope-down written as its OWN document carries, put in force beside a grant rather than
/// replacing it.
pub(crate) fn deny(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Deny", "Action": actions, "Resource": "*"}],
    })
}

/// A policy allowing everything beside an explicit deny of `actions` — the shape a
/// scoped-down role carries, where the `Deny` beats any `Allow` the principal also holds.
pub(crate) fn allow_all_but(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "*", "Resource": "*"},
            {"Effect": "Deny", "Action": actions, "Resource": "*"},
        ],
    })
}

/// The coarse AWS Marketplace action that also permits an operation that only REPORTS an
/// account's agreements — the alternative AWS names in the refusal it answers an
/// unauthorized read of an agreement with.
pub(crate) const VIEW_SUBSCRIPTIONS: &str = "ViewSubscriptions";

/// The operations that coarse action also permits: the reports of an account's agreements.
const VIEWABLE_OPERATIONS: &[&str] = &[
    "DescribeAgreement",
    "GetAgreementEntitlements",
    "GetAgreementTerms",
    "SearchAgreements",
];

/// Every action the decision about `operation` is taken over, the one it is performed under
/// first.
pub(crate) fn evaluated_actions(operation: &str) -> Vec<String> {
    let mut actions = vec![format!("{SIGNING_NAME}:{operation}")];
    if VIEWABLE_OPERATIONS.contains(&operation) {
        actions.push(format!("{SIGNING_NAME}:{VIEW_SUBSCRIPTIONS}"));
    }
    actions
}

/// Which verdict a row expects, and — for a refusal — which of IAM's rules reached it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// IAM permits the request, so the operation answers its own way.
    Served,
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Verdict {
    /// The clause IAM closes the refusal with, over the actions the decision was taken
    /// about: an operation SEVERAL actions permit is refused in one sentence naming the
    /// whole set and neither rule, and one a single action permits names the rule.
    pub(crate) fn reason(self, actions: &[String]) -> String {
        assert_ne!(
            self,
            Self::Served,
            "a served request carries no refusal clause"
        );
        let named = actions.join(" or ");
        if actions.len() > 1 {
            return format!(
                "because no identity-based policy allows the {named} action, or explicitly \
                 denies the actions"
            );
        }
        match self {
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
            _ => format!("because no identity-based policy allows the {named} action"),
        }
    }
}

/// One request a credential can reach this service with: the operation IAM is asked about
/// and the body that makes the call.
struct Call {
    case: &'static str,
    operation: &'static str,
    body: Value,
}

/// Every class of request a credential can reach this service with, one row per thing the
/// rule generalises over: a read of one agreement, the create that mints a quote, a
/// listing, a mutation of a stored agreement, and the operation whose own answer is a
/// modelled refusal.
fn calls(agreement_id: &str) -> Vec<Call> {
    vec![
        Call {
            case: "a read of one agreement",
            operation: "DescribeAgreement",
            body: json!({"agreementId": agreement_id}),
        },
        Call {
            case: "the create that mints a quote",
            operation: "CreateAgreementRequest",
            body: json!({
                "intent": "NEW",
                "agreementProposalIdentifier": "ap-authorization",
                "requestedTerms": [{"id": "LegalTerm"}],
            }),
        },
        Call {
            case: "a listing",
            operation: "SearchAgreements",
            body: json!({
                "filters": [{"name": "AgreementType", "values": ["PurchaseAgreement"]}],
            }),
        },
        Call {
            case: "a mutation of a stored agreement",
            operation: "CancelAgreement",
            body: json!({"agreementId": agreement_id}),
        },
        Call {
            case: "an operation whose own answer is a modelled refusal",
            operation: "UpdatePurchaseOrders",
            body: json!({"purchaseOrders": [{"chargeId": "charge-1"}]}),
        },
    ]
}

/// The error code a response carries (`__type` on the awsJson protocols), without its
/// namespace prefix, and the empty string for a response that is no error at all.
pub(crate) fn code_of(body: &Value) -> &str {
    let code = body["__type"].as_str().unwrap_or_default();
    code.rsplit('#').next().unwrap_or(code)
}

/// The `message` an error response carries.
pub(crate) fn message_of(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// RULE: every operation of this service is authorized under
/// `aws-marketplace:<OperationName>` before it reads or writes anything — served only where
/// the policies in force on the caller's principal allow that action, and otherwise refused
/// 403 `AccessDeniedException` with IAM's own sentence.
#[tokio::test]
async fn every_operation_is_authorized_under_this_services_action() {
    // (case, the document in force on the role, the verdict every call then gets)
    let documents: Vec<(&str, Option<Value>, Verdict)> = vec![
        ("no policy at all", None, Verdict::DefaultDeny),
        (
            "another service's actions only",
            Some(allow(json!("s3:*"))),
            Verdict::DefaultDeny,
        ),
        (
            "one other action of this service only",
            Some(allow(json!("aws-marketplace:GetAgreementTerms"))),
            Verdict::DefaultDeny,
        ),
        (
            "every action of this service",
            Some(allow(json!("aws-marketplace:*"))),
            Verdict::Served,
        ),
        (
            "every action of every service",
            Some(allow(json!("*"))),
            Verdict::Served,
        ),
        (
            "everything but an explicit deny of this service",
            Some(allow_all_but(json!("aws-marketplace:*"))),
            Verdict::ExplicitDeny,
        ),
    ];

    for (index, (document_case, document, verdict)) in documents.into_iter().enumerate() {
        let emulator = Emulator::new();
        let agreement_id = emulator.given_agreement(&format!("ap-authz{index}")).await;
        let session = emulator
            .assumed_role(&format!("agreement-caller-{index}"), document.as_ref())
            .await;

        for call in calls(&agreement_id) {
            let case = format!("{document_case} / {}", call.case);
            let (status, body) = emulator
                .call(&session.access_key_id, call.operation, &call.body)
                .await;
            let action = format!("{SIGNING_NAME}:{}", call.operation);
            let actions = evaluated_actions(call.operation);
            match verdict {
                Verdict::Served => {
                    assert_ne!(
                        code_of(&body),
                        ACCESS_DENIED.1,
                        "case {case:?}: an allowed action must not be refused by the \
                         authorization layer, got {status} with {body}"
                    );
                }
                refused => {
                    assert_eq!(
                        status, ACCESS_DENIED.0,
                        "case {case:?}: expected {}, got {status} with {body}",
                        ACCESS_DENIED.0
                    );
                    assert_eq!(
                        code_of(&body),
                        ACCESS_DENIED.1,
                        "case {case:?}: expected {}, got {body}",
                        ACCESS_DENIED.1
                    );
                    let expected = format!(
                        "User: {} is not authorized to perform: {action} on resource: \
                         {ANY_RESOURCE} {}",
                        session.arn,
                        refused.reason(&actions)
                    );
                    assert_eq!(
                        message_of(&body),
                        expected,
                        "case {case:?}: expected IAM's wording, got {body}"
                    );
                }
            }
        }
    }
}

/// RULE: the authorization decision is taken BEFORE the store is read, so it does not
/// depend on what the request names: the identical call is refused 403 whether the
/// agreement it names exists or not, and putting the `Allow` back returns both calls to the
/// answers the store gives — the not-found refusal for an id nothing answers to, the report
/// for the one that exists.
#[tokio::test]
async fn an_identity_denial_precedes_the_store_lookup() {
    let emulator = Emulator::new();
    let agreement_id = emulator.given_agreement("ap-precedence").await;
    let role = "agreement-precedence";
    let session = emulator
        .assumed_role(role, Some(&allow(json!("aws-marketplace:*"))))
        .await;

    // (case, the agreementId the identical call names, what the store answers once the
    // caller is allowed again: the error code, or the empty string for a report)
    let rows: &[(&str, String, &str)] = &[
        (
            "an agreement the store holds",
            agreement_id.clone(),
            "",
        ),
        (
            "an id nothing answers to",
            "no-such-agreement".to_string(),
            "ResourceNotFoundException",
        ),
        (
            "an id of another well-formed shape",
            "agmt-0000000000009999".to_string(),
            "ResourceNotFoundException",
        ),
    ];

    // While the role is allowed, each call gets the answer the STORE gives.
    for (case, named, expected) in rows {
        let (status, body) = emulator
            .call(
                &session.access_key_id,
                "DescribeAgreement",
                &json!({"agreementId": named}),
            )
            .await;
        assert_eq!(
            code_of(&body),
            *expected,
            "case {case:?} (allowed): expected {expected:?}, got {status} with {body}"
        );
    }

    // The grant is taken off the role: now the identical calls are refused, with the SAME
    // body whichever id they name — the decision never reached the lookup.
    emulator.delete_role_policy(role).await;
    for (case, named, _) in rows {
        let (status, body) = emulator
            .call(
                &session.access_key_id,
                "DescribeAgreement",
                &json!({"agreementId": named}),
            )
            .await;
        assert_eq!(
            status, ACCESS_DENIED.0,
            "case {case:?} (no grant): expected {}, got {status} with {body}",
            ACCESS_DENIED.0
        );
        assert_eq!(
            code_of(&body),
            ACCESS_DENIED.1,
            "case {case:?} (no grant): expected {}, got {body}",
            ACCESS_DENIED.1
        );
        let action = format!("{SIGNING_NAME}:DescribeAgreement");
        assert_eq!(
            message_of(&body),
            format!(
                "User: {} is not authorized to perform: {action} on resource: {ANY_RESOURCE} {}",
                session.arn,
                Verdict::DefaultDeny.reason(&evaluated_actions("DescribeAgreement"))
            ),
            "case {case:?} (no grant): expected IAM's wording, got {body}"
        );
    }

    // The grant is put back: the identical calls answer from the store again, which is also
    // what shows the refusals above changed nothing.
    emulator
        .put_role_policy(role, &allow(json!("aws-marketplace:*")))
        .await;
    for (case, named, expected) in rows {
        let (status, body) = emulator
            .call(
                &session.access_key_id,
                "DescribeAgreement",
                &json!({"agreementId": named}),
            )
            .await;
        assert_eq!(
            code_of(&body),
            *expected,
            "case {case:?} (allowed again): expected {expected:?}, got {status} with {body}"
        );
    }
}

/// RULE: a refused call performs nothing — the agreement a refused mutation named is left
/// exactly as it was, and the quote a refused create would have minted does not exist.
#[tokio::test]
async fn a_refused_call_changes_nothing() {
    let emulator = Emulator::new();
    let agreement_id = emulator.given_agreement("ap-untouched").await;
    let session = emulator.assumed_role("agreement-nothing", None).await;

    for call in calls(&agreement_id) {
        let (status, body) = emulator
            .call(&session.access_key_id, call.operation, &call.body)
            .await;
        assert_eq!(
            status, ACCESS_DENIED.0,
            "case {:?}: expected {}, got {status} with {body}",
            call.case, ACCESS_DENIED.0
        );
    }

    // The account's own credential still reads the agreement back ACTIVE: the refused
    // `CancelAgreement` did not move its status.
    let (status, described) = emulator
        .call(
            ACCOUNT_KEY,
            "DescribeAgreement",
            &json!({"agreementId": agreement_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the fixture survives\": expected 200, got {status} with {described}"
    );
    assert_eq!(
        described["status"].as_str().unwrap_or_default(),
        "ACTIVE",
        "case \"a refused mutation changes nothing\": expected the agreement to stay ACTIVE, \
         got {described}"
    );

    // And the quote the refused create named was never minted: the store still holds
    // exactly the one agreement the fixture accepted, so nothing the refused calls asked
    // for reached it.
    let (status, found) = emulator
        .call(
            ACCOUNT_KEY,
            "SearchAgreements",
            &json!({
                "filters": [{"name": "AgreementType", "values": ["PurchaseAgreement"]}],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the store is readable\": expected 200, got {status} with {found}"
    );
    let reported: Vec<&str> = found["agreementViewSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["agreementId"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported,
        vec![agreement_id.as_str()],
        "case \"a refused call adds nothing to the store\": expected only the fixture \
         agreement, got {found}"
    );
}

/// RULE: the account's own published credential is not a principal the IAM store holds
/// policies for — the position AWS account root credentials are in — so a request signed
/// with it is served whatever the role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bounded_by_the_roles_policies() {
    let emulator = Emulator::new();
    let agreement_id = emulator.given_agreement("ap-ambient").await;
    emulator
        .assumed_role(
            "agreement-ambient",
            Some(&allow_all_but(json!("aws-marketplace:*"))),
        )
        .await;

    for call in calls(&agreement_id) {
        let (status, body) = emulator.call(ACCOUNT_KEY, call.operation, &call.body).await;
        assert_ne!(
            code_of(&body),
            ACCESS_DENIED.1,
            "case {:?}: expected the account's own credential to be served, got {status} \
             with {body}",
            call.case
        );
    }
}

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
