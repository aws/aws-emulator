// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's operations, over the restJson1 wire.
//!
//! The rule under test: a request to an operation of this service is served only to a
//! caller whose identity-based IAM policies allow THAT operation's own action
//! (`appsync:<OperationName>`) on the resource the request addresses — the custom domain's
//! ARN, the API's, the channel namespace's, the ARN a tagging call names, and the
//! any-resource wildcard for a call that addresses no resource of its own. Every other
//! caller is refused with the modelled `AccessDeniedException` (HTTP 403) BEFORE the
//! operation reads or writes anything, so a refused caller learns nothing about what the
//! account holds — not even whether the resource it named exists — and a refused mutation
//! changes nothing. The decision belongs to the layer in front of the service, so it is
//! taken for every action this service publishes, whether or not the operation behind it
//! lists the refusal among its own modelled errors.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateUser` + `PutUserPolicy` + `CreateAccessKey`,
//! `CreateRole` + `PutRolePolicy` + `AssumeRole`), so the documents this service reads are
//! the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (a read of one resource, a read of a collection, a create, an
//! update, a delete, a tagging read, a tagging write and a sandbox evaluation — each with
//! its own action, its own HTTP method and its own kind of resource ARN), what the
//! caller's policies say (nothing at all, an `Allow` for this action on this resource, a
//! wildcard `Allow`, an `Allow` for another action, an `Allow` on another resource, an
//! explicit `Deny` beside a wildcard `Allow`), and the KIND of principal (the account's own
//! published credential, an IAM user, an assumed-role session).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::service::authorization::{
    ACTION_PREFIX, ANY_RESOURCE,
};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::encode_label;

/// The region every call below is signed for, so the ARNs the decisions are made about are
/// the ones this region's resources carry.
pub const REGION: &str = "eu-west-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
pub const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// The wire name and status of the modelled refusal.
pub const ACCESS_DENIED: &str = "AccessDeniedException";
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// One response, reduced to what a client branches on.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub error_code: String,
    pub body: Value,
}

impl Reply {
    /// The `message` the error body carries.
    pub fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
pub struct Principal {
    pub access_key_id: String,
    pub arn: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    pub async fn call(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     appsync/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
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
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
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

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint
    /// a long-term access key for it, and answer that key id with the user's own ARN.
    pub async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
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
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the session's access key id with the session ARN.
    pub async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Principal {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            ::engine::emulator_account_id()
        );
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            ::engine::emulator_account_id()
                        ),
                    ),
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

/// A policy document allowing `action` on `resource`.
pub fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every action of this service on everything, and explicitly
/// denying `denied`.
pub fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"{ACTION_PREFIX}:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// An ARN of this service in the region the suite signs for.
pub fn arn(resource: &str) -> String {
    ::engine::emulator_arn(ACTION_PREFIX, REGION, resource)
}

/// One request a decision is taken about: the call itself, the action and resource IAM
/// judges it by, and the status an AUTHORIZED caller gets — the operation's own answer,
/// which is how a served call is told from a refused one.
struct Gated {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    operation: &'static str,
    resource: String,
    served_status: u16,
}

/// The resources the table is written about, created by the account's own credential.
struct Fixture {
    api_id: String,
    api_arn: String,
    domain: String,
}

/// A domain name of the modelled `DomainName` form, distinct per `label`.
fn domain_name(label: &str) -> String {
    format!("{label}.example.com")
}

/// Create the one GraphQL API and the one custom domain the table addresses.
async fn fixture(emulator: &Emulator) -> Fixture {
    let created = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v1/apis",
            Some(&json!({
                "name": "Authorized_api",
                "authenticationType": "API_KEY",
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"fixture\": expected CreateGraphqlApi 200, got {created:?}"
    );
    let domain = domain_name("held");
    let held = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v1/domainnames",
            Some(&json!({
                "domainName": domain,
                "certificateArn":
                    "arn:aws:acm:us-east-1:000000000000:certificate/authorized-0001",
            })),
        )
        .await;
    assert_eq!(
        held.status, 200,
        "case \"fixture\": expected CreateDomainName 200, got {held:?}"
    );
    Fixture {
        api_id: created.body["graphqlApi"]["apiId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        api_arn: created.body["graphqlApi"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        domain,
    }
}

/// The table of gated requests, one per class of resource and method the rule spans.
///
/// `suffix` keeps the rows that CREATE a resource distinct per principal, so the same
/// table can be replayed for each one.
fn gated_requests(fixture: &Fixture, suffix: &str) -> Vec<Gated> {
    let held = &fixture.domain;
    let absent = domain_name(&format!("absent-{suffix}"));
    let created = domain_name(&format!("created-{suffix}"));
    // An API id of the modelled form (26 characters) that nothing created.
    let absent_api = "zzzzzzzzzzzzzzzzzzzzzzzzzz";
    vec![
        Gated {
            case: "a read of one custom domain",
            method: "GET",
            path: format!("/v1/domainnames/{held}"),
            body: None,
            operation: "GetDomainName",
            resource: arn(&state::domain_name_resource(held)),
            served_status: 200,
        },
        Gated {
            case: "a read of one custom domain the account does not hold",
            method: "GET",
            path: format!("/v1/domainnames/{absent}"),
            body: None,
            operation: "GetDomainName",
            resource: arn(&state::domain_name_resource(&absent)),
            // The gate fires BEFORE the lookup, so an authorized caller is the only one
            // that learns the domain is absent.
            served_status: 404,
        },
        Gated {
            case: "a read of the custom domain collection",
            method: "GET",
            path: "/v1/domainnames".to_string(),
            body: None,
            operation: "ListDomainNames",
            // A listing names no one resource, so IAM judges it against every resource of
            // this service the account holds in the region addressed.
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
        Gated {
            case: "a create in the custom domain collection",
            method: "POST",
            path: "/v1/domainnames".to_string(),
            body: Some(json!({
                "domainName": created,
                "certificateArn":
                    "arn:aws:acm:us-east-1:000000000000:certificate/authorized-0002",
            })),
            operation: "CreateDomainName",
            // A create names the resource it is about to create, so that resource's own ARN
            // is what the action is judged against.
            resource: arn(&state::domain_name_resource(&created)),
            served_status: 200,
        },
        Gated {
            case: "an update of one custom domain",
            method: "POST",
            path: format!("/v1/domainnames/{held}"),
            body: Some(json!({"description": "the domain the table addresses"})),
            operation: "UpdateDomainName",
            resource: arn(&state::domain_name_resource(held)),
            served_status: 200,
        },
        Gated {
            case: "a delete of one custom domain the account does not hold",
            method: "DELETE",
            path: format!("/v1/domainnames/{absent}"),
            body: None,
            operation: "DeleteDomainName",
            resource: arn(&state::domain_name_resource(&absent)),
            served_status: 404,
        },
        Gated {
            case: "a read of one GraphQL API",
            method: "GET",
            path: format!("/v1/apis/{}", fixture.api_id),
            body: None,
            operation: "GetGraphqlApi",
            resource: arn(&state::api_resource(&fixture.api_id)),
            served_status: 200,
        },
        Gated {
            case: "a read of one Event API the account does not hold",
            method: "GET",
            path: format!("/v2/apis/{absent_api}"),
            body: None,
            operation: "GetApi",
            resource: arn(&state::api_resource(absent_api)),
            served_status: 404,
        },
        Gated {
            case: "a read of one channel namespace of an API the account does not hold",
            method: "GET",
            path: format!("/v2/apis/{absent_api}/channelNamespaces/orders"),
            body: None,
            operation: "GetChannelNamespace",
            resource: arn(&state::channel_namespace_resource(absent_api, "orders")),
            served_status: 404,
        },
        Gated {
            case: "a read of a resource's tags",
            method: "GET",
            path: format!("/v1/tags/{}", encode_label(&fixture.api_arn)),
            body: None,
            operation: "ListTagsForResource",
            resource: fixture.api_arn.clone(),
            served_status: 200,
        },
        Gated {
            case: "a write of a resource's tags",
            method: "POST",
            path: format!("/v1/tags/{}", encode_label(&fixture.api_arn)),
            body: Some(json!({"tags": {"stage": "beta"}})),
            operation: "TagResource",
            resource: fixture.api_arn.clone(),
            // `TagResourceResponse` carries no members, which restJson1 answers empty.
            served_status: 204,
        },
        Gated {
            case: "a code evaluation, which addresses no resource of its own",
            method: "POST",
            path: "/v1/dataplane-evaluatecode".to_string(),
            body: Some(json!({
                "runtime": {"name": "APPSYNC_JS", "runtimeVersion": "1.0.0"},
                "code": "export function request(ctx) { return {ok: true} }",
                "context": "{\"arguments\":{}}",
            })),
            operation: "EvaluateCode",
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
    ]
}

/// The refusal IAM words for a request no identity-based document allows.
pub fn default_deny_message(principal: &str, operation: &str, resource: &str) -> String {
    let action = format!("{ACTION_PREFIX}:{operation}");
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// Assert `reply` is the modelled refusal of `request` for `principal`.
fn assert_refused(case: &str, principal: &Principal, request: &Gated, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {case:?} / {:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got {} {:?} \
         with {:?}",
        request.case,
        reply.status,
        reply.error_code,
        reply.body
    );
    let expected = default_deny_message(&principal.arn, request.operation, &request.resource);
    assert_eq!(
        reply.message(),
        expected,
        "case {case:?} / {:?}: expected the refusal to name the principal, the action and \
         the resource, got {:?}",
        request.case,
        reply.message()
    );
}

/// RULE: every gated operation refuses a principal whose identity-based policies allow
/// nothing, with the modelled `AccessDeniedException` (403) naming the principal, the
/// action and the resource the request addressed — and changes nothing.
#[tokio::test]
async fn a_principal_no_policy_allows_is_refused_every_gated_operation() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let principal = emulator.user("zero_policy", None).await;

    for request in gated_requests(&fixture, "zero") {
        let reply = emulator
            .call(
                &principal.access_key_id,
                request.method,
                &request.path,
                request.body.as_ref(),
            )
            .await;
        assert_refused("no policy at all", &principal, &request, &reply);
    }

    // The refused mutations changed nothing: the create never happened, and the domain the
    // update addressed still carries the description its create stored (none).
    let listed = emulator
        .call(ACCOUNT_KEY, "GET", "/v1/domainnames", None)
        .await;
    let names: Vec<&str> = listed.body["domainNameConfigs"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["domainName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![fixture.domain.as_str()],
        "case \"a refused mutation writes nothing\": expected only the fixture's domain, \
         got {names:?}"
    );
    let read = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/v1/domainnames/{}", fixture.domain),
            None,
        )
        .await;
    assert_eq!(
        read.body["domainNameConfig"]["description"],
        Value::Null,
        "case \"a refused mutation writes nothing\": expected the description untouched, \
         got {read:?}"
    );
}

/// RULE: the same signed call flips between served and refused with what the caller's
/// identity-based policies say — a grant of that action on that resource serves it, and
/// anything short of one (a grant of another action, a grant on another resource, an
/// explicit deny beside a wildcard grant) refuses it.
#[tokio::test]
async fn what_the_policies_say_decides_the_same_signed_call() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // The policies each row of the table is replayed under: a label, and the document, built
    // from the row's own action and resource.
    type Document = fn(&Gated) -> String;
    let served: &[(&str, Document)] = &[
        ("an allow of this action on this resource", |request| {
            allow(
                &format!("{ACTION_PREFIX}:{}", request.operation),
                &request.resource,
            )
        }),
        ("an allow of every action on everything", |_| {
            allow(&format!("{ACTION_PREFIX}:*"), ANY_RESOURCE)
        }),
    ];
    let refused: &[(&str, Document)] = &[
        ("an allow of another action", |_| {
            allow(&format!("{ACTION_PREFIX}:StartSchemaCreation"), ANY_RESOURCE)
        }),
        ("an allow on another resource", |request| {
            allow(
                &format!("{ACTION_PREFIX}:{}", request.operation),
                &arn("domainnames/not-the-one.example.com"),
            )
        }),
        ("an explicit deny beside a wildcard allow", |request| {
            allow_all_but(&format!("{ACTION_PREFIX}:{}", request.operation))
        }),
    ];

    for (index, (case, document)) in served.iter().enumerate() {
        for (row, request) in gated_requests(&fixture, &format!("served{index}"))
            .into_iter()
            .enumerate()
        {
            let principal = emulator
                .user(
                    &format!("served_{index}_{row}"),
                    Some(&document(&request)),
                )
                .await;
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    request.method,
                    &request.path,
                    request.body.as_ref(),
                )
                .await;
            assert_eq!(
                reply.status, request.served_status,
                "case {case:?} / {:?}: expected the operation's own {} answer, got {} {:?} \
                 with {:?}",
                request.case, request.served_status, reply.status, reply.error_code, reply.body
            );
        }
    }

    for (index, (case, document)) in refused.iter().enumerate() {
        for (row, request) in gated_requests(&fixture, &format!("refused{index}"))
            .into_iter()
            .enumerate()
        {
            let principal = emulator
                .user(
                    &format!("refused_{index}_{row}"),
                    Some(&document(&request)),
                )
                .await;
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    request.method,
                    &request.path,
                    request.body.as_ref(),
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                "case {case:?} / {:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got \
                 {} {:?} with {:?}",
                request.case,
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}

/// RULE: the account's own published credential is not a principal the IAM store holds
/// policies for, so it is served every gated operation — the position AWS account root
/// credentials are in.
#[tokio::test]
async fn the_accounts_own_credential_is_served_every_gated_operation() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for request in gated_requests(&fixture, "account") {
        let reply = emulator
            .call(
                ACCOUNT_KEY,
                request.method,
                &request.path,
                request.body.as_ref(),
            )
            .await;
        assert_eq!(
            reply.status, request.served_status,
            "case \"the account's own credential\" / {:?}: expected the operation's own {} \
             answer, got {} {:?} with {:?}",
            request.case, request.served_status, reply.status, reply.error_code, reply.body
        );
    }
}

/// RULE: an assumed-role session is judged on the ROLE's policies — so the same session is
/// refused when the role grants nothing and served once it grants the action.
#[tokio::test]
async fn an_assumed_role_session_is_judged_on_the_roles_policies() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let read = format!("/v1/domainnames/{}", fixture.domain);
    let resource = arn(&state::domain_name_resource(&fixture.domain));

    let ungranted = emulator.assumed_role("domain_none", None).await;
    let reply = emulator
        .call(&ungranted.access_key_id, "GET", &read, None)
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case \"a role granting nothing\": expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, \
         got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.message(),
        default_deny_message(&ungranted.arn, "GetDomainName", &resource),
        "case \"a role granting nothing\": expected the refusal to blame the session, got \
         {:?}",
        reply.message()
    );

    let granted = emulator
        .assumed_role(
            "domain_reader",
            Some(&allow(&format!("{ACTION_PREFIX}:GetDomainName"), &resource)),
        )
        .await;
    let served = emulator
        .call(&granted.access_key_id, "GET", &read, None)
        .await;
    assert_eq!(
        served.status, 200,
        "case \"a role granting the action\": expected the domain to be reported, got {} \
         {:?} with {:?}",
        served.status, served.error_code, served.body
    );
}

/// RULE: an operation that does NOT list `AccessDeniedException` among its own modelled
/// errors is judged all the same — the decision is the layer's, not the operation's — so a
/// caller whose policies allow nothing is refused with the 403 and its implicit-deny
/// sentence, and the same call is served once a policy allows the action.
///
/// The rows are operations of three different URI spaces and three different methods, each
/// one asserted to be outside the modelled 403 set rather than assumed, so a model change
/// moves this test rather than letting it pass vacuously.
#[tokio::test]
async fn an_operation_modelling_no_refusal_is_judged_too() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let ungranted = emulator.user("ungated_probe", None).await;
    let granted = emulator
        .user(
            "ungated_allowed",
            Some(&allow(&format!("{ACTION_PREFIX}:*"), ANY_RESOURCE)),
        )
        .await;

    /// One row: the operation, the resource it is judged against, its method, its path, its
    /// body, and the answer an authorized caller gets.
    type Row = (&'static str, String, &'static str, String, Option<Value>, u16);
    let rows: &[Row] = &[
        (
            "ListGraphqlApis",
            arn(ANY_RESOURCE),
            "GET",
            "/v1/apis".to_string(),
            None,
            200,
        ),
        (
            "ListApis",
            arn(ANY_RESOURCE),
            "GET",
            "/v2/apis".to_string(),
            None,
            200,
        ),
        (
            "ListApiKeys",
            arn(&state::api_resource(&fixture.api_id)),
            "GET",
            format!("/v1/apis/{}/apikeys", fixture.api_id),
            None,
            200,
        ),
        (
            "CreateApiKey",
            arn(&state::api_resource(&fixture.api_id)),
            "POST",
            format!("/v1/apis/{}/apikeys", fixture.api_id),
            Some(json!({})),
            200,
        ),
        (
            "ListDataSources",
            arn(&state::api_resource(&fixture.api_id)),
            "GET",
            format!("/v1/apis/{}/datasources", fixture.api_id),
            None,
            200,
        ),
    ];
    for (operation, resource, method, path, body, served) in rows {
        assert!(
            modelled_refusals(operation)
                .is_none_or(|errors| !errors.iter().any(|error| error == ACCESS_DENIED)),
            "case {operation:?}: this operation models the refusal, so it is covered by the \
             tables above rather than by this one"
        );
        let refused = emulator
            .call(&ungranted.access_key_id, method, path, body.as_ref())
            .await;
        assert_eq!(
            refused.status, ACCESS_DENIED_STATUS,
            "case {operation:?}: expected a caller whose policies allow nothing to be \
             refused, got {} {:?} with {:?}",
            refused.status, refused.error_code, refused.body
        );
        assert_eq!(
            refused.error_code, ACCESS_DENIED,
            "case {operation:?}: expected {ACCESS_DENIED}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            default_deny_message(&ungranted.arn, operation, resource),
            "case {operation:?}: expected IAM's implicit-deny sentence, got {:?}",
            refused.message()
        );
        let reply = emulator
            .call(&granted.access_key_id, method, path, body.as_ref())
            .await;
        assert_eq!(
            reply.status, *served,
            "case {operation:?}: expected the operation's own {served} answer for a caller \
             the policy allows, got {} {:?} with {:?}",
            reply.status, reply.error_code, reply.body
        );
    }
}

/// The error shapes `operation`'s own model declares, or `None` for a name the model does
/// not carry — read from the model that generated this crate, so the row above is checked
/// against the model rather than against a second hand list.
fn modelled_refusals(operation: &str) -> Option<Vec<String>> {
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json"
    );
    let model: Value = serde_json::from_str(
        &std::fs::read_to_string(MODEL_PATH).expect("the model ships beside this crate"),
    )
    .expect("the model is JSON");
    let (_, shape) = model["shapes"]
        .as_object()
        .expect("a Smithy model declares shapes")
        .iter()
        .find(|(id, shape)| {
            shape["type"] == "operation" && id.ends_with(&format!("#{operation}"))
        })?;
    Some(
        shape["errors"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|error| error["target"].as_str())
            .filter_map(|target| target.rsplit('#').next())
            .map(str::to_string)
            .collect(),
    )
}
