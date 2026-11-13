// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IAM AUTHORIZATION of this service's operations.
//!
//! RULE: AWS decides a signed call at the IAM layer, ahead of the service looking at the
//! resource the request names, so every operation of this endpoint is judged against the
//! CALLING principal's own identity-based policy documents — the principal being the
//! identity the access key id the request presents speaks for, and for an `sts:AssumeRole`
//! credential the session, whose permissions are the role's. The decision is IAM's own: an
//! explicit `Deny` about the request wins over every `Allow`, an `Allow` with no `Deny`
//! permits, and a principal with neither falls to IAM's default deny. The action is
//! `ssm-quicksetup:<OperationName>` — this service's own action namespace — and the
//! resource is the ARN the request named, or IAM's `*` for a call that names none. A
//! refusal is the service-common `AccessDeniedException` with HTTP 403
//! (`smithy_query(query="digest")` lists it under `common_errors`), carrying IAM's own
//! sentence, and it changes nothing in the store.
//!
//! The rows below span what the rule generalises over: which operation is asked for
//! (reads, a mutation and the tag plane), whether the principal holds no document at all /
//! a broad grant / a grant of another action / a grant plus an explicit deny of the asked
//! action / a deny scoped to the very resource named / a deny scoped to another resource.
//! Beside the table: the account's own published credential, which holds no IAM policies
//! and keeps being served, and a refused mutation, which leaves the resource exactly as it
//! was.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, so
//! the documents this service reads are the ones those services wrote.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const REGION: &str = "us-west-2";

/// The status and code an authorization refusal carries, from this service's model.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The namespace this service's actions are published under, which is also its SigV4
/// signing name and the namespace its ARNs carry
/// (`smithy_query(query="service_info")`: `arnNamespace` / `sigv4Name`).
const ACTION_PREFIX: &str = "ssm-quicksetup";

/// IAM's spelling of "no particular resource", which a call naming no ARN is judged
/// against.
const ANY_RESOURCE: &str = "*";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which
/// holds no IAM policies — the position AWS account root credentials are in. Every fixture
/// below is made with it.
const ACCOUNT_KEY: &str = "AKIDQUICKSETUP";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// This service beside the IAM and STS services its callers are made with, all on ONE
/// shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id and Region
        // from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(Arc::clone(
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
        Self { router }
    }

    /// One restJson1 call to this service, signed for [`REGION`] with `access_key_id` in
    /// the credential scope: the status and the parsed body, with the wire error code
    /// folded in under `__type` so one accessor reads either rendering.
    async fn call_as(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let host = format!("{ACTION_PREFIX}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {ACTION_PREFIX}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        let payload = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = builder.body(payload).expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let mut parsed: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
        };
        if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
            map.entry("__type")
                .or_insert_with(|| Value::String(error_type));
        }
        (status, parsed)
    }

    /// One awsQuery call to IAM or STS, made with the account's own credential, that must
    /// be served.
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
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "setup {action}: the call must succeed, got {status} with {text}"
        );
        text
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

/// Create an IAM role carrying `policies` as its inline policies, assume it through STS,
/// and answer the session's access key id with the session ARN.
async fn assumed_role(aws: &Emulator, role_name: &str, policies: &[String]) -> Principal {
    let account = engine::emulator_account_id();
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    aws.query(
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    for (index, policy) in policies.iter().enumerate() {
        aws.query(
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", format!("inline-{index}")),
                ("PolicyDocument", policy.clone()),
            ],
        )
        .await;
    }
    let assumed = aws
        .query(
            "sts",
            "2011-06-15",
            "AssumeRole",
            &[
                ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                ("RoleSessionName", SESSION.to_string()),
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

/// One identity-based policy document carrying one statement.
fn statement(effect: &str, actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// One identity-based document that grants every action of this service and denies
/// `denied` on `resource`.
fn allow_all_but(denied: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*"},
            {"Effect": "Deny", "Action": format!("{ACTION_PREFIX}:{denied}"),
             "Resource": resource},
        ],
    })
    .to_string()
}

/// A definition of a published configuration type, deployed through an organization so it
/// needs no local-deployment role: no case here is about the definition.
fn sound_definition() -> Value {
    let type_ = service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG[0]
        .type_;
    json!({
        "Type": type_,
        "Parameters": super::type_parameter_fixtures::with_required_parameters(
            type_,
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" }),
        ),
    })
}

/// Create one configuration manager with the account's own credential, answering its ARN.
async fn create_manager(aws: &Emulator, name: &str) -> String {
    let (status, answer) = aws
        .call_as(
            ACCOUNT_KEY,
            "POST",
            "/configurationManager",
            Some(&json!({ "Name": name, "ConfigurationDefinitions": [sound_definition()] })),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: the create must be served, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("setup {name:?}: the create answers a ManagerArn, got {answer:?}")
        })
        .to_string()
}

/// The error shape a failed call reported, with any namespace stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, under either casing an error structure's single message
/// member may render as.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// IAM's refusal sentence, spelled here independently of the crate under test: who was
/// refused, the action, the resource the request named, and which of IAM's rules refused.
fn not_authorized(principal: &str, operation: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {ACTION_PREFIX}:{operation} \
         on resource: {resource} {reason}"
    )
}

/// The reason clause for a request an explicit `Deny` refuses.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The reason clause for a request no readable document allows — IAM's default deny.
fn default_deny(operation: &str) -> String {
    format!("because no identity-based policy allows the {ACTION_PREFIX}:{operation} action")
}

/// Assert one authorization refusal: the status, the wire code and IAM's whole sentence.
fn assert_refused(case: &str, status: u16, answer: &Value, expected: &str) {
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} / {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED}, got {code:?} in {answer:?}"
    );
    let message = error_message(answer);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// RULE over the READ of one configuration manager: whichever documents the calling
/// principal holds, the decision is IAM's — an explicit `Deny` about the request refuses
/// it with HTTP 403 `AccessDeniedException` and IAM's explicit-deny sentence, a principal
/// with no grant falls to the default deny, and a grant that covers the request serves it.
#[tokio::test]
async fn a_read_is_decided_against_the_callers_identity_policies() {
    let aws = Emulator::new();
    let manager = create_manager(&aws, "authorized-read").await;
    let other_manager = create_manager(&aws, "another-manager").await;

    // (case, the role's inline documents, whether the read is served, the reason clause a
    // refusal must state)
    let rows: Vec<(&str, Vec<String>, bool, String)> = vec![
        (
            "no document at all",
            Vec::new(),
            false,
            default_deny("GetConfigurationManager"),
        ),
        (
            "a grant of every action of this service",
            vec![statement(
                "Allow",
                &[&format!("{ACTION_PREFIX}:*")],
                ANY_RESOURCE,
            )],
            true,
            String::new(),
        ),
        (
            "a grant scoped to the manager named",
            vec![statement(
                "Allow",
                &[&format!("{ACTION_PREFIX}:GetConfigurationManager")],
                &manager,
            )],
            true,
            String::new(),
        ),
        (
            "a grant scoped to another manager",
            vec![statement(
                "Allow",
                &[&format!("{ACTION_PREFIX}:GetConfigurationManager")],
                &other_manager,
            )],
            false,
            default_deny("GetConfigurationManager"),
        ),
        (
            "a grant of another action only",
            vec![statement(
                "Allow",
                &[&format!("{ACTION_PREFIX}:ListConfigurationManagers")],
                ANY_RESOURCE,
            )],
            false,
            default_deny("GetConfigurationManager"),
        ),
        (
            "a broad grant plus an explicit deny of the read",
            vec![allow_all_but("GetConfigurationManager", ANY_RESOURCE)],
            false,
            EXPLICIT_DENY.to_string(),
        ),
        (
            "a deny scoped to the very manager named",
            vec![allow_all_but("GetConfigurationManager", &manager)],
            false,
            EXPLICIT_DENY.to_string(),
        ),
        (
            "a deny scoped to another manager",
            vec![allow_all_but("GetConfigurationManager", &other_manager)],
            true,
            String::new(),
        ),
        (
            "a grant in one document and the deny in another",
            vec![
                statement("Allow", &[&format!("{ACTION_PREFIX}:*")], ANY_RESOURCE),
                statement(
                    "Deny",
                    &[&format!("{ACTION_PREFIX}:GetConfigurationManager")],
                    ANY_RESOURCE,
                ),
            ],
            false,
            EXPLICIT_DENY.to_string(),
        ),
    ];

    for (index, (case, documents, served, reason)) in rows.iter().enumerate() {
        let principal = assumed_role(&aws, &format!("reader-{index}"), documents).await;
        let (status, answer) = aws
            .call_as(
                &principal.access_key_id,
                "GET",
                &format!("/configurationManager/{}", manager.replace('/', "%2F")),
                None,
            )
            .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status} / {answer:?}"
            );
            assert_eq!(
                answer.get("ManagerArn").and_then(Value::as_str),
                Some(manager.as_str()),
                "case {case:?}: expected the manager that was read back, got {answer:?}"
            );
        } else {
            assert_refused(
                case,
                status,
                &answer,
                &not_authorized(
                    &principal.arn,
                    "GetConfigurationManager",
                    &manager,
                    reason,
                ),
            );
        }
    }
}

/// RULE: the decision is taken per ACTION, so a deny of one operation leaves every other
/// action the same principal is granted untouched — and the resource a call that names no
/// ARN is judged against is IAM's `*`.
#[tokio::test]
async fn a_deny_of_one_action_leaves_the_others_granted() {
    let aws = Emulator::new();
    let manager = create_manager(&aws, "one-action-denied").await;
    let principal = assumed_role(
        &aws,
        "reader-of-lists",
        &[allow_all_but("GetConfigurationManager", ANY_RESOURCE)],
    )
    .await;

    let (status, answer) = aws
        .call_as(
            &principal.access_key_id,
            "POST",
            "/listConfigurationManagers",
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the listing the deny does not name\": expected it served, got {status} / \
         {answer:?}"
    );

    let (status, answer) = aws
        .call_as(
            &principal.access_key_id,
            "GET",
            &format!("/configurationManager/{}", manager.replace('/', "%2F")),
            None,
        )
        .await;
    assert_refused(
        "the read the deny names",
        status,
        &answer,
        &not_authorized(
            &principal.arn,
            "GetConfigurationManager",
            &manager,
            EXPLICIT_DENY,
        ),
    );

    // A principal granted nothing at all is refused the listing too, and that refusal
    // names IAM's own `*` as the resource, since the call addresses none.
    let stranger = assumed_role(&aws, "stranger", &[]).await;
    let (status, answer) = aws
        .call_as(
            &stranger.access_key_id,
            "POST",
            "/listConfigurationManagers",
            Some(&json!({})),
        )
        .await;
    assert_refused(
        "a listing no document allows",
        status,
        &answer,
        &not_authorized(
            &stranger.arn,
            "ListConfigurationManagers",
            ANY_RESOURCE,
            &default_deny("ListConfigurationManagers"),
        ),
    );
}

/// RULE: the decision is taken before anything is read or written, so a refused MUTATION
/// changes nothing — and the same rule decides the tag plane, whose route labels the
/// resource's own ARN.
#[tokio::test]
async fn a_refused_mutation_changes_nothing() {
    let aws = Emulator::new();
    let manager = create_manager(&aws, "mutation-denied").await;
    let label = manager.replace('/', "%2F");
    let principal = assumed_role(
        &aws,
        "tagger",
        &[
            allow_all_but("DeleteConfigurationManager", ANY_RESOURCE),
            statement(
                "Deny",
                &[&format!("{ACTION_PREFIX}:ListTagsForResource")],
                ANY_RESOURCE,
            ),
        ],
    )
    .await;

    // (case, method, path, body, the operation the refusal names, the resource it names)
    let rows: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "the delete the deny names",
            "DELETE",
            format!("/configurationManager/{label}"),
            None,
            "DeleteConfigurationManager",
        ),
        (
            "the tag read the deny names",
            "GET",
            format!("/tags/{label}"),
            None,
            "ListTagsForResource",
        ),
    ];
    for (case, method, path, body, operation) in rows {
        let (status, answer) = aws
            .call_as(&principal.access_key_id, method, &path, body.as_ref())
            .await;
        assert_refused(
            case,
            status,
            &answer,
            &not_authorized(&principal.arn, operation, &manager, EXPLICIT_DENY),
        );
    }

    // Nothing the refusals touched: the manager is still there, still readable, and its
    // tags are still the ones the create left.
    let (status, answer) = aws
        .call_as(ACCOUNT_KEY, "GET", &format!("/configurationManager/{label}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case \"the manager after the refused delete\": expected it still served, got \
         {status} / {answer:?}"
    );
    let (status, answer) = aws
        .call_as(ACCOUNT_KEY, "GET", &format!("/tags/{label}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case \"the tags after the refused read\": expected them still served, got {status} \
         / {answer:?}"
    );
}

/// RULE: the account's own published credential is not a principal the IAM store holds
/// policies for — the position AWS account root credentials are in — so every operation
/// stays served for it, whatever documents other principals carry.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let aws = Emulator::new();
    let manager = create_manager(&aws, "root-served").await;
    let label = manager.replace('/', "%2F");
    // A role denying everything exists in the account; it decides nothing about a caller
    // that is not that role.
    assumed_role(
        &aws,
        "denied-everything",
        &[statement(
            "Deny",
            &[&format!("{ACTION_PREFIX}:*")],
            ANY_RESOURCE,
        )],
    )
    .await;

    // (case, method, path, body)
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "the read of a manager",
            "GET",
            format!("/configurationManager/{label}"),
            None,
        ),
        (
            "the listing of managers",
            "POST",
            "/listConfigurationManagers".to_string(),
            Some(json!({})),
        ),
        (
            "the published type catalog",
            "GET",
            "/listQuickSetupTypes".to_string(),
            None,
        ),
        (
            "the service settings",
            "GET",
            "/serviceSettings".to_string(),
            None,
        ),
        (
            "the tag plane",
            "GET",
            format!("/tags/{label}"),
            None,
        ),
    ];
    for (case, method, path, body) in rows {
        let (status, answer) = aws
            .call_as(ACCOUNT_KEY, method, &path, body.as_ref())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the account's own credential to be served, got \
             {status} / {answer:?}"
        );
    }
}
