// SPDX-License-Identifier: CC0-1.0

//! `CreateGatewayRule`, and the read of the rule it wrote, end to end over HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="closure",operation="CreateGatewayRule,GetGatewayRule")`): the
//! create ADDS one rule to the gateway the path names, under a `ruleId` the service mints
//! (`GatewayRuleId` is the 36-character hyphenated UUID form, and no request member carries
//! one), stores exactly the priority, conditions, actions and description submitted, and
//! reports it `CREATING` until the clock owes it `ACTIVE` — so the rule reads back through
//! `GetGatewayRule` exactly as the create reported it. `system` is "System-managed metadata
//! for rules created by automated processes", and no request member carries it, so a rule a
//! caller created reports none. A request the gateway cannot take as a rule is refused the
//! modelled error and writes nothing: an identifier no gateway holds is the modelled 404,
//! and a gateway whose deletion has been accepted takes no further configuration.
//!
//! The rows differ along what the rule generalises over: the priority (both published bounds
//! of `GatewayRulePriority`, `@range 1..1000000`, and values between them), how many
//! conditions the rule carries and of which kind, how many principals one condition matches
//! and under which operator, how many actions it takes and of which kind, and whether it
//! carries a description (including one at the published upper bound of
//! `GatewayRuleDescription`, `@length 1..256`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// A `clientToken` inside `ClientToken`'s 33..256 length and its pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
const CLIENT_TOKEN: &str = "creategatewayruletokenaaaaaaaaaaaa";

/// A configuration-bundle ARN inside `GatewayConfigurationBundleArn`'s pattern
/// `^arn:aws[a-zA-Z-]*:bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:configuration-bundle/\
/// [a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const BUNDLE_ARN: &str = "arn:aws:bedrock-agentcore:us-east-1:000000000000:\
                          configuration-bundle/checkout-abcdefghij";

/// A bundle version inside `ConfigurationBundleReference.bundleVersion`'s pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
const BUNDLE_VERSION: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// How long a poll loop keeps looking for a clock-owed transition before giving up.
/// Generous on purpose: the loop waits for the CONDITION, and the span this service gives
/// the gateway family is far shorter than this, on any host.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(20);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn create_rule(router: &MultiServiceRouter, gateway: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/gateways/{gateway}/rules"),
        Some(body),
    )
    .await
}

async fn get_rule(router: &MultiServiceRouter, gateway: &str, rule: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/gateways/{gateway}/rules/{rule}"),
        None,
    )
    .await
}

/// Create a gateway and hand back its `gatewayId`.
async fn gateway(router: &MultiServiceRouter, name: &str) -> String {
    let created = send(
        router,
        "POST",
        "/gateways/",
        Some(&json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup {name:?}: expected HTTP 202 from CreateGateway, got {} ({})",
        created.status, created.body
    );
    text(&created.body, "gatewayId")
}

/// Read the rule again until it reports `wanted`, or the deadline passes — the transition
/// is the clock's, so the loop looks for the CONDITION rather than waiting a fixed time.
async fn rule_reporting(
    router: &MultiServiceRouter,
    gateway_id: &str,
    rule_id: &str,
    wanted: &str,
) -> Answer {
    let started = Instant::now();
    loop {
        let answer = get_rule(router, gateway_id, rule_id).await;
        if text(&answer.body, "status") == wanted || started.elapsed() > POLL_DEADLINE {
            return answer;
        }
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Read the gateway again until it reports `READY`, or the deadline passes. `DeleteGateway`
/// is refused while the clock still owes the gateway its transition, so a row that needs a
/// deleted gateway waits for that condition first.
async fn gateway_ready(router: &MultiServiceRouter, gateway_id: &str) {
    let started = Instant::now();
    loop {
        let answer = send(router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
        if text(&answer.body, "status") == "READY" || started.elapsed() > POLL_DEADLINE {
            return;
        }
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// One principal condition over the IAM ARNs `entries` names.
fn match_principals(entries: Vec<Value>) -> Value {
    json!({"matchPrincipals": {"anyOf": entries}})
}

/// One `iamPrincipal` entry, with the optional `operator` only when `operator` is given.
/// `PrincipalMatchOperator`'s full value list is `StringEquals | StringLike`
/// (`smithy_query(query="shape",shape="PrincipalMatchOperator")`).
fn iam_principal(arn: &str, operator: Option<&str>) -> Value {
    let mut principal = json!({"arn": arn});
    if let Some(operator) = operator {
        principal["operator"] = json!(operator);
    }
    json!({"iamPrincipal": principal})
}

/// A static route to one target by name.
fn static_route(target_name: &str) -> Value {
    json!({"routeToTarget": {"staticRoute": {"targetName": target_name}}})
}

/// A bundle reference, as both override shapes carry it.
fn bundle_reference() -> Value {
    json!({"bundleArn": BUNDLE_ARN, "bundleVersion": BUNDLE_VERSION})
}

/// One submitted rule.
struct Submitted {
    priority: i64,
    /// The conditions the request carries, or `None` to leave the member out.
    conditions: Option<Vec<Value>>,
    actions: Vec<Value>,
    description: Option<String>,
}

impl Submitted {
    fn body(&self) -> Value {
        let mut request = json!({"priority": self.priority, "actions": self.actions});
        if let Some(conditions) = &self.conditions {
            request["conditions"] = json!(conditions);
        }
        if let Some(description) = &self.description {
            request["description"] = json!(description);
        }
        request
    }
}

/// One row of the create table.
struct Row {
    case: &'static str,
    /// The gateway name the row creates (`GatewayName` is `^([0-9a-zA-Z][-]?){1,48}$`).
    gateway: &'static str,
    submitted: Submitted,
}

/// A description exactly at `GatewayRuleDescription`'s published upper bound of 256.
fn description_at_upper_bound() -> String {
    "d".repeat(256)
}

/// The rows. Every condition and action value below is built from the union members the
/// model declares — `Condition` is `matchPrincipals | matchPaths`, `Action` is
/// `routeToTarget | configurationBundle`, `RouteToTargetAction` is
/// `staticRoute | weightedRoute` and `ConfigurationBundleAction` is
/// `staticOverride | weightedOverride`
/// (`smithy_query(query="closure",operation="CreateGatewayRule")`) — so every one of the
/// six shapes a rule can be built from is covered, including those no recorded request used.
/// The priorities include both published bounds of `GatewayRulePriority` (`@range
/// 1..1000000`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one principal condition, a static route, the lowest published priority",
            gateway: "createrulelowestpriority",
            submitted: Submitted {
                priority: 1,
                conditions: Some(vec![match_principals(vec![iam_principal(
                    "arn:aws:iam::000000000000:role/Caller",
                    None,
                )])]),
                actions: vec![static_route("orders")],
                description: Some("route the caller to the orders target".to_string()),
            },
        },
        Row {
            case: "no conditions member, a weighted route, the highest published priority",
            gateway: "createruleweightedroute",
            submitted: Submitted {
                priority: 1_000_000,
                conditions: None,
                actions: vec![json!({"routeToTarget": {"weightedRoute": {"trafficSplit": [
                    {"name": "blue", "weight": 90, "targetName": "orders"},
                    {"name": "green", "weight": 10, "targetName": "ordersNext"},
                ]}}})],
                description: None,
            },
        },
        Row {
            case: "an empty conditions list and a static configuration-bundle override",
            gateway: "createrulestaticoverride",
            submitted: Submitted {
                priority: 500,
                conditions: Some(Vec::new()),
                actions: vec![json!({"configurationBundle": {
                    "staticOverride": bundle_reference(),
                }})],
                description: Some("pin the bundle version".to_string()),
            },
        },
        Row {
            case: "two conditions, two principals under both operators, and two actions",
            gateway: "createruletwoconditions",
            submitted: Submitted {
                priority: 42,
                conditions: Some(vec![
                    match_principals(vec![
                        iam_principal(
                            "arn:aws:iam::000000000000:user/Analyst",
                            Some("StringEquals"),
                        ),
                        iam_principal(
                            "arn:aws-us-gov:sts::*:assumed-role/Deployer/*",
                            Some("StringLike"),
                        ),
                    ]),
                    json!({"matchPaths": {"anyOf": ["/orders/*", "/reports.v2/*"]}}),
                ]),
                actions: vec![
                    static_route("orders"),
                    json!({"configurationBundle": {"weightedOverride": {"trafficSplit": [
                        {
                            "name": "baseline",
                            "weight": 60,
                            "configurationBundle": bundle_reference(),
                            "description": "the bundle in production",
                            "metadata": {"owner": "platform"},
                        },
                        {
                            "name": "candidate",
                            "weight": 40,
                            "configurationBundle": bundle_reference(),
                        },
                    ]}}}),
                ],
                description: Some("principal and path scoped".to_string()),
            },
        },
        Row {
            case: "one path condition, a description at the published upper bound",
            gateway: "createrulelongdescription",
            submitted: Submitted {
                priority: 999_999,
                conditions: Some(vec![json!({"matchPaths": {"anyOf": ["/a/*"]}})]),
                actions: vec![json!({"configurationBundle": {
                    "staticOverride": bundle_reference(),
                }})],
                description: Some(description_at_upper_bound()),
            },
        },
    ]
}

/// A `ruleId` is well formed: `GatewayRuleId` is 36 characters matching
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
fn well_formed_rule_id(rule_id: &str) -> bool {
    let groups: Vec<&str> = rule_id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| group.len() == *width)
        && rule_id
            .chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit())
        && rule_id
            .chars()
            .all(|character| !character.is_ascii_uppercase())
}

/// A create stores exactly the submitted rule under a well-formed minted id, reports it
/// `CREATING`, and it reads back unchanged — settling to `ACTIVE` only once the span the
/// clock owes passes.
#[tokio::test]
async fn a_create_stores_exactly_the_submitted_rule() {
    for Row {
        case,
        gateway: gateway_name,
        submitted,
    } in rows()
    {
        let router = router();
        let gateway_id = gateway(&router, gateway_name).await;

        let created = create_rule(&router, &gateway_id, &submitted.body()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateGatewayRule, got {} ({})",
            created.status, created.body
        );
        let rule_id = text(&created.body, "ruleId");
        assert!(
            well_formed_rule_id(&rule_id),
            "case {case:?}: the service mints a well-formed ruleId, got {rule_id:?}"
        );
        assert_eq!(
            text(&created.body, "gatewayArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:000000000000:gateway/{gateway_id}"),
            "case {case:?}: the rule names the gateway it belongs to, got {}",
            created.body
        );
        assert_eq!(
            text(&created.body, "status"),
            "CREATING",
            "case {case:?}: a new rule is CREATING until the clock owes it ACTIVE, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("priority"),
            Some(&json!(submitted.priority)),
            "case {case:?}: the rule reports the priority submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("actions"),
            Some(&json!(submitted.actions)),
            "case {case:?}: the rule reports the actions submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("conditions").cloned(),
            submitted.conditions.as_ref().map(|conditions| json!(conditions)),
            "case {case:?}: the rule reports the conditions submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("description").cloned(),
            submitted
                .description
                .as_ref()
                .map(|description| json!(description)),
            "case {case:?}: the rule reports the description submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("system"),
            None,
            "case {case:?}: a rule a caller created is managed by no system, got {}",
            created.body
        );

        // The round trip: the stored rule reads back through the single-rule read.
        let read = get_rule(&router, &gateway_id, &rule_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created rule must read back, got {} ({})",
            read.status, read.body
        );
        for member in [
            "ruleId",
            "gatewayArn",
            "priority",
            "conditions",
            "actions",
            "description",
            "createdAt",
            "system",
        ] {
            assert_eq!(
                read.body.get(member),
                created.body.get(member),
                "case {case:?}: the read reports the {member} the create reported, got {}",
                read.body
            );
        }
        assert_eq!(
            read.body.get("updatedAt"),
            read.body.get("createdAt"),
            "case {case:?}: nothing has updated the rule since it was created, got {}",
            read.body
        );

        // Nothing but the clock moves the rule to its terminal status.
        let settled = rule_reporting(&router, &gateway_id, &rule_id, "ACTIVE").await;
        assert_eq!(
            (settled.status, text(&settled.body, "status").as_str()),
            (200, "ACTIVE"),
            "case {case:?}: once the span passes the rule is ACTIVE, got {} {} ({})",
            settled.status,
            text(&settled.body, "status"),
            settled.body
        );
    }
}

/// A create ADDS to the gateway's rules: a second rule leaves the first exactly as it was,
/// under its own id.
#[tokio::test]
async fn a_second_create_adds_beside_the_first() {
    let router = router();
    let gateway_id = gateway(&router, "createrulesecond").await;

    let first = Submitted {
        priority: 10,
        conditions: Some(vec![json!({"matchPaths": {"anyOf": ["/orders/*"]}})]),
        actions: vec![static_route("orders")],
        description: Some("the first rule".to_string()),
    };
    let second = Submitted {
        priority: 20,
        conditions: Some(vec![match_principals(vec![iam_principal(
            "arn:aws:iam::000000000000:role/Reader",
            Some("StringEquals"),
        )])]),
        actions: vec![json!({"configurationBundle": {
            "staticOverride": bundle_reference(),
        }})],
        description: Some("the second rule".to_string()),
    };

    let created_first = create_rule(&router, &gateway_id, &first.body()).await;
    assert_eq!(
        created_first.status, 202,
        "setup: the first rule must be created, got {} ({})",
        created_first.status, created_first.body
    );
    let created_second = create_rule(&router, &gateway_id, &second.body()).await;
    assert_eq!(
        created_second.status, 202,
        "a second rule is created beside the first, got {} ({})",
        created_second.status, created_second.body
    );
    let first_id = text(&created_first.body, "ruleId");
    let second_id = text(&created_second.body, "ruleId");
    assert_ne!(
        second_id, first_id,
        "the two rules are held under different ids, got {}",
        created_second.body
    );

    let held = get_rule(&router, &gateway_id, &first_id).await;
    assert_eq!(
        held.status, 200,
        "the first rule is still held, got {} ({})",
        held.status, held.body
    );
    assert_eq!(
        (held.body.get("priority"), held.body.get("actions")),
        (
            Some(&json!(first.priority)),
            Some(&json!(first.actions.clone()))
        ),
        "the second create leaves the first rule alone, got {}",
        held.body
    );
    let other = get_rule(&router, &gateway_id, &second_id).await;
    assert_eq!(
        (other.body.get("priority"), other.body.get("actions")),
        (
            Some(&json!(second.priority)),
            Some(&json!(second.actions.clone()))
        ),
        "the second rule holds its own priority and actions, got {}",
        other.body
    );
}

/// A replayed request carrying the same `clientToken` answers with the rule that token
/// first created, rather than creating a second one — "If this token matches a previous
/// request, the service ignores the request, but doesn't return an error."
#[tokio::test]
async fn a_replayed_client_token_creates_one_rule() {
    let router = router();
    let gateway_id = gateway(&router, "createrulereplay").await;

    let mut body = Submitted {
        priority: 7,
        conditions: Some(vec![json!({"matchPaths": {"anyOf": ["/orders/*"]}})]),
        actions: vec![static_route("orders")],
        description: Some("the first submission".to_string()),
    }
    .body();
    body["clientToken"] = json!(CLIENT_TOKEN);

    let first = create_rule(&router, &gateway_id, &body).await;
    assert_eq!(
        first.status, 202,
        "setup: the first submission must be created, got {} ({})",
        first.status, first.body
    );

    // The replay differs in every member a second create would store, so an answer
    // carrying the FIRST submission's values can only come from the stored rule.
    let mut replayed = body.clone();
    replayed["priority"] = json!(900);
    replayed["description"] = json!("the replayed submission");
    replayed["actions"] = json!([static_route("ordersNext")]);
    let second = create_rule(&router, &gateway_id, &replayed).await;
    assert_eq!(
        second.status, 202,
        "a replayed clientToken is not an error, got {} ({})",
        second.status, second.body
    );
    for member in ["ruleId", "priority", "actions", "description", "createdAt"] {
        assert_eq!(
            second.body.get(member),
            first.body.get(member),
            "a replayed clientToken answers with the {member} the token first produced, got {}",
            second.body
        );
    }

    let stored = get_rule(&router, &gateway_id, &text(&first.body, "ruleId")).await;
    assert_eq!(
        stored.body.get("actions"),
        first.body.get("actions"),
        "the replay wrote nothing over the rule the token first created, got {}",
        stored.body
    );

    // A replay is a replay only on the gateway the first request addressed: the same
    // token presented to ANOTHER gateway creates that gateway's own rule, rather than
    // reporting a rule the addressed gateway does not hold.
    let other_gateway_id = gateway(&router, "createruleotherreplay").await;
    let elsewhere = create_rule(&router, &other_gateway_id, &replayed).await;
    assert_eq!(
        elsewhere.status, 202,
        "the same token on another gateway is served as a fresh create, got {} ({})",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        text(&elsewhere.body, "ruleId"),
        text(&first.body, "ruleId"),
        "the rule created on the other gateway is its own, got {}",
        elsewhere.body
    );
    assert_eq!(
        elsewhere.body.get("actions"),
        replayed.get("actions"),
        "the other gateway's rule holds what THAT request submitted, got {}",
        elsewhere.body
    );
    let on_the_other = get_rule(
        &router,
        &other_gateway_id,
        &text(&elsewhere.body, "ruleId"),
    )
    .await;
    assert_eq!(
        on_the_other.status, 200,
        "the other gateway holds the rule it was told to create, got {} ({})",
        on_the_other.status, on_the_other.body
    );
    let not_here = get_rule(&router, &other_gateway_id, &text(&first.body, "ruleId")).await;
    assert_eq!(
        (not_here.status, error_code(&not_here.body).as_str()),
        (404, "ResourceNotFoundException"),
        "the first gateway's rule is not held by the other gateway, got {} {} ({})",
        not_here.status,
        error_code(&not_here.body),
        not_here.body
    );
}

/// One row of the refusal table: a request that is not a rule this service will store.
struct Refused {
    case: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
}

/// The refusals. Each names a constraint the model publishes and crosses it by one step:
/// `GatewayRulePriority` is `@range 1..1000000`, `Actions` is `@length 1..2` and
/// `@required`, `Conditions` is `@length 0..2`, and `GatewayRuleDescription` is
/// `@length 1..256`.
fn refusals() -> Vec<Refused> {
    let one_action = json!([static_route("orders")]);
    let one_condition = json!({"matchPaths": {"anyOf": ["/orders/*"]}});
    vec![
        Refused {
            case: "a priority one below the published lower bound",
            request: json!({"priority": 0, "actions": one_action}),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "a priority one above the published upper bound",
            request: json!({"priority": 1_000_001, "actions": one_action}),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "no actions at all, below the published lower bound of one",
            request: json!({"priority": 5, "actions": []}),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "three actions, one above the published upper bound of two",
            request: json!({"priority": 5, "actions": [
                static_route("a"),
                static_route("b"),
                static_route("c"),
            ]}),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "three conditions, one above the published upper bound of two",
            request: json!({
                "priority": 5,
                "actions": one_action,
                "conditions": [one_condition, one_condition, one_condition],
            }),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "an empty description, below the published lower bound of one character",
            request: json!({"priority": 5, "actions": one_action, "description": ""}),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "a description one character past the published upper bound",
            request: json!({
                "priority": 5,
                "actions": one_action,
                "description": "d".repeat(257),
            }),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "no actions member at all, which the model makes required",
            request: json!({"priority": 5}),
            status: 400,
            code: "ValidationException",
        },
    ]
}

/// A request that is not a rule this service will store is refused the modelled error and
/// writes nothing: the rule the gateway already held is untouched, and the gateway holds no
/// rule the refused request could be read back as.
#[tokio::test]
async fn a_request_outside_the_published_constraints_is_refused_and_writes_nothing() {
    let router = router();
    let gateway_id = gateway(&router, "createrulerefused").await;
    let held = Submitted {
        priority: 11,
        conditions: Some(vec![json!({"matchPaths": {"anyOf": ["/orders/*"]}})]),
        actions: vec![static_route("orders")],
        description: Some("written before the refusals".to_string()),
    }
    .body();
    let written = create_rule(&router, &gateway_id, &held).await;
    assert_eq!(
        written.status, 202,
        "setup: the held rule must be created, got {} ({})",
        written.status, written.body
    );
    let held_id = text(&written.body, "ruleId");

    for Refused {
        case,
        request,
        status,
        code,
    } in refusals()
    {
        let refused = create_rule(&router, &gateway_id, &request).await;
        assert_eq!(
            (refused.status, error_code(&refused.body).as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {} ({})",
            refused.status,
            error_code(&refused.body),
            refused.body
        );

        let still = get_rule(&router, &gateway_id, &held_id).await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the refused create leaves the held rule in place, got {} ({})",
            still.status, still.body
        );
        assert_eq!(
            still.body.get("priority"),
            held.get("priority"),
            "case {case:?}: the held rule keeps its own priority, got {}",
            still.body
        );
    }
}

/// The rules one gateway holds are bounded: a caller looping on create meets the modelled
/// quota rather than this process's memory limit, and the rules the gateway did take are
/// still held.
#[tokio::test]
async fn a_gateway_holds_a_bounded_number_of_rules() {
    /// A ceiling on the loop itself, so a bound that is missing fails the test instead of
    /// running forever. Far above the ceiling `src/state/gateway_rules.rs` sets.
    const ATTEMPT_CEILING: usize = 10_000;

    let router = router();
    let gateway_id = gateway(&router, "createrulequota").await;
    let body = Submitted {
        priority: 1,
        conditions: None,
        actions: vec![static_route("orders")],
        description: None,
    }
    .body();

    let mut accepted: Vec<String> = Vec::new();
    let refused = loop {
        let answer = create_rule(&router, &gateway_id, &body).await;
        if answer.status != 202 {
            break answer;
        }
        accepted.push(text(&answer.body, "ruleId"));
        assert!(
            accepted.len() < ATTEMPT_CEILING,
            "the create accepted {} rules on one gateway without ever meeting a bound",
            accepted.len()
        );
    };
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "a gateway already holding its maximum rules refuses the next create, got {} {} ({})",
        refused.status,
        error_code(&refused.body),
        refused.body
    );
    assert!(
        accepted.len() > 1,
        "the bound is a ceiling, not a ban: one gateway takes many rules, got {}",
        accepted.len()
    );

    // The refusal wrote nothing over what the gateway already held.
    let first = accepted.first().expect("at least one rule was accepted");
    let last = accepted.last().expect("at least one rule was accepted");
    for rule_id in [first, last] {
        let held = get_rule(&router, &gateway_id, rule_id).await;
        assert_eq!(
            held.status, 200,
            "the rule {rule_id} the gateway took is still held, got {} ({})",
            held.status, held.body
        );
    }
}

/// A read resolves the gateway and then the rule: an identifier no gateway of this account
/// holds, and a `ruleId` the gateway does not hold, are both the modelled 404.
#[tokio::test]
async fn a_read_of_a_rule_nothing_holds_is_the_modelled_404() {
    let router = router();
    let gateway_id = gateway(&router, "createruleunknownread").await;
    // Inside `GatewayRuleId`'s own pattern; no rule of this gateway holds it.
    const UNHELD_RULE: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

    let unknown_rule = get_rule(&router, &gateway_id, UNHELD_RULE).await;
    assert_eq!(
        (unknown_rule.status, error_code(&unknown_rule.body).as_str()),
        (404, "ResourceNotFoundException"),
        "a ruleId the gateway does not hold is the modelled 404, got {} {} ({})",
        unknown_rule.status,
        error_code(&unknown_rule.body),
        unknown_rule.body
    );

    // `GatewayIdentifier`'s own pattern admits this id; no gateway holds it.
    let unknown_gateway = get_rule(&router, "nosuchgateway-abcdefghij", UNHELD_RULE).await;
    assert_eq!(
        (
            unknown_gateway.status,
            error_code(&unknown_gateway.body).as_str()
        ),
        (404, "ResourceNotFoundException"),
        "a gateway nothing holds is the modelled 404, got {} {} ({})",
        unknown_gateway.status,
        error_code(&unknown_gateway.body),
        unknown_gateway.body
    );
}

/// The create resolves its gateway first: an identifier no gateway of this account holds is
/// the modelled 404, a gateway the clock is still creating TAKES the rule (the rule carries
/// a clock-owed status of its own), and a gateway whose deletion has been accepted takes no
/// further configuration.
#[tokio::test]
async fn the_create_resolves_its_gateway_first() {
    let router = router();
    let body = Submitted {
        priority: 3,
        conditions: Some(vec![json!({"matchPaths": {"anyOf": ["/orders/*"]}})]),
        actions: vec![static_route("orders")],
        description: None,
    }
    .body();

    // `GatewayIdentifier`'s own pattern admits this id; no gateway holds it.
    let missing = create_rule(&router, "nosuchgateway-abcdefghij", &body).await;
    assert_eq!(
        (missing.status, error_code(&missing.body).as_str()),
        (404, "ResourceNotFoundException"),
        "an unknown gateway is the modelled 404, got {} {} ({})",
        missing.status,
        error_code(&missing.body),
        missing.body
    );

    let gateway_id = gateway(&router, "createrulecreating").await;
    let creating = create_rule(&router, &gateway_id, &body).await;
    assert_eq!(
        creating.status, 202,
        "a gateway the clock is still creating takes a rule, got {} ({})",
        creating.status, creating.body
    );
    let stored = get_rule(&router, &gateway_id, &text(&creating.body, "ruleId")).await;
    assert_eq!(
        stored.status, 200,
        "the rule written on a creating gateway is held, got {} ({})",
        stored.status, stored.body
    );

    // A gateway on its way out takes no further configuration.
    gateway_ready(&router, &gateway_id).await;
    let deleted = send(&router, "DELETE", &format!("/gateways/{gateway_id}/"), None).await;
    assert_eq!(
        deleted.status, 202,
        "setup: the gateway's deletion must be accepted, got {} ({})",
        deleted.status, deleted.body
    );
    let deleting = create_rule(&router, &gateway_id, &body).await;
    assert_eq!(
        (deleting.status, error_code(&deleting.body).as_str()),
        (400, "ValidationException"),
        "a gateway on its way out takes no rule, got {} {} ({})",
        deleting.status,
        error_code(&deleting.body),
        deleting.body
    );
}
