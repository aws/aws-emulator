// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals a request to a workspace can draw, end to end over
//! HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes.
//!
//! THE RULES UNDER TEST:
//!
//! 1. **A well-formed id that names no workspace** is 404 `ResourceNotFoundException`, and
//!    the message is composed from the request and the caller's account — the workspace id
//!    that was looked up and the account it was looked up in — as
//!    `Workspace with id=<workspaceId> of account=<accountId> does not exist.` The same
//!    rule holds for every operation that resolves a `workspaceId`, because it is the one
//!    lookup they all make.
//! 2. **A value outside `WorkspaceId`'s own `@pattern`** is 400 `ValidationException`, and
//!    the message is produced from the member's modelled constraint —
//!    `The <member> should satisfy the pattern <pattern>.` — naming the member and the
//!    pattern and echoing NO part of the rejected value, which is what keeps a caller's
//!    own string out of the answer.
//!
//! 3. **A member whose own modelling states a cross-member condition** — one member
//!    required by, or admitted only with, another member's value — has that condition
//!    judged against the record the request would LEAVE BEHIND, and a combination it
//!    rules out is 400 `ValidationException` naming the member at fault, with nothing
//!    written. On `UpdateWorkspace` those conditions are the `workspaceRoleArn` a request
//!    that sets `permissionType` to `CUSTOMER_MANAGED` must leave the workspace holding,
//!    and `organizationRoleName` being admitted only on a customer-managed workspace.
//!
//! The tables span what the rules generalise over: the operation (every implemented one
//! that takes a `workspaceId` in its URI, across all four HTTP methods this service uses),
//! the id (several well-formed ids nothing minted, including the published bounds of the
//! pattern's hex range), the WAY a value fails the pattern (prefix, length either side,
//! character class, emptiness, and a value carrying URI punctuation) and — for the
//! cross-member rule — which condition is broken, which member of the pair the request
//! carries, and whether the value that decides it comes from the request or from what an
//! earlier call stored.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `WorkspaceId`'s pattern, from `smithy_query(query="operation",
/// operation="DescribeWorkspace")` — `workspaceId` is `@required`, `@httpLabel` and
/// `@pattern("^g-[0-9a-f]{10}$")`. Spelled out here rather than imported, so the sentence
/// the service states is checked against the MODEL's pattern and not against whatever the
/// implementation happens to carry.
const WORKSPACE_ID_PATTERN: &str = "^g-[0-9a-f]{10}$";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Every implemented operation that carries a `workspaceId` in its request URI: the name,
/// the method, the URI under `/workspaces/<id>`, and a request body the operation would
/// otherwise accept.
///
/// Source: the operations' own `@http` bindings
/// (`smithy_query(query="digest")`), less the four this emulator still answers 501 for
/// (`AssociateLicense`, `DisassociateLicense`, `ListPermissions`, `UpdatePermissions`),
/// which refuse before they resolve anything.
fn workspace_operations() -> Vec<(&'static str, &'static str, &'static str, Option<Value>)> {
    vec![
        ("DescribeWorkspace", "GET", "", None),
        ("DeleteWorkspace", "DELETE", "", None),
        (
            "UpdateWorkspace",
            "PUT",
            "",
            Some(json!({"workspaceDescription": "unreachable"})),
        ),
        (
            "DescribeWorkspaceConfiguration",
            "GET",
            "/configuration",
            None,
        ),
        (
            "UpdateWorkspaceConfiguration",
            "PUT",
            "/configuration",
            Some(json!({"configuration": "{}"})),
        ),
        (
            "DescribeWorkspaceAuthentication",
            "GET",
            "/authentication",
            None,
        ),
        (
            "UpdateWorkspaceAuthentication",
            "POST",
            "/authentication",
            Some(json!({"authenticationProviders": ["AWS_SSO"]})),
        ),
        ("ListWorkspaceServiceAccounts", "GET", "/serviceaccounts", None),
        (
            "CreateWorkspaceServiceAccount",
            "POST",
            "/serviceaccounts",
            Some(json!({"name": "unreachable", "grafanaRole": "ADMIN"})),
        ),
        (
            "CreateWorkspaceApiKey",
            "POST",
            "/apikeys",
            Some(json!({"keyName": "unreachable", "keyRole": "ADMIN", "secondsToLive": 600})),
        ),
        ("DeleteWorkspaceApiKey", "DELETE", "/apikeys/unreachable", None),
        (
            "DeleteWorkspaceServiceAccount",
            "DELETE",
            "/serviceaccounts/1",
            None,
        ),
        (
            "ListWorkspaceServiceAccountTokens",
            "GET",
            "/serviceaccounts/1/tokens",
            None,
        ),
        (
            "CreateWorkspaceServiceAccountToken",
            "POST",
            "/serviceaccounts/1/tokens",
            Some(json!({"name": "unreachable", "secondsToLive": 600})),
        ),
        (
            "DeleteWorkspaceServiceAccountToken",
            "DELETE",
            "/serviceaccounts/1/tokens/1",
            None,
        ),
    ]
}

/// THE RULE: an id that satisfies the model's pattern but names no workspace is refused
/// with the sentence built from that id and the account it was looked up in — the same
/// sentence from every operation that resolves a `workspaceId`.
#[tokio::test]
async fn an_unknown_workspace_id_is_refused_with_the_id_and_the_account() {
    // Ids the pattern admits that no workspace carries: the two ends of the hex range the
    // pattern publishes, and ordinary values in between.
    let unknown_ids = [
        "g-0000000000",
        "g-ffffffffff",
        "g-0123456789",
        "g-abcdef0123",
    ];
    let account = ::engine::emulator_account_id();

    let router = emulator();
    for id in unknown_ids {
        let expected = format!("Workspace with id={id} of account={account} does not exist.");
        for (operation, method, suffix, body) in workspace_operations() {
            let case = format!("{operation} with {id}");
            let answer = call(
                &router,
                method,
                &format!("/workspaces/{id}{suffix}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.body["resourceId"].as_str().unwrap_or_default(),
                id,
                "case {case:?}: the refusal must name the id it could not find, got {:?}",
                answer.body
            );
        }
    }
}

/// THE RULE: a `workspaceId` outside the modelled pattern is refused with the member's own
/// constraint stated in this service's form — naming the member and the pattern, and
/// echoing no part of the value sent.
#[tokio::test]
async fn a_workspace_id_outside_the_pattern_is_refused_with_the_patterns_own_sentence() {
    // (case, the value sent) — the ways a value fails `^g-[0-9a-f]{10}$`.
    let malformed: [(&str, &str); 8] = [
        ("no g- prefix at all", "not-a-valid-workspace-id"),
        ("the prefix but no digits", "g-"),
        ("one hex digit too few", "g-012345678"),
        ("one hex digit too many", "g-0123456789a"),
        ("hex digits out of range", "g-0123456xyz"),
        ("upper-case hex digits, which the pattern does not admit", "g-ABCDEF0123"),
        ("a different prefix", "w-0123456789"),
        ("punctuation inside the id", "g-01234.6789"),
    ];
    let expected = format!("The workspaceId should satisfy the pattern {WORKSPACE_ID_PATTERN}.");

    let router = emulator();
    for (case, id) in malformed {
        for (operation, method, suffix, body) in workspace_operations() {
            let case = format!("{operation} with {case}");
            let answer = call(
                &router,
                method,
                &format!("/workspaces/{id}{suffix}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
            assert!(
                // A value that is itself a fragment of the sentence (the `g-` prefix the
                // pattern spells) is no evidence either way; every other one would be.
                expected.contains(id) || !answer.message().contains(id),
                "case {case:?}: the refusal must not echo the rejected value, got {:?}",
                answer.message()
            );
        }
    }
}

// --- the cross-member conditions an UpdateWorkspace request member documents -----------

/// `PermissionType`'s two values, its whole value set
/// (`smithy_query(query="shape",shape="PermissionType")`), spelled out here rather than
/// imported so a wrong value in the implementation fails.
const CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";
const SERVICE_MANAGED: &str = "SERVICE_MANAGED";

/// `WorkspaceStatus$ACTIVE`: the settled status a change to a workspace is accepted in.
const ACTIVE: &str = "ACTIVE";

/// The `reason` a `ValidationException` about one member's value carries, from
/// `ValidationExceptionReason`'s own value set.
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The IAM role a fixture create stores, and the one an update names: `IamRoleArn` is
/// modelled only `@length(min: 1, max: 2048)`, so any role ARN serves.
const CREATED_ROLE: &str = "arn:aws:iam::000000000000:role/GrafanaCreated";
const UPDATED_ROLE: &str = "arn:aws:iam::000000000000:role/GrafanaUpdated";

/// The Organizations role name an update names (`OrganizationRoleName`,
/// `@length(min: 1, max: 2048)`).
const ORGANIZATION_ROLE: &str = "AmazonGrafanaOrganizationsRole";

/// How long a case waits for the clock to settle a workspace before giving up. Generous:
/// what it waits for is a CONDITION a later read reports, never a span of time.
const CLOCK_DEADLINE: Duration = Duration::from_secs(30);

/// How often such a case looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/workspaces", Some(request)).await
}

async fn describe(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/workspaces/{id}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PUT", &format!("/workspaces/{id}"), Some(request)).await
}

/// The value a described workspace carries at `pointer`, or `Value::Null` when it carries
/// none there.
fn at(workspace: &Value, pointer: &str) -> Value {
    workspace.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The `WorkspaceDescription` an answer carries.
fn workspace_of(answer: &Answer) -> Value {
    answer.body["workspace"].clone()
}

/// Create a workspace and look again until it reports `ACTIVE`, then answer its id.
///
/// A create is ACCEPTED, not finished, and a change to a workspace whose own create is
/// still in flight is a conflict — so a case about something else lets the record settle
/// first. The loop waits for the CONDITION a read reports, never for a span of time.
async fn active_workspace(router: &MultiServiceRouter, request: Value) -> String {
    let created = create(router, request).await;
    assert_eq!(
        created.status, 202,
        "the fixture create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the fixture create must answer a workspace id: {:?}", created.body))
        .to_string();

    let deadline = Instant::now() + CLOCK_DEADLINE;
    loop {
        let described = describe(router, &id).await;
        if described.body["workspace"]["status"].as_str() == Some(ACTIVE) {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "the fixture workspace never reached {ACTIVE}, last read {} with {:?}",
            described.status,
            described.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A create that leaves the workspace `SERVICE_MANAGED` and holding no role.
fn service_managed_create() -> Value {
    json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": SERVICE_MANAGED,
        "authenticationProviders": ["AWS_SSO"],
    })
}

/// A create that stores a role WITHOUT making the workspace customer managed: the record a
/// later `permissionType: CUSTOMER_MANAGED` is satisfied by on its own.
fn service_managed_with_role_create() -> Value {
    json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": SERVICE_MANAGED,
        "authenticationProviders": ["AWS_SSO"],
        "workspaceRoleArn": CREATED_ROLE,
    })
}

/// A create of a customer-managed workspace holding no role: `CreateWorkspace` admits
/// one, and an update that names neither member of the pair is not judged against the
/// condition `permissionType` states for a request that sets it.
fn customer_managed_without_role_create() -> Value {
    json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": CUSTOMER_MANAGED,
        "authenticationProviders": ["AWS_SSO"],
    })
}

/// A create of a customer-managed workspace carrying the role such a workspace needs.
fn customer_managed_create() -> Value {
    json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": CUSTOMER_MANAGED,
        "authenticationProviders": ["AWS_SSO"],
        "workspaceRoleArn": CREATED_ROLE,
    })
}

/// THE RULE: `UpdateWorkspace` judges the cross-member conditions its request members
/// document against the record the request would leave behind, and refuses the
/// combinations they rule out with `ValidationException` naming the member at fault —
/// `workspaceRoleArn` is required of a `CUSTOMER_MANAGED` workspace, and
/// `organizationRoleName` is admitted only on one.
#[tokio::test]
async fn update_workspace_refuses_the_member_combinations_the_model_rules_out() {
    // (case, the create the stored record comes from, the update, the member at fault)
    let cases: Vec<(&str, Value, Value, &str)> = vec![
        (
            "the update turns a workspace holding no role customer managed",
            service_managed_create(),
            json!({"permissionType": CUSTOMER_MANAGED}),
            "workspaceRoleArn",
        ),
        (
            "the update re-states customer managed on a workspace that holds no role",
            customer_managed_without_role_create(),
            json!({"permissionType": CUSTOMER_MANAGED}),
            "workspaceRoleArn",
        ),
        (
            "the same, alongside a member the conditions do not mention",
            service_managed_create(),
            json!({"permissionType": CUSTOMER_MANAGED, "workspaceDescription": "customer managed"}),
            "workspaceRoleArn",
        ),
        (
            "organizationRoleName against a stored service-managed workspace",
            service_managed_create(),
            json!({"organizationRoleName": ORGANIZATION_ROLE}),
            "organizationRoleName",
        ),
        (
            "organizationRoleName beside a role, with the permission type left alone",
            service_managed_with_role_create(),
            json!({"organizationRoleName": ORGANIZATION_ROLE, "workspaceRoleArn": UPDATED_ROLE}),
            "organizationRoleName",
        ),
        (
            "organizationRoleName with the service-managed permission type in one request",
            customer_managed_create(),
            json!({"permissionType": SERVICE_MANAGED, "organizationRoleName": ORGANIZATION_ROLE}),
            "organizationRoleName",
        ),
    ];

    let router = emulator();
    for (case, created_with, patch, member) in cases {
        let id = active_workspace(&router, created_with).await;
        let before = workspace_of(&describe(&router, &id).await);

        let refused = update(&router, &id, patch).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["reason"].as_str().unwrap_or_default(),
            FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
            refused.body
        );
        let named: Vec<String> = refused.body["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| field["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            named.iter().any(|name| name == member),
            "case {case:?}: the refusal must name {member} in its fieldList, got {named:?}"
        );
        assert!(
            refused.message().contains(member) && refused.message().contains(CUSTOMER_MANAGED),
            "case {case:?}: the message must state the condition on {member} and \
             {CUSTOMER_MANAGED}, got {:?}",
            refused.message()
        );

        let after = workspace_of(&describe(&router, &id).await);
        assert_eq!(
            before, after,
            "case {case:?}: a refused update must leave the stored record untouched"
        );
    }
}

/// One accepted case: its name, the create the stored record comes from, the update sent,
/// and the (JSON pointer, value) pairs `DescribeWorkspace` must report afterwards.
type AcceptedCase = (&'static str, Value, Value, Vec<(&'static str, Value)>);

/// THE RULE, from the other side: the conditions are judged on the POST-update record, so
/// a request that satisfies them — with its own members, or with what an earlier call
/// stored — is accepted and persisted, and an update naming neither member of a pair is
/// never judged against them.
#[tokio::test]
async fn update_workspace_accepts_what_the_post_update_record_satisfies() {
    // (case, the create the stored record comes from, the update, what Describe must report)
    let cases: Vec<AcceptedCase> = vec![
        (
            "the role arrives in the same request as the permission type",
            service_managed_create(),
            json!({"permissionType": CUSTOMER_MANAGED, "workspaceRoleArn": UPDATED_ROLE}),
            vec![
                ("/permissionType", json!(CUSTOMER_MANAGED)),
                ("/workspaceRoleArn", json!(UPDATED_ROLE)),
            ],
        ),
        (
            "the role the create stored satisfies the permission type on its own",
            service_managed_with_role_create(),
            json!({"permissionType": CUSTOMER_MANAGED}),
            vec![
                ("/permissionType", json!(CUSTOMER_MANAGED)),
                ("/workspaceRoleArn", json!(CREATED_ROLE)),
            ],
        ),
        (
            "organizationRoleName on a workspace already customer managed",
            customer_managed_create(),
            json!({"organizationRoleName": ORGANIZATION_ROLE}),
            vec![
                ("/organizationRoleName", json!(ORGANIZATION_ROLE)),
                ("/permissionType", json!(CUSTOMER_MANAGED)),
            ],
        ),
        (
            "the permission type, the role and organizationRoleName in one request",
            service_managed_create(),
            json!({
                "permissionType": CUSTOMER_MANAGED,
                "workspaceRoleArn": UPDATED_ROLE,
                "organizationRoleName": ORGANIZATION_ROLE,
            }),
            vec![
                ("/permissionType", json!(CUSTOMER_MANAGED)),
                ("/workspaceRoleArn", json!(UPDATED_ROLE)),
                ("/organizationRoleName", json!(ORGANIZATION_ROLE)),
            ],
        ),
        (
            "an update naming neither member, on a service-managed workspace holding no role",
            service_managed_create(),
            json!({"workspaceDescription": "neither condition applies"}),
            vec![
                ("/description", json!("neither condition applies")),
                ("/permissionType", json!(SERVICE_MANAGED)),
                ("/workspaceRoleArn", Value::Null),
            ],
        ),
        (
            "an update naming neither member, on a customer-managed workspace holding no role",
            customer_managed_without_role_create(),
            json!({"workspaceName": "renamed"}),
            vec![
                ("/name", json!("renamed")),
                ("/permissionType", json!(CUSTOMER_MANAGED)),
                ("/workspaceRoleArn", Value::Null),
            ],
        ),
    ];

    let router = emulator();
    for (case, created_with, patch, expected) in cases {
        let id = active_workspace(&router, created_with).await;

        let accepted = update(&router, &id, patch).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: expected HTTP 202, got {} with {:?}",
            accepted.status, accepted.body
        );

        let answered = workspace_of(&accepted);
        let stored = workspace_of(&describe(&router, &id).await);
        for (pointer, value) in expected {
            assert_eq!(
                at(&stored, pointer),
                value,
                "case {case:?}: {pointer} expected {value:?}, got {:?}",
                at(&stored, pointer)
            );
            assert_eq!(
                at(&answered, pointer),
                value,
                "case {case:?}: UpdateWorkspace's own answer must match the stored record \
                 at {pointer}, got {:?}",
                at(&answered, pointer)
            );
        }
    }
}
