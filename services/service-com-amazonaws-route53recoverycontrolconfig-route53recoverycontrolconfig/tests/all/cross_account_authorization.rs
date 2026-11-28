// SPDX-License-Identifier: CC0-1.0

//! Who may act on the resource an ARN names.
//!
//! The rule under test: every operation that takes a caller-supplied ARN for one of this
//! service's resources decides authorization from the REQUEST alone, before it resolves
//! the ARN — so an ARN whose account id is not the caller's is refused with
//! `AccessDeniedException` (403), naming the principal that signed, the IAM action it
//! asked for and the ARN it asked for, and the answer never says whether a resource is
//! stored there. An ARN of the caller's own account is left to that lookup, which is what
//! answers `ResourceNotFoundException` (404) when it holds nothing
//! (`tests/all/not_found_message.rs`).
//!
//! The tables span what that rule generalises over: every implemented operation that
//! carries such an ARN — read, delete, update, list and the two creates that reference a
//! parent — all three resource levels, both ARN members of the create that carries two,
//! an ARN bound to the URI and one carried in the body or the query string, and three
//! foreign accounts (including the account swapped on an ARN this service really minted,
//! so the refusal is demonstrably not about the resource being absent).
//!
//! The last rule here is the stage AHEAD of all of that: who the CALLER is. This API is
//! reached through a REST front door that verifies the request's credentials before
//! anything reads what the call says, so a caller the emulator's authentication layer
//! cannot verify is refused by that layer — HTTP 401, `x-amzn-errortype:
//! UnauthorizedException`, body exactly `{"message":"Unauthorized"}` — whatever operation
//! the request names and whatever its payload carries, and never with one of this
//! service's modelled errors (whose sentences travel at the model's own statuses: 400
//! `ValidationException`, 403 `AccessDeniedException`, 404 `ResourceNotFoundException`).
//! That case drives a router with request authentication ON, the `--auth on` path the
//! production front door runs; every other suite in this crate drives the same routes over
//! the open router, which is what shows the 401 is the verification layer's verdict rather
//! than an operation's.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, create_cluster, enc, error_code, router, REGION, SIGNING_NAME};

/// The account the emulator's own credential belongs to: the caller of every request in
/// this suite.
const CALLER_ACCOUNT: &str = "000000000000";

/// The IAM user that credential speaks for, as AWS names a principal in an authorization
/// refusal: `arn:aws:iam::<account>:user/<name>`.
const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The namespace this service's IAM actions are published under: the model's
/// `arnNamespace` (`smithy_query(query="service_info")`).
const ACTION_NAMESPACE: &str = "route53-recovery-control-config";

/// Accounts that are not the caller's, so an ARN built for one of them is another
/// account's resource.
const FOREIGN_ACCOUNTS: &[&str] = &["123456789012", "999999999999", "210987654321"];

/// The sentence AWS refuses an unauthorized request with, restated here independently of
/// the service's own code: the principal, the fully qualified action and the resource, all
/// as the request carried them.
fn not_authorized(action: &str, resource: &str) -> String {
    format!(
        "User: {CALLER_PRINCIPAL} is not authorized to perform: {ACTION_NAMESPACE}:{action} \
         on resource: {resource} because no resource-based policy allows the \
         {ACTION_NAMESPACE}:{action} action"
    )
}

/// A recovery-control ARN: AWS writes them with an empty Region segment.
fn arn(account: &str, tail: &str) -> String {
    format!("arn:aws:route53-recovery-control::{account}:{tail}")
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="AccessDeniedException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One request to make, with the action and resource its refusal must name.
struct Case {
    label: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    action: &'static str,
    resource: String,
}

/// Every implemented request that carries a cluster ARN, for the cluster ARN `cluster_arn`.
///
/// `own_cluster_arn` is a cluster of the CALLER's account, used by the row that puts a
/// foreign ARN in the create's OTHER ARN member.
fn cases_for(cluster_arn: &str, own_cluster_arn: &str) -> Vec<Case> {
    let panel_arn = cluster_arn.replace("cluster/", "controlpanel/");
    let control_arn = format!("{panel_arn}/routingcontrol/control");
    vec![
        Case {
            label: format!("DescribeCluster({cluster_arn})"),
            method: "GET",
            path: format!("/cluster/{}", enc(cluster_arn)),
            body: None,
            action: "DescribeCluster",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("DeleteCluster({cluster_arn})"),
            method: "DELETE",
            path: format!("/cluster/{}", enc(cluster_arn)),
            body: None,
            action: "DeleteCluster",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("UpdateCluster({cluster_arn})"),
            method: "PUT",
            path: "/cluster".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "NetworkType": "DUALSTACK" })),
            action: "UpdateCluster",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("ListControlPanels(ClusterArn={cluster_arn})"),
            method: "GET",
            path: format!("/controlpanels?ClusterArn={}", enc(cluster_arn)),
            body: None,
            action: "ListControlPanels",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("CreateControlPanel(ClusterArn={cluster_arn})"),
            method: "POST",
            path: "/controlpanel".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "ControlPanelName": "panel" })),
            action: "CreateControlPanel",
            resource: cluster_arn.to_string(),
        },
        Case {
            label: format!("CreateRoutingControl(ClusterArn={cluster_arn})"),
            method: "POST",
            path: "/routingcontrol".to_string(),
            body: Some(json!({ "ClusterArn": cluster_arn, "RoutingControlName": "control" })),
            action: "CreateRoutingControl",
            resource: cluster_arn.to_string(),
        },
        Case {
            // The create's OTHER ARN member, with a cluster the caller does own: every
            // ARN the request carries is authorized, not just the first.
            label: format!("CreateRoutingControl(ControlPanelArn={panel_arn})"),
            method: "POST",
            path: "/routingcontrol".to_string(),
            body: Some(json!({
                "ClusterArn": own_cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": "control"
            })),
            action: "CreateRoutingControl",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("DescribeControlPanel({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}", enc(&panel_arn)),
            body: None,
            action: "DescribeControlPanel",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("DeleteControlPanel({panel_arn})"),
            method: "DELETE",
            path: format!("/controlpanel/{}", enc(&panel_arn)),
            body: None,
            action: "DeleteControlPanel",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("ListRoutingControls({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}/routingcontrols", enc(&panel_arn)),
            body: None,
            action: "ListRoutingControls",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("ListSafetyRules({panel_arn})"),
            method: "GET",
            path: format!("/controlpanel/{}/safetyrules", enc(&panel_arn)),
            body: None,
            action: "ListSafetyRules",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("UpdateControlPanel({panel_arn})"),
            method: "PUT",
            path: "/controlpanel".to_string(),
            body: Some(json!({ "ControlPanelArn": panel_arn, "ControlPanelName": "renamed" })),
            action: "UpdateControlPanel",
            resource: panel_arn.clone(),
        },
        Case {
            label: format!("DescribeRoutingControl({control_arn})"),
            method: "GET",
            path: format!("/routingcontrol/{}", enc(&control_arn)),
            body: None,
            action: "DescribeRoutingControl",
            resource: control_arn.clone(),
        },
        Case {
            label: format!("DeleteRoutingControl({control_arn})"),
            method: "DELETE",
            path: format!("/routingcontrol/{}", enc(&control_arn)),
            body: None,
            action: "DeleteRoutingControl",
            resource: control_arn.clone(),
        },
        Case {
            label: format!("UpdateRoutingControl({control_arn})"),
            method: "PUT",
            path: "/routingcontrol".to_string(),
            body: Some(json!({
                "RoutingControlArn": control_arn,
                "RoutingControlName": "renamed"
            })),
            action: "UpdateRoutingControl",
            resource: control_arn,
        },
    ]
}

/// RULE: an ARN of another account is refused with `AccessDeniedException` (403) naming
/// the principal, the action and the ARN — on every operation that carries one.
#[tokio::test]
async fn an_arn_of_another_account_is_refused_with_access_denied() {
    let router = router();

    // A cluster of the caller's own account exists throughout: a 403 below is about the
    // ARN the request names and not about an empty store, and the rows that need a
    // second, valid ARN use this one.
    let own = create_cluster(&router, json!({ "ClusterName": "owned" })).await;
    let own_arn = own["ClusterArn"].as_str().unwrap_or_default().to_string();

    let mut cases: Vec<Case> = Vec::new();
    for account in FOREIGN_ACCOUNTS {
        cases.extend(cases_for(
            &arn(account, "cluster/7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4"),
            &own_arn,
        ));
    }
    // The ARN this service really minted, with only its account swapped: the resource
    // behind it exists, and the answer is the same refusal, so the refusal cannot be
    // reporting absence.
    cases.extend(cases_for(
        &own_arn.replace(CALLER_ACCOUNT, "123456789012"),
        &own_arn,
    ));

    for case in &cases {
        let (status, answer) = call(&router, case.method, &case.path, case.body.clone()).await;
        assert_eq!(
            status, 403,
            "case {}: expected 403, got {status} {answer:?}",
            case.label
        );
        assert_eq!(
            error_code(&answer),
            "AccessDeniedException",
            "case {}: expected AccessDeniedException, got {answer:?}",
            case.label
        );
        let actual = message(&answer);
        let expected = not_authorized(case.action, &case.resource);
        assert_eq!(
            actual, expected,
            "case {}: expected {expected:?}, got {actual:?}",
            case.label
        );
    }

    // And none of them created anything: the refusal happens before any handler writes.
    let (status, listed) = call(&router, "GET", "/cluster?MaxResults=1000", None).await;
    assert_eq!(
        status, 200,
        "case list clusters: expected 200, got {status} {listed:?}"
    );
    let arns: Vec<&str> = listed["Clusters"]
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .map(|cluster| cluster["ClusterArn"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![own_arn.as_str()],
        "case after {} refused requests: expected only the cluster created up front, got \
         {arns:?}",
        cases.len()
    );
}

/// RULE, at its boundary: the check refuses an ARN that names ANOTHER account and nothing
/// else, so a well-formed ARN with no account to read — and one of the caller's own
/// account — reaches the lookup, which answers for it.
#[tokio::test]
async fn an_arn_naming_no_other_account_reaches_the_lookup() {
    let router = router();

    let own = create_cluster(&router, json!({ "ClusterName": "boundary" })).await;
    let own_arn = own["ClusterArn"].as_str().unwrap_or_default().to_string();

    // (case, the ClusterArn to ask for, the status expected, the code expected)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "the caller's own stored cluster",
            own_arn.clone(),
            200,
            "",
        ),
        (
            "an unstored ARN of the caller's own account",
            arn(CALLER_ACCOUNT, "cluster/00000000-0000-0000-0000-000000000000"),
            404,
            "ResourceNotFoundException",
        ),
        (
            // Well formed, with nothing in the account field to compare: authorization
            // refuses nothing and the lookup answers. (A value that is not a well-formed
            // ARN never reaches either stage: it is malformed input, 400
            // `ValidationException`, pinned by
            // `every_operation_taking_an_arn_refuses_a_malformed_one_as_input` in
            // `tests/all/rejection_error_classes.rs`.)
            "an ARN whose account segment is empty",
            arn("", "cluster/00000000-0000-0000-0000-000000000000"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, cluster_arn, expected_status, expected_code) in &rows {
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/cluster/{}", enc(cluster_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected {expected_status}, got {status} {answer:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&answer),
                *expected_code,
                "case {case}: expected {expected_code}, got {answer:?}"
            );
        }
    }
}

/// The status the front door refuses a caller it could not verify with, the wire error code
/// that refusal names and the whole sentence it states — none of them a shape this
/// service's model declares (`smithy_query(query="shape",shape="UnauthorizedException")`
/// reports no such shape), because the layer that answers is not the service.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");

/// A fixed signing date, as SigV4 writes it, and its credential-scope date.
const AMZ_DATE: &str = "20260101T000000Z";
const SCOPE_DATE: &str = "20260101";

/// How one caller presents its credentials: the `Authorization` header when it carries
/// one, the signing date when it sends one, and the query string a presigned caller
/// carries its scope in.
struct Presentation {
    what: &'static str,
    authorization: Option<String>,
    amz_date: Option<&'static str>,
    query: String,
}

/// A header-signed presentation carrying `scope` under `algorithm`, with no valid
/// signature: the verifier reads the algorithm and the scope before it compares one.
fn header_signed(
    what: &'static str,
    algorithm: &str,
    scope: &str,
    amz_date: Option<&'static str>,
) -> Presentation {
    Presentation {
        what,
        authorization: Some(format!(
            "{algorithm} Credential={scope}, SignedHeaders=host;x-amz-date, \
             Signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )),
        amz_date,
        query: String::new(),
    }
}

/// Every way a caller can fail verification at this endpoint: nothing presented at all, a
/// key this emulator never minted, a header-signed request with no signing instant, a
/// scope that does not end in the SigV4 terminator, an algorithm the authenticator cannot
/// read, and the presigned carrier of the same unknown key.
fn presentations() -> Vec<Presentation> {
    let scope = format!("AKIDEXAMPLE/{SCOPE_DATE}/{REGION}/{SIGNING_NAME}/aws4_request");
    vec![
        Presentation {
            what: "unsigned",
            authorization: None,
            amz_date: None,
            query: String::new(),
        },
        header_signed(
            "an access key the emulator never minted",
            "AWS4-HMAC-SHA256",
            &scope,
            Some(AMZ_DATE),
        ),
        header_signed(
            "header-signed with no x-amz-date",
            "AWS4-HMAC-SHA256",
            &scope,
            None,
        ),
        header_signed(
            "a credential scope whose terminator is not aws4_request",
            "AWS4-HMAC-SHA256",
            &format!("AKIDEXAMPLE/{SCOPE_DATE}/{REGION}/{SIGNING_NAME}/aws5_request"),
            Some(AMZ_DATE),
        ),
        header_signed(
            "an AWS4-ECDSA-P256-SHA256 Authorization the authenticator cannot read",
            "AWS4-ECDSA-P256-SHA256",
            &scope,
            Some(AMZ_DATE),
        ),
        Presentation {
            what: "presigned with an access key the emulator never minted",
            authorization: None,
            amz_date: None,
            query: format!(
                "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={scope}\
                 &X-Amz-Date={AMZ_DATE}&X-Amz-Expires=300&X-Amz-SignedHeaders=host\
                 &X-Amz-Signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                scope = scope.replace('/', "%2F"),
            ),
        },
    ]
}

/// One request this suite sends over the authenticating router, named by the operation it
/// would have reached.
struct Unverified {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// A request for each shape of operation this service serves — a read, a create, an
/// update, a delete, a list and a tagging call — so the rows show the refusal landing
/// ahead of every one of them, addressed by an ARN that REALLY EXISTS as well as by one
/// that does not, because the verdict depends on neither.
fn unverified_calls(stored_cluster: &str) -> Vec<Unverified> {
    let unstored = arn(CALLER_ACCOUNT, "cluster/00000000-0000-0000-0000-000000000000");
    vec![
        Unverified {
            operation: "DescribeCluster (a stored cluster)",
            method: "GET",
            path: format!("/cluster/{}", enc(stored_cluster)),
            body: None,
        },
        Unverified {
            operation: "DescribeCluster (an unstored ARN)",
            method: "GET",
            path: format!("/cluster/{}", enc(&unstored)),
            body: None,
        },
        Unverified {
            operation: "CreateCluster",
            method: "POST",
            path: "/cluster".to_string(),
            body: Some(json!({ "ClusterName": "unverified-create" })),
        },
        Unverified {
            operation: "UpdateCluster",
            method: "PUT",
            path: "/cluster".to_string(),
            body: Some(json!({ "ClusterArn": stored_cluster, "NetworkType": "IPV4" })),
        },
        Unverified {
            operation: "DeleteCluster",
            method: "DELETE",
            path: format!("/cluster/{}", enc(stored_cluster)),
            body: None,
        },
        Unverified {
            operation: "ListClusters",
            method: "GET",
            path: "/cluster?MaxResults=1000".to_string(),
            body: None,
        },
        Unverified {
            operation: "TagResource",
            method: "POST",
            path: format!("/tags/{}", enc(stored_cluster)),
            body: Some(json!({ "Tags": { "suite": "unverified" } })),
        },
        Unverified {
            operation: "CreateSafetyRule (a body the operation would refuse)",
            method: "POST",
            path: "/safetyrule".to_string(),
            // Both rule members, so this row turns on the CALLER and nothing else: such a
            // body names the resource the front door authorizes a safety-rule write
            // against, and a verified caller sending it is refused by the OPERATION
            // (`tests/all/create_safety_rule_rule_choice.rs`).
            body: Some(json!({
                "AssertionRule": {
                    "AssertedControls": [format!("{stored_cluster}/routingcontrol/control")],
                    "ControlPanelArn": stored_cluster.replace("cluster/", "controlpanel/"),
                    "Name": "unverified-assertion",
                    "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "ATLEAST" },
                    "WaitPeriodMs": 5000
                },
                "GatingRule": {
                    "ControlPanelArn": stored_cluster.replace("cluster/", "controlpanel/"),
                    "GatingControls": [format!("{stored_cluster}/routingcontrol/control")],
                    "Name": "unverified-gating",
                    "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "AND" },
                    "TargetControls": [format!("{stored_cluster}/routingcontrol/control")],
                    "WaitPeriodMs": 5000
                },
                "ClientToken": "unverified-rule"
            })),
        },
    ]
}

/// Send one request over `router` as `presentation` says, and answer
/// `(status, error code, message, parsed body)`.
async fn unverified(
    router: &MultiServiceRouter,
    presentation: &Presentation,
    request: &Unverified,
) -> (u16, String, String, Value) {
    let mut builder = http::Request::builder()
        .method(request.method)
        .uri(format!(
            "https://{SIGNING_NAME}.{REGION}.amazonaws.com{path}{query}",
            path = request.path,
            query = presentation.query
        ))
        .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json");
    if let Some(authorization) = &presentation.authorization {
        builder = builder.header("authorization", authorization.clone());
    }
    if let Some(date) = presentation.amz_date {
        builder = builder.header("x-amz-date", date);
    }
    let payload = request.body.as_ref().map_or_else(bytes::Bytes::new, |body| {
        bytes::Bytes::from(serde_json::to_vec(body).expect("the request body serializes"))
    });
    let response = router
        .dispatch(builder.body(payload).expect("a well-formed test request"))
        .await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    let message = parsed["message"].as_str().unwrap_or_default().to_string();
    (status, error_type, message, parsed)
}

/// RULE: the stage ahead of every operation is who the CALLER is — a caller the
/// authentication layer cannot verify is refused by the front door (HTTP 401,
/// `x-amzn-errortype: UnauthorizedException`, body exactly `{"message":"Unauthorized"}`)
/// whatever operation it names and whatever its payload carries, and that refusal reaches
/// no handler: nothing it would have written is applied.
///
/// The rows span both dimensions the rule generalises over: WHY the caller cannot be
/// verified (six presentations, including the presigned carrier) and WHICH operation the
/// request names (a read of a stored resource and of an unstored one, a create, an update,
/// a delete, a list, a tagging call, and a body the operation itself would refuse).
#[tokio::test]
async fn a_caller_the_authentication_layer_cannot_verify_is_refused_by_the_front_door() {
    // One engine behind TWO routers: the authenticating one every row drives, and the open
    // one the rest of this crate's suites drive — which is how the fixture is seeded and
    // how a refused mutation is shown to have changed nothing.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut authenticating = MultiServiceRouter::new();
    authenticating.set_request_authentication(true);
    authenticating.register_service(EmulatedService::new(Arc::clone(&engine)));
    let mut open = MultiServiceRouter::new();
    open.set_request_authentication(false);
    open.register_service(EmulatedService::new(Arc::clone(&engine)));

    let stored = create_cluster(&open, json!({ "ClusterName": "front-door" })).await;
    let stored_arn = stored["ClusterArn"].as_str().unwrap_or_default().to_string();
    let (expected_status, expected_code, expected_message) = UNAUTHORIZED;

    let mut answers = Vec::new();
    for presentation in presentations() {
        for request in &unverified_calls(&stored_arn) {
            let case = format!("{} -> {}", presentation.what, request.operation);
            let (status, code, message, body) =
                unverified(&authenticating, &presentation, request).await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (expected_status, expected_code, expected_message),
                "case {case}: expected {expected_status} {expected_code} \
                 {expected_message:?}, got {status} {code:?} {body:?}"
            );
            // Not one of this service's modelled errors, whose sentences travel under the
            // same member but at the model's own statuses, and never with `__type`.
            assert_eq!(
                body,
                json!({ "message": expected_message }),
                "case {case}: the refusal states the sentence and nothing else, got {body:?}"
            );
            answers.push((case, body));
        }
    }

    // No refused request reached a handler: the seeded cluster is still readable through
    // the open router, the refused create stored nothing, and the refused tagging call
    // left it untagged.
    let (status, read) = call(&open, "GET", &format!("/cluster/{}", enc(&stored_arn)), None).await;
    assert_eq!(
        status, 200,
        "case the seeded cluster after the refusals: expected 200, got {status} {read:?}"
    );
    let (status, listed) = call(&open, "GET", "/cluster?MaxResults=1000", None).await;
    assert_eq!(
        status, 200,
        "case list the clusters: expected 200, got {status} {listed:?}"
    );
    let clusters = listed["Clusters"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        clusters.len(),
        1,
        "case after {} refused requests: expected only the seeded cluster, got {clusters:?}",
        answers.len()
    );
    let (status, tags) = call(&open, "GET", &format!("/tags/{}", enc(&stored_arn)), None).await;
    assert_eq!(
        status, 200,
        "case the seeded cluster's tags: expected 200, got {status} {tags:?}"
    );
    assert_eq!(
        tags["Tags"],
        json!({}),
        "case after the refused tagging calls: expected no tag, got {:?}",
        tags["Tags"]
    );
}
