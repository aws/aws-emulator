// SPDX-License-Identifier: CC0-1.0

//! WHERE in the serving of a request a WAFv2 web ACL in front of a deployed REST API stage
//! decides: ahead of everything the api itself declares about that request.
//!
//! The rule this suite pins, one level above the recorded case:
//!
//! > A web ACL associated with a deployed stage inspects EVERY request to that stage before
//! > the stage applies anything the api declares about it, so a rule whose action
//! > TERMINATES settles the request at the edge — through the api's `WAF_FILTERED` gateway
//! > response — whatever the api's own route, authorization, API key or integration would
//! > have answered, and whether or not that rule's
//! > `VisibilityConfig$CloudWatchMetricsEnabled` is set, because that flag governs
//! > telemetry and never enforcement. A non-terminating `Count` leaves the request to the
//! > stage, which then answers it exactly as it does with no web ACL in front of it at all,
//! > and enforcement lasts exactly as long as the association does.
//!
//! The rows differ along what the rule generalises over: WHAT the api itself would have
//! answered the request — a path its deployment holds no resource for, a method whose
//! authorization needs a credential the request carries none of, a method that requires an
//! API key the request does not present, a method whose configuration cannot serve the call
//! at all, and a method that answers from its integration — so the rows span the `403`, the
//! `500` and the `200` the stage reaches on its own, and the terminating action's answer
//! must be the same `403` for all five. The `CloudWatchMetricsEnabled` flag alternates
//! across the rows, so no row's enforcement can depend on it.
//!
//! Every call goes through the real wire path: the api, its stages and its gateway-response
//! customization through API Gateway's own control plane, the web ACLs through WAFv2's, and
//! the requests are unsigned GETs to the api's own invoke host — both services mounted on
//! ONE shared engine behind the multi-service router, as `core/router/src/lib.rs` mounts
//! them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the invoke host, both control planes and every ARN name.
const REGION: &str = "us-east-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource.
const SCOPE: &str = "REGIONAL";

/// The three stages of the one api every row is driven at: the stage no web ACL is in front
/// of (the baseline each row's own answer is read from), the stage whose web ACL BLOCKS the
/// row's path, and the stage whose web ACL only COUNTS it.
const BARE_STAGE: &str = "bare";
const BLOCKED_STAGE: &str = "blocked";
const COUNTED_STAGE: &str = "counted";

/// The gateway response type API Gateway answers a request its web ACL blocked through.
/// Customized below, because the refusal this type carries by default — `403`
/// `{"message":"Forbidden"}` — is word for word the one a key-required method reached
/// without a key earns, and this suite has to tell the two apart.
const WAF_FILTERED: &str = "WAF_FILTERED";

/// The status `WAF_FILTERED` answers under, and the sentence the customization below puts in
/// its body, which is what identifies an answer as the web ACL's rather than the api's.
const WAF_FILTERED_STATUS: u16 = 403;
const WAF_FILTERED_MESSAGE: &str = "refused by the web acl";

/// How many times a request to a blocked path is repeated: a web ACL decides every request,
/// not merely the first one past an association, so each row's refusal is asserted over a
/// window of them rather than once.
const REPEATS: usize = 5;

/// One emulator with API Gateway and WAFv2 mounted on a single shared engine.
struct Edge {
    router: MultiServiceRouter,
}

/// One answered request: its status and its body verbatim.
#[derive(Debug)]
struct Answered {
    status: u16,
    body: String,
}

impl Answered {
    /// The `message` member of a JSON body — what every API Gateway gateway response
    /// carries — or the body itself when it carries none.
    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| self.body.clone())
    }
}

impl Edge {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises the services, not SigV4: the control-plane requests carry a
        // credential scope so the router can name the target service, and the invoke
        // requests carry no credential at all, as a client of a published invoke URL sends
        // none.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    /// One API Gateway control-plane call that must be accepted with `expected`.
    async fn api_ok(&self, method: &str, uri: &str, body: Option<Value>, expected: u16) -> Value {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("apigateway.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/apigateway/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, expected,
            "setup {method} {uri}: expected HTTP {expected}, got {status} with {parsed}"
        );
        parsed
    }

    /// One WAFv2 call that must succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://wafv2.example.com/")
            .header("host", "wafv2.example.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLEWAFV2/20260101/{REGION}/wafv2/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        assert_eq!(
            status, 200,
            "setup {operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One unsigned GET to a deployed stage's invoke URL — the api's own host, no
    /// credential, as a client of a published invoke URL sends it.
    async fn get(&self, api: &str, path: &str) -> Answered {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}{path}"))
            .header("host", &host)
            .body(Bytes::new())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).into_owned();
        Answered { status, body }
    }
}

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by:
/// `arn:aws:apigateway:{region}::/restapis/{api-id}/stages/{stage}`, the one form
/// `AssociateWebACLRequest$ResourceArn` documents for an API Gateway stage.
fn stage_arn(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// What the api itself declares about one row's path, which is what decides the answer the
/// STAGE reaches on its own — every one of them a decision the web ACL must precede.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Declared {
    /// Nothing: the deployment holds no resource for this path, so the api cannot even
    /// place the route.
    NoRoute,
    /// A `GET` whose `authorizationType` is `AWS_IAM`, reached by an unsigned request.
    IamAuthorized,
    /// A `GET` whose `apiKeyRequired` is true, reached by a request presenting no key.
    KeyRequired,
    /// A `GET` whose `MOCK` integration has no integration response that maps the call —
    /// its only one carries a selection pattern — so the method's configuration cannot
    /// serve the call.
    UnservableIntegration,
    /// A `GET` on `NONE` authorization whose `MOCK` integration answers `200`.
    Served,
}

/// One row: a path, what the api declares about it, what the stage answers it on its own,
/// and the `CloudWatchMetricsEnabled` its rule carries.
struct Row {
    case: &'static str,
    /// The path part under the api's root — also the suffix the row's rule matches on.
    path_part: &'static str,
    declared: Declared,
    /// What the stage answers a request to this path with NO web ACL in front of it: the
    /// status, and the `message` member of the body (the body verbatim when it carries
    /// none).
    own_status: u16,
    own_message: &'static str,
    /// The row's rule `VisibilityConfig$CloudWatchMetricsEnabled`. Alternated across the
    /// rows: the flag is the modelled switch for the telemetry a rule publishes, so nothing
    /// this suite asserts may vary with it.
    metrics_enabled: bool,
}

/// The table: one row per answer the stage reaches on its own, spanning the three statuses
/// it can settle a request with. The sentences are API Gateway's own for each — a route it
/// cannot place and an `AWS_IAM` method reached unsigned are both
/// `Missing Authentication Token`, a key-required method reached without a key is
/// `Forbidden`, a method whose configuration cannot serve the call is
/// `Internal server error`, and a served request is the integration's own body.
const ROWS: [Row; 5] = [
    Row {
        case: "a path the deployment holds no resource for",
        path_part: "nowhere",
        declared: Declared::NoRoute,
        own_status: 403,
        own_message: "Missing Authentication Token",
        metrics_enabled: false,
    },
    Row {
        case: "a method whose authorization needs a credential the request carries none of",
        path_part: "signed",
        declared: Declared::IamAuthorized,
        own_status: 403,
        own_message: "Missing Authentication Token",
        metrics_enabled: true,
    },
    Row {
        case: "a method that requires an API key the request does not present",
        path_part: "keyed",
        declared: Declared::KeyRequired,
        own_status: 403,
        own_message: "Forbidden",
        metrics_enabled: false,
    },
    Row {
        case: "a method whose configuration cannot serve the call at all",
        path_part: "unwired",
        declared: Declared::UnservableIntegration,
        own_status: 500,
        own_message: "Internal server error",
        metrics_enabled: true,
    },
    Row {
        case: "a method that answers from its integration",
        path_part: "served",
        declared: Declared::Served,
        own_status: 200,
        own_message: r#"{"at":"served"}"#,
        metrics_enabled: false,
    },
];

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// A rule matching the requests whose URI path — `/{stage}/{resource-path}`, the path the
/// edge inspects — ends in `/{path_part}`, applying `action` (the single member of
/// `RuleAction` it names) with the row's own visibility configuration.
fn rule_of(priority: usize, row: &Row, action: &str) -> Value {
    json!({
        "Name": format!("{}-rule", row.path_part),
        "Priority": priority as i64,
        "Statement": {
            "ByteMatchStatement": {
                "SearchString": base64_of(&format!("/{}", row.path_part)),
                "FieldToMatch": { "UriPath": {} },
                "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                "PositionalConstraint": "ENDS_WITH",
            }
        },
        "Action": { action: {} },
        "VisibilityConfig": {
            "SampledRequestsEnabled": true,
            "CloudWatchMetricsEnabled": row.metrics_enabled,
            "MetricName": format!("{}Rule", row.path_part),
        },
    })
}

/// A web ACL named `name` whose `DefaultAction` allows, carrying one rule per row applying
/// `action` to that row's path. Answers its ARN.
async fn create_web_acl(edge: &Edge, name: &str, action: &str) -> String {
    let rules: Vec<Value> = ROWS
        .iter()
        .enumerate()
        .map(|(priority, row)| rule_of(priority, row, action))
        .collect();
    let created = edge
        .waf_ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "Rules": rules,
                "VisibilityConfig": {
                    "SampledRequestsEnabled": true,
                    "CloudWatchMetricsEnabled": true,
                    "MetricName": name,
                },
            }),
        )
        .await;
    created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string()
}

/// Put the web ACL `web_acl_arn` in front of `api`'s `stage`.
async fn associate(edge: &Edge, web_acl_arn: &str, api: &str, stage: &str) {
    edge.waf_ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": stage_arn(api, stage) }),
    )
    .await;
}

/// Take the web ACL in front of `api`'s `stage` away again.
async fn disassociate(edge: &Edge, api: &str, stage: &str) {
    edge.waf_ok(
        "DisassociateWebACL",
        json!({ "ResourceArn": stage_arn(api, stage) }),
    )
    .await;
}

/// An api carrying one resource per row that declares one, each declared as its row says,
/// deployed to every stage in `stages`. Answers the api's id.
async fn deployed_api(edge: &Edge, name: &str, stages: &[&str]) -> String {
    let created = edge
        .api_ok("POST", "/restapis", Some(json!({ "name": name })), 201)
        .await;
    let api = created["id"].as_str().expect("the api id").to_string();
    let root = created["rootResourceId"]
        .as_str()
        .expect("the root resource id")
        .to_string();

    for row in &ROWS {
        if row.declared == Declared::NoRoute {
            continue;
        }
        let resource = edge
            .api_ok(
                "POST",
                &format!("/restapis/{api}/resources/{root}"),
                Some(json!({ "pathPart": row.path_part })),
                201,
            )
            .await;
        let resource = resource["id"]
            .as_str()
            .expect("the resource id")
            .to_string();
        let method = match row.declared {
            Declared::IamAuthorized => json!({ "authorizationType": "AWS_IAM" }),
            Declared::KeyRequired => {
                json!({ "authorizationType": "NONE", "apiKeyRequired": true })
            }
            _ => json!({ "authorizationType": "NONE" }),
        };
        edge.api_ok(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(method),
            201,
        )
        .await;
        // Every method carries an integration, because an api holding one that does not
        // cannot be deployed at all. The row whose configuration cannot serve the call
        // carries a `MOCK` whose ONLY integration response is patterned, so nothing maps
        // the call the mock returns — which is the fault class that row drives.
        edge.api_ok(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(json!({ "type": "MOCK" })),
            201,
        )
        .await;
        let mapped = if row.declared == Declared::UnservableIntegration {
            json!({ "selectionPattern": "4\\d\\d" })
        } else {
            json!({
                "responseTemplates": {
                    "application/json": format!("{{\"at\":\"{}\"}}", row.path_part)
                }
            })
        };
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"
            ),
            Some(mapped),
            201,
        )
        .await;
    }

    // The one customization this suite makes: the sentence a blocked caller reads, so an
    // answer the web ACL decided is told apart from the api's own `403 Forbidden`.
    edge.api_ok(
        "PUT",
        &format!("/restapis/{api}/gatewayresponses/{WAF_FILTERED}"),
        Some(json!({
            "statusCode": WAF_FILTERED_STATUS.to_string(),
            "responseTemplates": {
                "application/json": format!("{{\"message\":\"{WAF_FILTERED_MESSAGE}\"}}")
            },
        })),
        201,
    )
    .await;

    for stage in stages {
        edge.api_ok(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
            201,
        )
        .await;
    }
    api
}

/// Assert that `stage` answers `row`'s path with `status` and `message`, over a window of
/// requests rather than one — a web ACL decides every request to the stage it is in front
/// of, so a stage that enforces once and then stops fails here.
async fn answers(edge: &Edge, api: &str, stage: &str, row: &Row, status: u16, message: &str) {
    let path = format!("/{stage}/{}", row.path_part);
    for ordinal in 1..=REPEATS {
        let answered = edge.get(api, &path).await;
        assert_eq!(
            answered.status, status,
            "case {:?} at the {stage} stage, request {ordinal} of {REPEATS}: expected HTTP \
             {status}, got {answered:?}",
            row.case
        );
        assert_eq!(
            answered.message(),
            message,
            "case {:?} at the {stage} stage, request {ordinal} of {REPEATS}: expected the \
             sentence {message:?}, got {answered:?}",
            row.case
        );
    }
}

/// RULE: a terminating rule of the web ACL in front of a stage settles the request before
/// the stage applies anything the api declares about it, so its answer replaces the api's
/// own whatever that would have been and whether or not the rule reports metrics; a
/// non-terminating `Count` leaves the api's own answer exactly as it is.
#[tokio::test]
async fn a_web_acl_decides_before_anything_the_api_declares_about_the_request() {
    let edge = Edge::new();
    let api = deployed_api(
        &edge,
        "edge-precedence",
        &[BARE_STAGE, BLOCKED_STAGE, COUNTED_STAGE],
    )
    .await;

    let blocking = create_web_acl(&edge, "precedence-blocking", "Block").await;
    associate(&edge, &blocking, &api, BLOCKED_STAGE).await;
    let counting = create_web_acl(&edge, "precedence-counting", "Count").await;
    associate(&edge, &counting, &api, COUNTED_STAGE).await;

    for row in &ROWS {
        // The baseline: with no web ACL in front of it, the stage answers what the api
        // declares about the path — which is what the rows differ along, so a row whose
        // baseline is not what it claims cannot pass as a precedence case below.
        answers(&edge, &api, BARE_STAGE, row, row.own_status, row.own_message).await;

        // The same path with a TERMINATING rule in front of it: the web ACL's answer, not
        // the api's, for every row and for both settings of the metrics flag.
        answers(
            &edge,
            &api,
            BLOCKED_STAGE,
            row,
            WAF_FILTERED_STATUS,
            WAF_FILTERED_MESSAGE,
        )
        .await;
        assert_ne!(
            (WAF_FILTERED_STATUS, WAF_FILTERED_MESSAGE),
            (row.own_status, row.own_message),
            "case {:?}: the row must differ from the blocked answer in the status or the \
             sentence, or it proves nothing about precedence",
            row.case
        );

        // And with a NON-terminating rule in front of it: `Count` records the match and
        // lets the request on, so the api decides it exactly as the baseline did.
        answers(
            &edge,
            &api,
            COUNTED_STAGE,
            row,
            row.own_status,
            row.own_message,
        )
        .await;
    }
}

/// RULE, continued: enforcement lasts exactly as long as the association — the stage a web
/// ACL is taken off is served by the api again, and putting one back in front of it enforces
/// again, so the binding the control plane reports is the binding the edge acts on.
#[tokio::test]
async fn a_stage_is_enforced_exactly_while_a_web_acl_is_associated_with_it() {
    let edge = Edge::new();
    let api = deployed_api(&edge, "edge-association-window", &[BLOCKED_STAGE]).await;
    let blocking = create_web_acl(&edge, "window-blocking", "Block").await;

    // The rows that would be SERVED or refused by the api itself alike: each is answered by
    // the api before the association, by the web ACL during it, and by the api again after
    // it — the same window driven three times over.
    for row in &ROWS {
        answers(
            &edge,
            &api,
            BLOCKED_STAGE,
            row,
            row.own_status,
            row.own_message,
        )
        .await;
    }

    associate(&edge, &blocking, &api, BLOCKED_STAGE).await;
    for row in &ROWS {
        answers(
            &edge,
            &api,
            BLOCKED_STAGE,
            row,
            WAF_FILTERED_STATUS,
            WAF_FILTERED_MESSAGE,
        )
        .await;
    }

    disassociate(&edge, &api, BLOCKED_STAGE).await;
    for row in &ROWS {
        answers(
            &edge,
            &api,
            BLOCKED_STAGE,
            row,
            row.own_status,
            row.own_message,
        )
        .await;
    }

    // Put back: the same stage, the same web ACL, enforcing again.
    associate(&edge, &blocking, &api, BLOCKED_STAGE).await;
    for row in &ROWS {
        answers(
            &edge,
            &api,
            BLOCKED_STAGE,
            row,
            WAF_FILTERED_STATUS,
            WAF_FILTERED_MESSAGE,
        )
        .await;
    }
}
