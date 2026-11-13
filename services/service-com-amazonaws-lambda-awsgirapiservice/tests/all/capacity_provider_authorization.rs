// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of the capacity-provider surface, over the restJson1 wire.
//!
//! RULE under test: a request to one of this surface's operations is served only to a caller
//! whose identity-based IAM policies allow THAT operation's own action
//! (`lambda:<OperationName>`) on the resource the request addresses — the capacity provider's
//! ARN, and the any-resource wildcard for the collection read that addresses no provider of
//! its own. Every other caller is refused `AccessDeniedException` (HTTP 403) carrying the one
//! sentence IAM words an authorization failure with, BEFORE the operation reads or writes
//! anything: a refused mutation changes nothing a later caller sees, and a refused read
//! reports nothing.
//!
//! The decision is the real one: each row's role and its inline policy are made with IAM's and
//! STS's OWN operations over the same engine (`CreateRole`, `PutRolePolicy`, `AssumeRole`), so
//! the documents the gate reads are the ones those calls wrote, and the caller signs with the
//! access key id `AssumeRole` answered. Every assertion about what the store holds is read back
//! through `GetCapacityProvider`, so an answer that was never persisted — or a write that
//! landed in spite of a refusal — fails here.
//!
//! The rows differ along what the rule generalises over: the OPERATION asked for (a create, a
//! read of one provider, an update, a delete, a child listing and a collection read — each with
//! its own action, its own HTTP method and its own kind of resource), which of IAM's rules
//! withheld the grant (an explicit `Deny` beside a wildcard allow, and the default deny of a
//! document that states nothing about the action), and WHICH RESOURCE the grant is written
//! about (this provider, another provider, the resource-type wildcard, another service's ARN,
//! every resource).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where the capacity-provider operations are bound, from
/// `smithy_query(query="operation", operation="GetCapacityProvider")`
/// (`/2025-11-30/capacity-providers/{CapacityProviderName}`); the collection itself is the
/// same space without a label, and the child listing is `…/function-versions`.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// The Region every request below signs for and addresses.
const REGION: &str = "us-east-1";

/// The access key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's own identity, which holds no IAM
/// policies — the position AWS account root credentials are in. The fixtures are built with it,
/// because this gate serves it.
const ACCOUNT_KEY_ID: &str = "AKIDTESTONLYACCOUNT1";

/// The name of the inline policy each row's role carries. `iam:PutRolePolicy` REPLACES the
/// policy of that name, so one role can be re-judged row by row.
const POLICY_NAME: &str = "inline";

/// The session name each role is assumed under.
const SESSION: &str = "probe";

/// The role every provider below is created with as its operator role, and the service
/// principal its trust policy admits — this service is what manages a provider's compute as
/// that role.
const OPERATOR_ROLE_NAME: &str = "lambda-capacity-operator";
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The subnet every provider below is created in, in the published `subnet-` id form.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// The `MaxVCpuCount` a provider is created with, and the one an update asks for. Both are
/// inside the model's `@range(min: 2, max: 15000)`
/// (`smithy_query(query="shape", shape="CapacityProviderScalingConfig")`), and they differ, so
/// which of the two a later read reports says whether a write landed.
const CREATED_MAX_VCPU: i64 = 8;
const REQUESTED_MAX_VCPU: i64 = 64;

/// The wire name and status of the refusal, which no operation of this service models: the
/// service answers the AWS common `AccessDeniedException` at HTTP 403.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The reason clauses IAM's refusal ends in, as `service_common::iam_authorization` words them.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The account id every ARN of this emulator carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn` pattern
/// spells it — the resource a call addressed at that provider is judged against.
fn provider_arn(name: &str) -> String {
    format!(
        "arn:aws:lambda:{REGION}:{}:capacity-provider:{name}",
        account()
    )
}

/// The ARN the account holds the role `role` under.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", account())
}

/// The principal ARN a session assumed from `role` speaks for — what a refusal must name.
fn session_arn(role: &str) -> String {
    format!("arn:aws:sts::{}:assumed-role/{role}/{SESSION}", account())
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
    error_type: Option<String>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn error_code(&self) -> String {
        self.error_type
            .clone()
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }

    /// The `Message` an error body carries — this service spells the member with a capital, as
    /// every error structure it models does.
    fn message(&self) -> String {
        let document = self.json();
        document
            .get("Message")
            .or_else(|| document.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Lambda beside the IAM and STS services over ONE shared engine, which is what `core/router`
/// assembles.
fn router_with_identity() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Percent-encode one component of an `application/x-www-form-urlencoded` body, which is how an
/// `awsQuery` parameter travels.
fn form_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The text of the first `<tag>` element of an XML body.
fn xml_element(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One Lambda request signed with `access_key_id`.
async fn lambda(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/lambda/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        error_type: parts
            .headers
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body,
    }
}

/// One `awsQuery` call to IAM or STS, signed with the account's own credential.
async fn query(
    router: &MultiServiceRouter,
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
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Create the role `role` with a trust policy admitting `principal` through `iam:CreateRole`.
async fn given_role(router: &MultiServiceRouter, case: &str, role: &str, principal: &str) {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole must succeed, got {status} {body}"
    );
}

/// Put `document` on `role` as its inline policy through `iam:PutRolePolicy`, which REPLACES
/// the policy of that name.
async fn given_role_policy(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
    document: &Value,
) {
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", role.to_string()),
            ("PolicyName", POLICY_NAME.to_string()),
            ("PolicyDocument", document.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy must succeed, got {status} {body}"
    );
}

/// Create the role `role` with a trust policy the account itself satisfies, put `policy` on it
/// and assume it through `sts:AssumeRole`: the answer is the access key id of the session, which
/// is the credential a row signs its call with.
async fn given_assumed_role(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
    policy: &Value,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole must succeed, got {status} {body}"
    );
    given_role_policy(router, case, role, policy).await;
    let (status, body) = query(
        router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", role_arn(role)),
            ("RoleSessionName", SESSION.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole must succeed, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId").unwrap_or_else(|| {
        panic!("case {case:?}: AssumeRole must answer an AccessKeyId, got {body}")
    })
}

/// The body a create carries: the required VPC and permissions configuration, plus the scaling
/// configuration whose `MaxVCpuCount` says later whether an update landed.
fn create_body(name: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "CapacityProviderName": name,
        "VpcConfig": { "SubnetIds": [SUBNET], "SecurityGroupIds": [] },
        "PermissionsConfig": {
            "CapacityProviderOperatorRoleArn": role_arn(OPERATOR_ROLE_NAME),
        },
        "CapacityProviderScalingConfig": { "MaxVCpuCount": CREATED_MAX_VCPU },
    }))
    .expect("the create body serializes")
}

/// Create the provider `name` with the account's own credential, which this gate serves.
async fn given_provider(router: &MultiServiceRouter, case: &str, name: &str) {
    let created = lambda(
        router,
        ACCOUNT_KEY_ID,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &create_body(name),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateCapacityProvider must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// `GetCapacityProvider` on `name` with the account's own credential: the provider as the store
/// holds it, or `None` when the account holds none under that name.
async fn stored_provider(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let read = lambda(
        router,
        ACCOUNT_KEY_ID,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{name}"),
        b"",
    )
    .await;
    if read.status == 404 {
        return None;
    }
    assert_eq!(
        read.status, 200,
        "GetCapacityProvider as the account must be served, got {} {}",
        read.status,
        read.text()
    );
    read.json()
        .get("CapacityProvider")
        .cloned()
        .or(Some(Value::Null))
}

/// The `MaxVCpuCount` the store holds for `name`, which is what says whether an update landed.
async fn stored_max_vcpu(router: &MultiServiceRouter, name: &str) -> Option<i64> {
    stored_provider(router, name)
        .await?
        .get("CapacityProviderScalingConfig")?
        .get("MaxVCpuCount")?
        .as_i64()
}

/// A policy document with one statement per `(effect, action, resource)` triple.
fn policy(statements: &[(&str, &str, String)]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": statements
            .iter()
            .map(|(effect, action, resource)| json!({
                "Effect": effect,
                "Action": action,
                "Resource": resource,
            }))
            .collect::<Vec<Value>>(),
    })
}

/// One operation of the capacity-provider surface: its action, its wire binding, the resource a
/// call to it is judged against, and what a served call answers.
#[derive(Clone, Copy, Debug)]
enum Call {
    Create,
    Get,
    Update,
    Delete,
    ListFunctionVersions,
    List,
}

impl Call {
    /// Every operation of this surface, which is the set the rule covers
    /// (`smithy_query(query="list_operations")`, the operations bound to the
    /// capacity-provider space).
    const ALL: [Self; 6] = [
        Self::Create,
        Self::Get,
        Self::Update,
        Self::Delete,
        Self::ListFunctionVersions,
        Self::List,
    ];

    /// The operation's own name, which is also the name of the IAM action a policy states about
    /// it (`lambda:<OperationName>`).
    fn operation(self) -> &'static str {
        match self {
            Self::Create => "CreateCapacityProvider",
            Self::Get => "GetCapacityProvider",
            Self::Update => "UpdateCapacityProvider",
            Self::Delete => "DeleteCapacityProvider",
            Self::ListFunctionVersions => "ListFunctionVersionsByCapacityProvider",
            Self::List => "ListCapacityProviders",
        }
    }

    /// The IAM action the call is performed under.
    fn action(self) -> String {
        format!("lambda:{}", self.operation())
    }

    /// The status a SERVED call answers, from each operation's own modelled success code.
    fn served_status(self) -> u16 {
        match self {
            Self::Create | Self::Update | Self::Delete => 202,
            Self::Get | Self::ListFunctionVersions | Self::List => 200,
        }
    }

    /// The resource the call is judged against: the ARN of the provider it addresses, and the
    /// any-resource wildcard for the collection read, which addresses no provider of its own.
    fn resource(self, provider: &str) -> String {
        match self {
            Self::List => "*".to_string(),
            _ => provider_arn(provider),
        }
    }

    /// Whether a refused call could have changed the store, which is what the refusal has to
    /// have left alone.
    fn mutates(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Delete)
    }

    /// Make the call on `provider`, signed with `access_key_id`.
    async fn dispatch(
        self,
        router: &MultiServiceRouter,
        access_key_id: &str,
        provider: &str,
    ) -> Answer {
        let addressed = format!("{CAPACITY_PROVIDERS_URI}/{provider}");
        match self {
            Self::Create => {
                lambda(
                    router,
                    access_key_id,
                    "POST",
                    CAPACITY_PROVIDERS_URI,
                    &create_body(provider),
                )
                .await
            }
            Self::Get => lambda(router, access_key_id, "GET", &addressed, b"").await,
            Self::Update => {
                lambda(
                    router,
                    access_key_id,
                    "PUT",
                    &addressed,
                    &serde_json::to_vec(&json!({
                        "CapacityProviderScalingConfig": { "MaxVCpuCount": REQUESTED_MAX_VCPU },
                    }))
                    .expect("the update body serializes"),
                )
                .await
            }
            Self::Delete => lambda(router, access_key_id, "DELETE", &addressed, b"").await,
            Self::ListFunctionVersions => {
                lambda(
                    router,
                    access_key_id,
                    "GET",
                    &format!("{addressed}/function-versions"),
                    b"",
                )
                .await
            }
            Self::List => {
                lambda(
                    router,
                    access_key_id,
                    "GET",
                    CAPACITY_PROVIDERS_URI,
                    b"",
                )
                .await
            }
        }
    }
}

/// Assert `answered` is IAM's refusal, naming the principal, the action, the resource and
/// `reason` — the one sentence AWS words an authorization failure with.
fn assert_refused(
    case: &str,
    answered: &Answer,
    role: &str,
    action: &str,
    resource: &str,
    reason: &str,
) {
    assert_eq!(
        answered.status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.error_code(),
        ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED}, got {}",
        answered.text()
    );
    assert_eq!(
        answered.message(),
        format!(
            "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
            session_arn(role)
        ),
        "case {case:?}: expected IAM's wording, got {}",
        answered.text()
    );
    assert!(
        answered.json().get("CapacityProvider").is_none(),
        "case {case:?}: a refused call must report no capacity provider, got {}",
        answered.text()
    );
}

/// RULE (the refusing half): a call no document in force allows is refused `AccessDeniedException`
/// (HTTP 403) with IAM's own sentence, and the refusal happens before the operation reads or
/// writes anything — so a refused create mints nothing, a refused update leaves the stored
/// configuration alone and a refused delete moves no state.
///
/// The rows differ along the OPERATION asked for (every one of this surface) and along which of
/// IAM's rules withheld the grant: an explicit `Deny` of that one action beside an allow of every
/// action of this service, and the default deny of a document that allows another service only.
#[tokio::test]
async fn a_caller_the_documents_do_not_allow_is_refused_and_changes_nothing() {
    let router = router_with_identity();
    given_role(&router, "the fixture", OPERATOR_ROLE_NAME, LAMBDA_PRINCIPAL).await;
    // One role for every row: no ceiling is in force here and `PutRolePolicy` REPLACES the
    // policy of that name, so each row is judged on its own document alone.
    let role = "capacity-provider-caller";
    let session_key = given_assumed_role(
        &router,
        "the caller",
        role,
        &policy(&[("Allow", "lambda:*", "*".to_string())]),
    )
    .await;

    for (index, call) in Call::ALL.iter().enumerate() {
        // The provider a row addresses: a create addresses the one it is about to mint, which
        // the account must therefore NOT hold yet; every other call addresses a provider the
        // account holds, created with the account's own credential.
        let provider = format!("refused-{index}");
        if !matches!(call, Call::Create) {
            given_provider(&router, call.operation(), &provider).await;
        }

        // (case, the document in force, the reason IAM's refusal ends in)
        let rows: Vec<(String, Value, String)> = vec![
            (
                format!("{}: an explicit deny beside a wildcard allow", call.operation()),
                policy(&[
                    ("Allow", "lambda:*", "*".to_string()),
                    ("Deny", &call.action(), "*".to_string()),
                ]),
                EXPLICIT_DENY.to_string(),
            ),
            (
                format!("{}: a document allowing another service only", call.operation()),
                policy(&[("Allow", "s3:*", "*".to_string())]),
                format!(
                    "because no identity-based policy allows the {} action",
                    call.action()
                ),
            ),
        ];

        for (case, document, reason) in &rows {
            given_role_policy(&router, case, role, document).await;
            let answered = call.dispatch(&router, &session_key, &provider).await;
            assert_refused(
                case,
                &answered,
                role,
                &call.action(),
                &call.resource(&provider),
                reason,
            );

            // The refusal changed nothing: the create minted no provider, and the update and
            // the delete left the record exactly as the account created it.
            if call.mutates() {
                match call {
                    Call::Create => assert!(
                        stored_provider(&router, &provider).await.is_none(),
                        "case {case:?}: a refused create must mint nothing"
                    ),
                    _ => {
                        let held = stored_provider(&router, &provider)
                            .await
                            .unwrap_or_else(|| panic!("case {case:?}: the provider must remain"));
                        assert_eq!(
                            held.get("CapacityProviderScalingConfig")
                                .and_then(|config| config.get("MaxVCpuCount"))
                                .and_then(Value::as_i64),
                            Some(CREATED_MAX_VCPU),
                            "case {case:?}: expected the created scaling configuration, got {held}"
                        );
                        assert_eq!(
                            held.get("State").and_then(Value::as_str),
                            Some("Pending"),
                            "case {case:?}: a refused call must move no state, got {held}"
                        );
                    }
                }
            }
        }
    }
}

/// RULE (the serving half): a caller whose document allows exactly that operation's own action on
/// exactly the resource the request addresses is served, and what it asked for really happens —
/// the create is readable afterwards, the update's value is what a later read reports, and the
/// delete moves the provider into the state a deletion in progress carries.
///
/// The rows differ along the OPERATION asked for: every one of this surface, each with its own
/// action, its own method and its own success code.
#[tokio::test]
async fn a_grant_of_exactly_the_action_on_exactly_the_resource_serves_the_call() {
    let router = router_with_identity();
    given_role(&router, "the fixture", OPERATOR_ROLE_NAME, LAMBDA_PRINCIPAL).await;
    let role = "capacity-provider-grantee";
    let session_key = given_assumed_role(
        &router,
        "the caller",
        role,
        &policy(&[("Allow", "lambda:*", "*".to_string())]),
    )
    .await;

    for (index, call) in Call::ALL.iter().enumerate() {
        let case = format!("{} granted on the resource it addresses", call.operation());
        let provider = format!("granted-{index}");
        if !matches!(call, Call::Create) {
            given_provider(&router, &case, &provider).await;
        }
        // Exactly this action, on exactly the resource this call is judged against: a grant any
        // narrower would not reach it, and this one needs nothing wider.
        given_role_policy(
            &router,
            &case,
            role,
            &policy(&[("Allow", &call.action(), call.resource(&provider))]),
        )
        .await;

        let answered = call.dispatch(&router, &session_key, &provider).await;
        assert_eq!(
            answered.status,
            call.served_status(),
            "case {case:?}: expected HTTP {}, got {} {}",
            call.served_status(),
            answered.status,
            answered.text()
        );

        // What the served call did is read back through the store, with the account's own
        // credential, so an answer that was never persisted fails here.
        match call {
            Call::Create => assert_eq!(
                stored_max_vcpu(&router, &provider).await,
                Some(CREATED_MAX_VCPU),
                "case {case:?}: the created provider must be readable afterwards"
            ),
            Call::Update => assert_eq!(
                stored_max_vcpu(&router, &provider).await,
                Some(REQUESTED_MAX_VCPU),
                "case {case:?}: the update must have landed"
            ),
            Call::Delete => assert_eq!(
                stored_provider(&router, &provider)
                    .await
                    .as_ref()
                    .and_then(|held| held.get("State"))
                    .and_then(Value::as_str),
                Some("Deleting"),
                "case {case:?}: the delete must have moved the provider's state"
            ),
            Call::Get => assert_eq!(
                answered
                    .json()
                    .get("CapacityProvider")
                    .and_then(|held| held.get("CapacityProviderArn"))
                    .and_then(Value::as_str),
                Some(provider_arn(&provider).as_str()),
                "case {case:?}: expected the addressed provider, got {}",
                answered.text()
            ),
            Call::ListFunctionVersions => assert_eq!(
                answered
                    .json()
                    .get("CapacityProviderArn")
                    .and_then(Value::as_str),
                Some(provider_arn(&provider).as_str()),
                "case {case:?}: expected the addressed provider, got {}",
                answered.text()
            ),
            Call::List => assert!(
                answered
                    .json()
                    .get("CapacityProviders")
                    .and_then(Value::as_array)
                    .is_some_and(|held| !held.is_empty()),
                "case {case:?}: expected the account's providers, got {}",
                answered.text()
            ),
        }
    }
}

/// RULE (the resource half): the grant has to be written about the resource the request
/// ADDRESSES — a provider's own ARN — so a policy scoped to another provider does not reach this
/// one, while a wildcard that covers this provider's ARN does.
///
/// The rows differ along the `Resource` element the grant names, and the rule is asserted for a
/// WRITE and a READ alike, since the resource a call is judged against is a property of the
/// request rather than of what the handler goes on to do.
#[tokio::test]
async fn the_grant_has_to_be_written_about_the_provider_the_request_addresses() {
    let router = router_with_identity();
    given_role(&router, "the fixture", OPERATOR_ROLE_NAME, LAMBDA_PRINCIPAL).await;
    let role = "capacity-provider-scoped";
    let session_key = given_assumed_role(
        &router,
        "the caller",
        role,
        &policy(&[("Allow", "lambda:*", "*".to_string())]),
    )
    .await;
    let provider = "scoped";
    given_provider(&router, "the fixture", provider).await;

    // (case, the `Resource` element of the one allow, whether the call is served)
    let rows: Vec<(&str, String, bool)> = vec![
        ("this provider's own ARN", provider_arn(provider), true),
        (
            "every capacity provider of this account and Region",
            provider_arn("*"),
            true,
        ),
        ("every resource", "*".to_string(), true),
        ("another provider's ARN", provider_arn("elsewhere"), false),
        (
            "another resource type of this service",
            format!("arn:aws:lambda:{REGION}:{}:function:orders", account()),
            false,
        ),
        (
            "another service's resource",
            "arn:aws:s3:::reports".to_string(),
            false,
        ),
    ];

    // A write and a read, each judged on the provider the request addresses.
    for call in [Call::Update, Call::Get] {
        for (case, resource, served) in &rows {
            let case = format!("{}: a grant on {case}", call.operation());
            given_role_policy(
                &router,
                &case,
                role,
                &policy(&[("Allow", "lambda:*", resource.clone())]),
            )
            .await;
            let answered = call.dispatch(&router, &session_key, provider).await;
            if *served {
                assert_eq!(
                    answered.status,
                    call.served_status(),
                    "case {case:?}: expected HTTP {}, got {} {}",
                    call.served_status(),
                    answered.status,
                    answered.text()
                );
            } else {
                assert_refused(
                    &case,
                    &answered,
                    role,
                    &call.action(),
                    &call.resource(provider),
                    &format!(
                        "because no identity-based policy allows the {} action",
                        call.action()
                    ),
                );
            }
        }
    }
}
