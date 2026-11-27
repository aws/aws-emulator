// SPDX-License-Identifier: CC0-1.0

//! The Amazon Managed Grafana workspace lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one the control plane states for its primary
//! resource: a workspace's members are whatever the create, or a later update, stored,
//! and every operation computes its answer from that record.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::{
    LIFECYCLE_TRANSITION, WORKSPACE_QUOTA,
};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The version a create that names none runs: `CreateWorkspaceRequest$grafanaVersion`
/// "defaults to the latest version", the newest version the service offers (the last entry
/// of the catalogue `ListVersions` publishes, which `list_versions` owns). Spelled out
/// here, independently of the constant the implementation carries, so a wrong default in
/// `src/state.rs` fails.
const LATEST_GRAFANA_VERSION: &str = "13.2";

/// `IPAddressType$IPv4` ("connections to this workspace can only be made over IPv4"), the
/// type a workspace created without naming one is reported with. Source: the model's own
/// `IPAddressType` values, `IPv4` | `DualStack`
/// (`smithy_query(query="shape",shape="IPAddressType")`).
const DEFAULT_IP_ADDRESS_TYPE: &str = "IPv4";

/// The largest page `ListWorkspacesRequest$maxResults` admits (`@range(min: 1, max: 100)`),
/// spelled out here rather than imported for the same reason.
const MAX_RESULTS_CEILING: i32 = 100;

/// `WorkspaceId`'s `^g-[0-9a-f]{10}$`, checked here rather than through the crate's own
/// predicate so a wrong predicate cannot make this test pass.
fn matches_workspace_id_pattern(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("g-") else {
        return false;
    };
    hex.len() == 10 && hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A SigV4 credential scope naming this service. It is what routes a request to the
/// grafana service.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The endpoint host every request below reaches this emulator on.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The endpoint a workspace is announced at: the address every request below reaches this
/// emulator on, spelled out here independently of the crate's own constants.
fn expected_endpoint() -> String {
    EMULATOR_HOST.to_string()
}

/// A create request with only the three required members, for a case that is about
/// something other than the optional ones.
fn minimal_create() -> Value {
    json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    })
}

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
    /// The `workspace` member every workspace operation but the list returns.
    fn workspace(&self) -> &Value {
        &self.body["workspace"]
    }

    /// The workspace id the answer carries, or the empty string.
    fn id(&self) -> String {
        self.workspace()["id"].as_str().unwrap_or_default().to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", EMULATOR_HOST)
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

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/workspaces", Some(request)).await
}

async fn describe(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/workspaces/{id}"), None).await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/workspaces{query}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PUT", &format!("/workspaces/{id}"), Some(request)).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "DELETE", &format!("/workspaces/{id}"), None).await
}

/// Wait out the span the clock owes a workspace its next state in: `ACTIVE` after a create
/// or an update, and its REMOVAL from the store after an accepted delete.
///
/// A change to a workspace whose previous accepted change is still in flight is refused
/// with `ConflictException`, so a case that is not about that gate lets the record settle
/// first — exactly as a client driving two changes in a row has to. The transition is owed
/// by the clock, not by a call: nothing is done here but wait.
async fn settled() {
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;
}

/// Create a workspace, wait for the clock to settle it, and answer its id: the fixture a
/// case that is about something other than the transition span needs.
async fn active_workspace(router: &MultiServiceRouter, request: Value) -> String {
    let created = create(router, request).await;
    assert_eq!(
        created.status, 202,
        "the fixture create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let id = created.id();
    settled().await;
    id
}

/// The ids `ListWorkspaces` reported, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWorkspaces must return a workspaces list: {:?}", answer.body))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Where a `CreateWorkspace` input member lands in `WorkspaceDescription`, as a JSON
/// pointer.
///
/// The table is the crate's `OPERATIONS.md` CreateWorkspace input list read against the
/// `WorkspaceDescription` shape; `None` marks an input member that shape does not carry.
/// An unmapped member panics rather than being skipped, so a case cannot quietly assert
/// nothing.
fn described_as(member: &str) -> Option<&'static str> {
    Some(match member {
        "accountAccessType" => "/accountAccessType",
        "authenticationProviders" => "/authentication/providers",
        "grafanaVersion" => "/grafanaVersion",
        "ipAddressType" => "/ipAddressType",
        "kmsKeyId" => "/kmsKeyId",
        "networkAccessControl" => "/networkAccessControl",
        "organizationRoleName" => "/organizationRoleName",
        "permissionType" => "/permissionType",
        "stackSetName" => "/stackSetName",
        "tags" => "/tags",
        "vpcConfiguration" => "/vpcConfiguration",
        "workspaceDataSources" => "/dataSources",
        "workspaceDescription" => "/description",
        "workspaceName" => "/name",
        "workspaceNotificationDestinations" => "/notificationDestinations",
        "workspaceOrganizationalUnits" => "/organizationalUnits",
        "workspaceRoleArn" => "/workspaceRoleArn",
        // `configuration` is reported by DescribeWorkspaceConfiguration and `clientToken`
        // by nothing at all: neither is a WorkspaceDescription member.
        "configuration" | "clientToken" => return None,
        other => panic!("this test names a CreateWorkspace member with no mapping: {other:?}"),
    })
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// `CreateWorkspace` persists what it was given and `DescribeWorkspace` reports that
/// record: for every input member the description carries, the described value is the
/// created one.
///
/// The rows differ along what the rule generalises over — which required values, how many
/// authentication providers, how many tags and data sources, whether an optional member is
/// present at all, and which Grafana version — so no row's shape can be special-cased.
#[tokio::test]
async fn a_created_workspace_is_described_from_what_was_stored() {
    let cases = vec![
        ("only the required members", minimal_create()),
        (
            "named, described and tagged",
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "CUSTOMER_MANAGED",
                "authenticationProviders": ["AWS_SSO"],
                "workspaceName": "demo",
                "workspaceDescription": "bootstrap demo",
                "tags": {"env": "test"},
            }),
        ),
        (
            "both authentication providers and no tags",
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "SERVICE_MANAGED",
                "authenticationProviders": ["AWS_SSO", "SAML"],
                "workspaceName": "two-providers",
                "tags": {},
            }),
        ),
        (
            "organization access with the organizational units it requires",
            json!({
                "accountAccessType": "ORGANIZATION",
                "permissionType": "CUSTOMER_MANAGED",
                "authenticationProviders": ["SAML"],
                "workspaceOrganizationalUnits": ["ou-1234-abcdefgh", "ou-1234-ijklmnop"],
                "organizationRoleName": "GrafanaOrganizationRole",
                "stackSetName": "grafana-stack-set",
            }),
        ),
        (
            "every list-valued member, with more than one entry each",
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "CUSTOMER_MANAGED",
                "authenticationProviders": ["SAML"],
                "workspaceDataSources": ["CLOUDWATCH", "PROMETHEUS", "XRAY"],
                "workspaceNotificationDestinations": ["SNS"],
                "workspaceRoleArn": "arn:aws:iam::000000000000:role/GrafanaWorkspaceRole",
                "tags": {"env": "test", "team": "observability", "cost-centre": "42"},
            }),
        ),
        (
            "a pinned Grafana version, networking and a KMS key",
            json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "CUSTOMER_MANAGED",
                "authenticationProviders": ["AWS_SSO"],
                "grafanaVersion": "9.4",
                "ipAddressType": "DualStack",
                "kmsKeyId": "alias/grafana",
                "configuration": "{\"plugins\":{\"pluginAdminEnabled\":true}}",
                "vpcConfiguration": {
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "subnetIds": ["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                },
                "networkAccessControl": {
                    "prefixListIds": ["pl-0123456789abcdef0"],
                    "vpceIds": ["vpce-0123456789abcdef0"],
                },
            }),
        ),
    ];

    let router = emulator();
    for (case, request) in cases {
        let created = create(&router, request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202, got {} with {:?}",
            created.status, created.body
        );

        let id = created.id();
        assert!(
            matches_workspace_id_pattern(&id),
            "case {case:?}: expected an id matching ^g-[0-9a-f]{{10}}$, got {id:?}"
        );
        assert_eq!(
            at(created.workspace(), "/endpoint"),
            json!(expected_endpoint()),
            "case {case:?}: the endpoint must be an address this emulator is reached at, \
             got {:?}",
            at(created.workspace(), "/endpoint")
        );
        assert_eq!(
            at(created.workspace(), "/status"),
            json!("CREATING"),
            "case {case:?}: a just-created workspace is still CREATING, got {:?}",
            at(created.workspace(), "/status")
        );

        let described = describe(&router, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeWorkspace, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            at(described.workspace(), "/id"),
            json!(id),
            "case {case:?}: DescribeWorkspace must report the id it was asked about"
        );

        for (member, expected) in request.as_object().expect("each case is a JSON object") {
            let Some(pointer) = described_as(member) else {
                continue;
            };
            let observed = at(described.workspace(), pointer);
            assert_eq!(
                &observed, expected,
                "case {case:?}: member {member:?} was stored as {expected:?} but {pointer} \
                 reports {observed:?}"
            );
        }

        // The members the model requires but the request left to the service.
        if request.get("grafanaVersion").is_none() {
            assert_eq!(
                at(described.workspace(), "/grafanaVersion"),
                json!(LATEST_GRAFANA_VERSION),
                "case {case:?}: an unspecified grafanaVersion defaults to the latest version"
            );
        }
        // `ipAddressType` is optional on the create, and a workspace created without one
        // supports IPv4 only — so it is REPORTED rather than left absent, whichever way the
        // request named it.
        if request.get("ipAddressType").is_none() {
            assert_eq!(
                at(described.workspace(), "/ipAddressType"),
                json!(DEFAULT_IP_ADDRESS_TYPE),
                "case {case:?}: a workspace created without an ipAddressType must report \
                 {DEFAULT_IP_ADDRESS_TYPE}, got {:?}",
                at(described.workspace(), "/ipAddressType")
            );
        }
        assert_eq!(
            at(described.workspace(), "/authentication/samlConfigurationStatus"),
            if request["authenticationProviders"]
                .as_array()
                .is_some_and(|providers| providers.iter().any(|p| p == "SAML"))
            {
                json!("NOT_CONFIGURED")
            } else {
                Value::Null
            },
            "case {case:?}: samlConfigurationStatus is reported only for a SAML workspace"
        );
        assert!(
            at(described.workspace(), "/created").is_number()
                && at(described.workspace(), "/modified").is_number(),
            "case {case:?}: created and modified must both be timestamps, got {:?} and {:?}",
            at(described.workspace(), "/created"),
            at(described.workspace(), "/modified")
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: the same token always names the workspace it
/// first created, a different token names a different one, and no token at all mints a new
/// workspace every time.
#[tokio::test]
async fn a_repeated_create_with_one_client_token_returns_one_workspace() {
    // Rows differ in token shape and in how often the create is repeated. The pattern
    // `ClientToken` allows is `^[!-~]{1,64}$`, so the bounds are a one-character token and
    // a 64-character one.
    let cases: Vec<(&str, String, usize)> = vec![
        ("a plain token, retried once", "token-a".to_string(), 2),
        ("the same token, retried four times", "token-b".to_string(), 4),
        ("the shortest token the pattern allows", "!".to_string(), 3),
        ("the longest token the pattern allows", "t".repeat(64), 2),
        ("a token of punctuation only", "~!@#$%^&*()_+".to_string(), 2),
    ];

    let router = emulator();
    let mut distinct = Vec::new();
    for (case, token, attempts) in &cases {
        let mut request = minimal_create();
        request["clientToken"] = json!(token);
        request["workspaceName"] = json!("idempotent");

        let mut ids = Vec::new();
        for attempt in 0..*attempts {
            let answer = create(&router, request.clone()).await;
            assert_eq!(
                answer.status, 202,
                "case {case:?}: attempt {attempt} expected HTTP 202, got {} with {:?}",
                answer.status, answer.body
            );
            ids.push(answer.id());
        }
        assert!(
            ids.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: one client token must name one workspace, got {ids:?}"
        );
        distinct.push(ids[0].clone());
    }

    // Two creates with no token at all are two workspaces.
    let first = create(&router, minimal_create()).await.id();
    let second = create(&router, minimal_create()).await.id();
    assert_ne!(
        first, second,
        "a create carrying no clientToken must mint a new workspace every time"
    );
    distinct.push(first);
    distinct.push(second);

    let mut unique = distinct.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        distinct.len(),
        "distinct client tokens must name distinct workspaces, got {distinct:?}"
    );

    let listed = listed_ids(&list(&router, "").await);
    assert_eq!(
        listed.len(),
        distinct.len(),
        "state must hold exactly the workspaces that were created, got {listed:?} for \
         {distinct:?}"
    );
}

/// `ListWorkspaces` is `@paginated(items: workspaces)`: whatever `maxResults` it is given,
/// following `nextToken` to the end enumerates every persisted workspace exactly once, in
/// one stable order, and no page is longer than was asked for.
#[tokio::test]
async fn list_workspaces_pages_over_one_stable_order() {
    let router = emulator();
    let mut created = Vec::new();
    for index in 0..7 {
        let mut request = minimal_create();
        request["workspaceName"] = json!(format!("page-{index}"));
        created.push(create(&router, request).await.id());
    }

    let unpaged = listed_ids(&list(&router, "").await);
    assert_eq!(
        unpaged, created,
        "an unpaged ListWorkspaces must report every workspace in the order its ids sort"
    );

    // `maxResults` is modelled @range(min: 1, max: 100): both bounds, and the sizes
    // between that cut the 7 workspaces into a different number of pages.
    for page_size in [1, 2, 3, 6, 7, MAX_RESULTS_CEILING] {
        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let answer = list(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = listed_ids(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page carried {} workspaces: {page:?}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case maxResults={page_size}: pagination did not terminate, walked {walked:?}"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case maxResults={page_size}: walking every page must enumerate every workspace \
             once, in the same order"
        );
    }

    // A token this operation never issued is a bad request, not an empty page.
    let refused = list(&router, "?nextToken=not-a-token").await;
    assert_eq!(
        refused.status, 400,
        "a nextToken that is not a workspace id must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "a refused nextToken must answer ValidationException, got {:?}",
        refused.body
    );
}

/// `UpdateWorkspace` mutates exactly the members the request carries: the named member
/// takes the new value, every other member keeps what the create stored, and `modified`
/// moves forward.
#[tokio::test]
async fn update_workspace_mutates_only_the_members_the_request_carries() {
    // The baseline carries a VPC configuration and a network access configuration so the
    // two removal instructions have something to remove.
    let baseline = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
        "workspaceName": "baseline",
        "workspaceDescription": "the baseline description",
        "workspaceDataSources": ["CLOUDWATCH"],
        "workspaceNotificationDestinations": ["SNS"],
        "workspaceRoleArn": "arn:aws:iam::000000000000:role/Baseline",
        "workspaceOrganizationalUnits": ["ou-1234-abcdefgh"],
        "ipAddressType": "IPv4",
        "tags": {"env": "test"},
        "vpcConfiguration": {
            "securityGroupIds": ["sg-0123456789abcdef0"],
            "subnetIds": ["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
        },
        "networkAccessControl": {
            "prefixListIds": ["pl-0123456789abcdef0"],
            "vpceIds": ["vpce-0123456789abcdef0"],
        },
    });

    let cases = vec![
        (
            "the name only",
            json!({"workspaceName": "renamed"}),
            "/name",
            json!("renamed"),
        ),
        (
            "the description only",
            json!({"workspaceDescription": "updated"}),
            "/description",
            json!("updated"),
        ),
        (
            "the account access type",
            json!({"accountAccessType": "ORGANIZATION"}),
            "/accountAccessType",
            json!("ORGANIZATION"),
        ),
        (
            "the permission type",
            json!({"permissionType": "SERVICE_MANAGED"}),
            "/permissionType",
            json!("SERVICE_MANAGED"),
        ),
        (
            "the IAM role",
            json!({"workspaceRoleArn": "arn:aws:iam::000000000000:role/Updated"}),
            "/workspaceRoleArn",
            json!("arn:aws:iam::000000000000:role/Updated"),
        ),
        (
            "a longer data source list",
            json!({"workspaceDataSources": ["ATHENA", "PROMETHEUS", "XRAY"]}),
            "/dataSources",
            json!(["ATHENA", "PROMETHEUS", "XRAY"]),
        ),
        (
            "an emptied notification destination list",
            json!({"workspaceNotificationDestinations": []}),
            "/notificationDestinations",
            json!([]),
        ),
        (
            "the IP address type",
            json!({"ipAddressType": "DualStack"}),
            "/ipAddressType",
            json!("DualStack"),
        ),
        (
            "a replaced VPC configuration",
            json!({"vpcConfiguration": {
                "securityGroupIds": ["sg-00000000000000001", "sg-00000000000000002"],
                "subnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            }}),
            "/vpcConfiguration",
            json!({
                "securityGroupIds": ["sg-00000000000000001", "sg-00000000000000002"],
                "subnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            }),
        ),
        (
            "a removed VPC configuration",
            json!({"removeVpcConfiguration": true}),
            "/vpcConfiguration",
            Value::Null,
        ),
        (
            "a replaced network access configuration",
            json!({"networkAccessControl": {
                "prefixListIds": [],
                "vpceIds": ["vpce-00000000000000001", "vpce-00000000000000002"],
            }}),
            "/networkAccessControl",
            json!({
                "prefixListIds": [],
                "vpceIds": ["vpce-00000000000000001", "vpce-00000000000000002"],
            }),
        ),
        (
            "a removed network access configuration",
            json!({"removeNetworkAccessConfiguration": true}),
            "/networkAccessControl",
            Value::Null,
        ),
    ];

    let router = emulator();
    for (case, patch, pointer, expected) in cases {
        let id = active_workspace(&router, baseline.clone()).await;
        let before = describe(&router, &id).await.workspace().clone();

        let updated = update(&router, &id, patch.clone()).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202 from UpdateWorkspace, got {} with {:?}",
            updated.status, updated.body
        );

        let after = describe(&router, &id).await.workspace().clone();
        assert_eq!(
            at(&after, pointer),
            expected,
            "case {case:?}: {pointer} expected {expected:?}, got {:?}",
            at(&after, pointer)
        );
        assert_eq!(
            at(&after, pointer),
            at(updated.workspace(), pointer),
            "case {case:?}: UpdateWorkspace's own answer must match the stored record"
        );

        // Nothing but the named member, the status and `modified` moved.
        let changed = pointer.trim_start_matches('/');
        let mut was = before.as_object().expect("a described workspace is an object").clone();
        let mut is = after.as_object().expect("a described workspace is an object").clone();
        for key in [changed, "status", "modified"] {
            was.remove(key);
            is.remove(key);
        }
        assert_eq!(
            was, is,
            "case {case:?}: an update naming only {changed:?} changed something else"
        );

        let was_modified = before["modified"].as_f64().unwrap_or_default();
        let is_modified = after["modified"].as_f64().unwrap_or_default();
        assert!(
            is_modified > was_modified,
            "case {case:?}: modified must move forward, got {was_modified} then {is_modified}"
        );
    }

    // Setting and removing the same member in one request cannot be applied either way.
    for (case, patch) in [
        (
            "vpcConfiguration and removeVpcConfiguration",
            json!({
                "vpcConfiguration": {
                    "securityGroupIds": ["sg-1"],
                    "subnetIds": ["subnet-1", "subnet-2"],
                },
                "removeVpcConfiguration": true,
            }),
        ),
        (
            "networkAccessControl and removeNetworkAccessConfiguration",
            json!({
                "networkAccessControl": {"prefixListIds": [], "vpceIds": []},
                "removeNetworkAccessConfiguration": true,
            }),
        ),
    ] {
        let id = active_workspace(&router, baseline.clone()).await;
        let refused = update(&router, &id, patch).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a request that both sets and removes one member must be refused, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // The ORGANIZATION rule CreateWorkspace enforces is judged on the record the update
    // would leave behind, so it holds whichever of the two members the patch carries.
    for (case, patch, status) in [
        (
            "switching to ORGANIZATION with no organizational units stored",
            json!({"accountAccessType": "ORGANIZATION"}),
            400,
        ),
        (
            "emptying the organizational units of an ORGANIZATION workspace",
            json!({"accountAccessType": "ORGANIZATION", "workspaceOrganizationalUnits": []}),
            400,
        ),
        (
            "switching to ORGANIZATION and naming the units in one request",
            json!({
                "accountAccessType": "ORGANIZATION",
                "workspaceOrganizationalUnits": ["ou-1234-abcdefgh"],
            }),
            202,
        ),
    ] {
        let mut unscoped = baseline.clone();
        unscoped
            .as_object_mut()
            .expect("the baseline is a JSON object")
            .remove("workspaceOrganizationalUnits");
        let id = active_workspace(&router, unscoped).await;
        let before = describe(&router, &id).await.workspace().clone();

        let answer = update(&router, &id, patch).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            let after = describe(&router, &id).await.workspace().clone();
            assert_eq!(
                before, after,
                "case {case:?}: a refused update must leave the record untouched"
            );
        }
    }
}

/// The store is bounded: once it holds `WORKSPACE_QUOTA` workspaces a further create is
/// refused with the modelled `ServiceQuotaExceededException` instead of growing the state,
/// and a delete frees room for exactly one more.
///
/// The bound is imported because the test has to know where it lies; what is asserted is
/// the behaviour at it, not the number.
#[tokio::test]
async fn the_workspace_store_is_bounded_by_its_quota() {
    let router = emulator();
    for index in 0..WORKSPACE_QUOTA {
        let answer = create(&router, minimal_create()).await;
        assert_eq!(
            answer.status, 202,
            "create {index} of {WORKSPACE_QUOTA} must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let refused = create(&router, minimal_create()).await;
    assert_eq!(
        refused.status, 402,
        "a create past the quota must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {:?}",
        refused.body
    );

    // Deleting one frees room for exactly one more — once the removal the delete owes the
    // store has fallen due, which is when the workspace stops occupying it.
    let first = listed_ids(&list(&router, "?maxResults=1").await)
        .first()
        .cloned()
        .expect("the store holds workspaces");
    assert_eq!(
        delete(&router, &first).await.status,
        202,
        "the delete that frees quota must be accepted"
    );
    settled().await;
    assert_eq!(
        create(&router, minimal_create()).await.status,
        202,
        "a create must be accepted again once a workspace has been deleted"
    );
    assert_eq!(
        create(&router, minimal_create()).await.status,
        402,
        "and the one after it must be refused again"
    );
}

/// `DeleteWorkspace` accepts the deletion and the removal follows: it answers with the
/// workspace it is deleting, the record stays readable as `DELETING` for the span the
/// clock owes the removal in, the id then names nothing, and the workspaces beside it are
/// untouched throughout.
#[tokio::test]
async fn deleting_a_workspace_removes_it_and_leaves_the_others() {
    let router = emulator();
    let mut ids = Vec::new();
    for index in 0..3 {
        let mut request = minimal_create();
        request["workspaceName"] = json!(format!("delete-{index}"));
        ids.push(create(&router, request).await.id());
    }
    settled().await;

    for (index, id) in ids.clone().into_iter().enumerate() {
        let deleted = delete(&router, &id).await;
        assert_eq!(
            deleted.status, 202,
            "case {index}: expected HTTP 202 from DeleteWorkspace, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.id(),
            id,
            "case {index}: DeleteWorkspace must answer with the workspace it is deleting"
        );
        assert_eq!(
            at(deleted.workspace(), "/status"),
            json!("DELETING"),
            "case {index}: a deleted workspace is reported DELETING, got {:?}",
            at(deleted.workspace(), "/status")
        );

        // While the deletion is in flight the workspace is still there: a poller reads
        // `DELETING`, the `@idempotent` delete is accepted again with the same record,
        // and a change to it is refused as the conflict it is.
        let polled = describe(&router, &id).await;
        assert_eq!(
            at(polled.workspace(), "/status"),
            json!("DELETING"),
            "case {index}: DescribeWorkspace must report DELETING while the deletion is in \
             flight, got {} with {:?}",
            polled.status, polled.body
        );
        let again = delete(&router, &id).await;
        assert_eq!(
            again.status, 202,
            "case {index}: a repeated DeleteWorkspace must be accepted idempotently, got {} \
             with {:?}",
            again.status, again.body
        );
        assert_eq!(
            (again.id(), at(again.workspace(), "/status")),
            (id.clone(), json!("DELETING")),
            "case {index}: the repeated delete must answer the same workspace, still DELETING"
        );
        let refused = update(&router, &id, json!({"workspaceName": "zombie"})).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (409, "ConflictException"),
            "case {index}: UpdateWorkspace on a DELETING workspace must answer 409 \
             ConflictException, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            listed_ids(&list(&router, "").await),
            ids[index..].to_vec(),
            "case {index}: ListWorkspaces must still carry the workspace being deleted"
        );

        // The removal is owed by the clock: once it falls due the id names nothing.
        settled().await;
        for (operation, answer) in [
            ("DescribeWorkspace", describe(&router, &id).await),
            ("DeleteWorkspace", delete(&router, &id).await),
            ("UpdateWorkspace", update(&router, &id, json!({"workspaceName": "zombie"})).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {index}: {operation} on a removed workspace must answer 404, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {index}: {operation} expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceId"].as_str().unwrap_or_default(),
                id,
                "case {index}: {operation} must name the id it could not find"
            );
        }

        let remaining = listed_ids(&list(&router, "").await);
        assert_eq!(
            remaining,
            ids[index + 1..].to_vec(),
            "case {index}: ListWorkspaces must report exactly the workspaces still stored"
        );
    }
}

/// A workspace id the model's `^g-[0-9a-f]{10}$` admits but no workspace has is a 404; one
/// the pattern does not admit is a 400.
#[tokio::test]
async fn an_id_that_names_no_workspace_is_refused_by_kind() {
    let cases = [
        ("a well-formed id nothing has", "g-0123456789", 404, "ResourceNotFoundException"),
        ("another well-formed id nothing has", "g-ffffffffff", 404, "ResourceNotFoundException"),
        ("not a workspace id at all", "not-a-workspace", 400, "ValidationException"),
        ("hex digits out of range", "g-ZZZZZZZZZZ", 400, "ValidationException"),
        ("too few hex digits", "g-0123", 400, "ValidationException"),
        ("too many hex digits", "g-0123456789a", 400, "ValidationException"),
    ];

    let router = emulator();
    // One real workspace, so a refusal cannot be an artefact of an empty store.
    let stored = create(&router, minimal_create()).await.id();

    for (case, id, status, code) in cases {
        let answer = describe(&router, id).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for {id:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code} for {id:?}, got {:?}",
            answer.body
        );
    }

    let found = describe(&router, &stored).await;
    assert_eq!(
        found.status, 200,
        "the stored workspace must still describe, got {} with {:?}",
        found.status, found.body
    );
}

/// A created workspace is not usable the instant the create returns: it reports `CREATING`
/// to a caller acting straight away, and the clock owes it `ACTIVE` without any further
/// call.
#[tokio::test]
async fn a_new_workspace_reports_creating_until_the_clock_owes_it_active() {
    let router = emulator();
    let started = Instant::now();
    let created = create(&router, minimal_create()).await;
    let id = created.id();
    assert_eq!(
        at(created.workspace(), "/status"),
        json!("CREATING"),
        "CreateWorkspace must answer with the transient status, got {:?}",
        at(created.workspace(), "/status")
    );

    let immediately = describe(&router, &id).await;
    if started.elapsed() < LIFECYCLE_TRANSITION {
        assert_eq!(
            at(immediately.workspace(), "/status"),
            json!("CREATING"),
            "a caller acting inside the transition span still sees CREATING, got {:?}",
            at(immediately.workspace(), "/status")
        );
    }

    // The transition is owed by the clock, not by a call: nothing is done here but wait.
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200));

    let settled = describe(&router, &id).await;
    assert_eq!(
        at(settled.workspace(), "/status"),
        json!("ACTIVE"),
        "once the transition span has passed the workspace is ACTIVE, got {:?}",
        at(settled.workspace(), "/status")
    );
    let listed = list(&router, "").await;
    assert_eq!(
        at(&listed.body, "/workspaces/0/status"),
        json!("ACTIVE"),
        "ListWorkspaces reports the same settled status, got {:?}",
        listed.body
    );
}

/// `accountAccessType: ORGANIZATION` documents that the organizational units must be named
/// with it; a create that leaves them out is refused rather than stored.
#[tokio::test]
async fn organization_access_without_organizational_units_is_refused() {
    let cases = [
        ("the member is absent", json!(null)),
        ("the member is an empty list", json!([])),
    ];

    let router = emulator();
    for (case, units) in cases {
        let mut request = minimal_create();
        request["accountAccessType"] = json!("ORGANIZATION");
        if !units.is_null() {
            request["workspaceOrganizationalUnits"] = units;
        }

        let refused = create(&router, request).await;
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
    }

    assert!(
        listed_ids(&list(&router, "").await).is_empty(),
        "a refused create must store nothing"
    );
}
