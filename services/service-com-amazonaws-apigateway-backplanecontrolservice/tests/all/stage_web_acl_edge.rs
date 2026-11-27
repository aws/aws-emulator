// SPDX-License-Identifier: CC0-1.0

//! A WAFv2 WEB ACL in front of a deployed REST API stage, end to end over the wire.
//!
//! The rule this suite pins: a request to a deployed stage that has a web ACL
//! associated is DECIDED BY THAT WEB ACL before the stage serves it, and the stage
//! always produces an HTTP response — the web ACL's rules run in ascending priority,
//! the first terminating action settles the request, and the `DefaultAction` decides
//! one no rule matched. A block is answered at the edge (through the api's own
//! `WAF_FILTERED` gateway response, or with the `BlockAction`'s own `CustomResponse`
//! when it configures one), an allow reaches the origin carrying the headers a
//! `CustomRequestHandling` inserted, and a counted rule's labels are visible to the
//! rules that run after it.
//!
//! Each test asserts that rule over a TABLE rather than one recorded request: the rows
//! differ along what the rule generalises over — which statement decides, which
//! component it inspects, how large the inspected component is, where in a forwarded
//! header an address sits, how many requests a caller has already sent — and each row
//! names its case and the actual answer.
//!
//! Every call goes through the real wire path: the api and its stages are built through
//! API Gateway's own control plane, the web ACLs through WAFv2's, and the requests are
//! unsigned GETs and POSTs to the api's own invoke host — both services mounted on ONE
//! shared engine behind the multi-service router, which is what lets one request reach
//! the apigateway data plane and the web ACL the wafv2 control plane stored.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the invoke host and both control planes are addressed in. One region is
/// enough: the stage's ARN carries the region the invoke host names, and the
/// association is keyed by that ARN, so a second region would exercise the ARN
/// builder rather than the web ACL.
const REGION: &str = "us-east-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource
/// (`Scope`'s `CLOUDFRONT` value is for "a global resource type, such as a Amazon
/// CloudFront distribution").
const SCOPE: &str = "REGIONAL";

/// The sampling window every `GetSampledRequests` below asks about: WAF retains samples
/// for the three hours before the call and refuses a window reaching back past that, so
/// a window that is to see every sample the case drove reaches as far back as it can
/// without crossing that bound.
fn sampling_window() -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    json!({ "StartTime": now - 3 * 3_600 + 60, "EndTime": now + 60 })
}

/// One emulator with API Gateway and WAFv2 mounted on a single shared engine.
struct Edge {
    router: MultiServiceRouter,
    /// The engine both services were mounted on, kept so a case can reach the state WAF
    /// keeps about its own data plane — see [`Edge::due_rate_check`].
    engine: Arc<engine::InMemoryEngine>,
}

/// One answered request: its status, its headers and its body verbatim.
struct Answered {
    status: u16,
    headers: Vec<(String, String)>,
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

    /// The first value of the header `name`, or `None` when the response carries none.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl Edge {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise the services, not SigV4: the control-plane requests carry
        // a credential scope so the router can name the target service, and the invoke
        // requests carry no credential at all, as a client of a published invoke URL
        // sends none.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// Put every rate check WAF has made a whole check interval behind the clock, so the
    /// next request a rate-based rule decides re-checks the rate.
    ///
    /// "WAF checks the rate about every 10 seconds"
    /// (`RateBasedStatement$EvaluationWindowSec`), so a rate-based rule acts on the rate
    /// its last check arrived at. A suite can no more wait ten seconds for the next check
    /// than it can wait for an evaluation window to pass, so the stored check is AGED,
    /// which is what the passage of that much time does to when the next one falls due.
    fn due_rate_check(&self) {
        let state = service_com_amazonaws_wafv2_awswaf_20190729::state::service_state(
            &*self.engine,
        )
        .expect("the wafv2 store is reachable from the shared engine");
        let mut store = state.lock();
        for evaluation in store.rate_based_evaluations.values_mut() {
            evaluation.checked_at -=
                service_com_amazonaws_wafv2_awswaf_20190729::service::edge::RATE_CHECK_INTERVAL_MILLIS;
        }
    }

    /// One API Gateway control-plane call, answering its status and parsed body.
    async fn api_call(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
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
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// An API Gateway control-plane call that must be accepted with `expected`.
    async fn api_ok(&self, method: &str, uri: &str, body: Option<Value>, expected: u16) -> Value {
        let (status, parsed) = self.api_call(method, uri, body).await;
        assert_eq!(
            status, expected,
            "case {method} {uri}: expected {expected}, got {status} with {parsed}"
        );
        parsed
    }

    /// One WAFv2 call, answering its status and parsed body.
    async fn waf_call(&self, operation: &str, body: Value) -> (u16, Value) {
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
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, parsed)
    }

    /// A WAFv2 call that must succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.waf_call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One unsigned request to a deployed stage's invoke URL — the api's own host, no
    /// credential, the headers and body the case sends.
    async fn invoke(
        &self,
        api: &str,
        verb: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Answered {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(verb)
            .uri(format!("https://{host}{path}"))
            .header("host", &host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = String::from_utf8_lossy(&response.into_body()).into_owned();
        Answered {
            status,
            headers,
            body,
        }
    }

    /// [`Self::invoke`]: a plain GET with no headers and no body.
    async fn get(&self, api: &str, path: &str) -> Answered {
        self.invoke(api, "GET", path, &[], &[]).await
    }
}

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by:
/// `arn:aws:apigateway:{region}::/restapis/{api-id}/stages/{stage}`, the one form
/// `AssociateWebACLRequest$ResourceArn` documents for an API Gateway stage.
fn stage_arn(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// A `VisibilityConfig` that stores samples, so `GetSampledRequests` has a population
/// to draw from.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// A web ACL rule: a name, a priority, a statement and an action.
fn rule(name: &str, priority: i64, statement: Value, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A `ByteMatchStatement` over `field` with no transformation.
fn byte_match(field: Value, search: &str, constraint: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(search),
            "FieldToMatch": field,
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": constraint,
        }
    })
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// Create a web ACL with `default_action` and `rules`, answering `(id, arn, lock
/// token)`.
async fn create_web_acl(
    edge: &Edge,
    name: &str,
    default_action: Value,
    rules: Value,
    extra: Value,
) -> (String, String, String) {
    let mut body = json!({
        "Name": name,
        "Scope": SCOPE,
        "DefaultAction": default_action,
        "VisibilityConfig": visibility(name),
        "Rules": rules,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body[key] = value;
    }
    let created = edge.waf_ok("CreateWebACL", body).await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// Put the web ACL `web_acl_arn` in front of `api`'s `stage`.
async fn associate(edge: &Edge, web_acl_arn: &str, api: &str, stage: &str) {
    edge.waf_ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": stage_arn(api, stage) }),
    )
    .await;
}

/// An api carrying the resources every case below drives, each on a `MOCK` integration
/// that answers `200` with a body naming the resource — so the answer a request gets
/// says whether it reached the origin.
struct Api {
    id: String,
    /// The api's ROOT resource, so a case that needs a resource of its own can add one
    /// beside the ones every case drives.
    root: String,
}

impl Api {
    /// Build the api and its resources; the caller deploys the stages it needs.
    async fn build(edge: &Edge, name: &str) -> Self {
        let created = edge
            .api_ok("POST", "/restapis", Some(json!({ "name": name })), 201)
            .await;
        let id = created["id"].as_str().expect("the api id").to_string();
        let root = created["rootResourceId"]
            .as_str()
            .expect("the root resource id")
            .to_string();
        let api = Self {
            id,
            root: root.clone(),
        };

        // A request validator that checks the method's request PARAMETERS, so a method
        // can declare a header required: that is what makes a header WAF inserted
        // observable at the origin, since a request missing it is refused by the api
        // itself.
        let validator = edge
            .api_ok(
                "POST",
                &format!("/restapis/{}/requestvalidators", api.id),
                Some(json!({ "name": "parameters", "validateRequestParameters": true })),
                201,
            )
            .await;
        let validator_id = validator["id"]
            .as_str()
            .expect("the validator id")
            .to_string();

        for (path_part, verb, extra) in [
            ("orders", "GET", json!({})),
            ("admin", "GET", json!({})),
            ("upload", "POST", json!({})),
            (
                "tagged",
                "GET",
                json!({
                    "requestParameters": { "method.request.header.x-amzn-waf-tag": true },
                    "requestValidatorId": validator_id,
                }),
            ),
        ] {
            api.resource(edge, &root, path_part, verb, extra).await;
        }
        api
    }

    /// One resource under `root` with a method on `verb` and a `MOCK` integration that
    /// answers `200 {"at":"<path_part>"}`.
    async fn resource(&self, edge: &Edge, root: &str, path_part: &str, verb: &str, extra: Value) {
        self.mock_resource(edge, root, path_part, verb, extra, &origin_body(path_part))
            .await;
    }

    /// One `GET` resource under the api's root whose `MOCK` integration answers `200`
    /// with a body ECHOING each name in `echoed`: one JSON member per name, carrying the
    /// value the request reaching the ORIGIN held under that header, and the empty string
    /// for a name it did not carry.
    ///
    /// This is what makes an inserted header observable as a CLIENT observes it: the body
    /// a request is answered with reports, name by name, exactly the headers the
    /// integration was reached with.
    async fn echoing_resource(&self, edge: &Edge, path_part: &str, echoed: &[String]) {
        let members: Vec<String> = echoed
            .iter()
            .map(|name| format!("\"{name}\":\"$input.params('{name}')\""))
            .collect();
        let template = format!("{{{}}}", members.join(","));
        let root = self.root.clone();
        self.mock_resource(edge, &root, path_part, "GET", json!({}), &template)
            .await;
    }

    /// One resource under `root` with a method on `verb` and a `MOCK` integration whose
    /// `200` response template is `template`, answering the id of the resource it
    /// created so a case can hang its own children under it.
    async fn mock_resource(
        &self,
        edge: &Edge,
        root: &str,
        path_part: &str,
        verb: &str,
        extra: Value,
        template: &str,
    ) -> String {
        let created = edge
            .api_ok(
                "POST",
                &format!("/restapis/{}/resources/{root}", self.id),
                Some(json!({ "pathPart": path_part })),
                201,
            )
            .await;
        let resource = created["id"].as_str().expect("the resource id").to_string();
        let mut method = json!({ "authorizationType": "NONE" });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            method[key] = value;
        }
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{}/resources/{resource}/methods/{verb}",
                self.id
            ),
            Some(method),
            201,
        )
        .await;
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{}/resources/{resource}/methods/{verb}/integration",
                self.id
            ),
            Some(json!({ "type": "MOCK" })),
            201,
        )
        .await;
        edge.api_ok(
            "PUT",
            &format!(
                "/restapis/{}/resources/{resource}/methods/{verb}/integration/responses/200",
                self.id
            ),
            Some(json!({
                "responseTemplates": { "application/json": template }
            })),
            201,
        )
        .await;
        resource
    }

    /// Deploy the api into `stage`.
    async fn deploy(&self, edge: &Edge, stage: &str) {
        edge.api_ok(
            "POST",
            &format!("/restapis/{}/deployments", self.id),
            Some(json!({ "stageName": stage })),
            201,
        )
        .await;
    }
}

/// The body a resource's `MOCK` integration answers with.
fn origin_body(path_part: &str) -> String {
    format!("{{\"at\":\"{path_part}\"}}")
}

/// A request body of `len` filler bytes carrying `needle` at offset `at`, or no occurrence of
/// it at all when `at` is `None`.
///
/// What a body-inspection case varies is the SIZE of the body and WHERE in it the fragment a
/// rule hunts for sits, so those are the two knobs; the filler is a byte the fragment does not
/// contain, so the only occurrence is the one the case placed.
fn body_carrying(len: usize, needle: &str, at: Option<usize>) -> Vec<u8> {
    let mut body = vec![b'.'; len];
    if let Some(at) = at {
        body[at..at + needle.len()].copy_from_slice(needle.as_bytes());
    }
    body
}

/// The rule over the STATEMENTS that decide: which action a web ACL settles on, and
/// what the caller is answered, for each kind of rule a web ACL can carry.
///
/// One row per deciding path — the default action with no rule at all, the default
/// action BLOCK, a byte-match rule that blocks with the api's own gateway response, a
/// byte-match rule that blocks with the web ACL's own custom response, an allow rule
/// whose custom request handling the origin sees, and a label a counted rule added
/// that a later rule blocks on — each on its own stage of the same api, so the
/// decision is the web ACL's and nothing else.
#[tokio::test]
async fn a_web_acl_in_front_of_a_stage_decides_every_request_the_stage_answers() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-decisions").await;
    for stage in [
        "none", "allowed", "blocked", "filtered", "custom", "tagging", "labelled",
    ] {
        api.deploy(&edge, stage).await;
    }

    // A stage with NO web ACL: the baseline every other row is read against.
    // `tagged` declares its header required, so the stage itself refuses a request
    // that does not carry one — which is how the inserted-header row below is proved.

    // A web ACL with no rules at all and the default `Allow`.
    let (_, allow_arn, _) =
        create_web_acl(&edge, "allow-all", json!({ "Allow": {} }), json!([]), json!({})).await;
    associate(&edge, &allow_arn, &api.id, "allowed").await;

    // A web ACL whose DEFAULT action blocks: nothing matches, so the default decides.
    let (_, block_arn, _) =
        create_web_acl(&edge, "block-all", json!({ "Block": {} }), json!([]), json!({})).await;
    associate(&edge, &block_arn, &api.id, "blocked").await;

    // A blocking rule with no custom response: answered through the api's own
    // `WAF_FILTERED` gateway response.
    let (_, filtered_arn, _) = create_web_acl(
        &edge,
        "block-orders",
        json!({ "Allow": {} }),
        json!([rule(
            "orders",
            0,
            byte_match(json!({ "UriPath": {} }), "/orders", "ENDS_WITH"),
            json!({ "Block": {} }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &filtered_arn, &api.id, "filtered").await;

    // The same rule with the web ACL's OWN custom response: the status, the body and
    // the header are the web ACL's, served at the edge.
    let (_, custom_arn, _) = create_web_acl(
        &edge,
        "block-with-body",
        json!({ "Allow": {} }),
        json!([rule(
            "orders",
            0,
            byte_match(json!({ "UriPath": {} }), "/orders", "ENDS_WITH"),
            json!({
                "Block": {
                    "CustomResponse": {
                        "ResponseCode": 429,
                        "CustomResponseBodyKey": "slow-down",
                        "ResponseHeaders": [{ "Name": "x-waf-case", "Value": "custom" }],
                    }
                }
            }),
        )]),
        json!({
            "CustomResponseBodies": {
                "slow-down": { "ContentType": "APPLICATION_JSON", "Content": "{\"waf\":\"too fast\"}" }
            }
        }),
    )
    .await;
    associate(&edge, &custom_arn, &api.id, "custom").await;

    // An allow rule whose `CustomRequestHandling` inserts a header: the origin sees it,
    // which the method's own required-parameter validator is what proves.
    let (_, tagging_arn, _) = create_web_acl(
        &edge,
        "tag-requests",
        json!({ "Allow": {} }),
        json!([rule(
            "tag",
            0,
            byte_match(json!({ "UriPath": {} }), "/tagged", "ENDS_WITH"),
            json!({
                "Allow": {
                    "CustomRequestHandling": {
                        "InsertHeaders": [{ "Name": "tag", "Value": "edge" }]
                    }
                }
            }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &tagging_arn, &api.id, "tagging").await;

    // A COUNTED rule that adds a label, and a later rule that blocks on it: the label
    // a rule adds is visible to the rules that run after it.
    let (_, labelled_arn, _) = create_web_acl(
        &edge,
        "label-then-block",
        json!({ "Allow": {} }),
        json!([
            json!({
                "Name": "mark",
                "Priority": 0,
                "Statement": byte_match(json!({ "SingleHeader": { "Name": "x-tenant" } }), "beta", "EXACTLY"),
                "Action": { "Count": {} },
                "RuleLabels": [{ "Name": "beta" }],
                "VisibilityConfig": visibility("mark"),
            }),
            rule(
                "on-label",
                1,
                json!({ "LabelMatchStatement": { "Scope": "LABEL", "Key": "beta" } }),
                json!({ "Block": {} }),
            ),
        ]),
        json!({}),
    )
    .await;
    associate(&edge, &labelled_arn, &api.id, "labelled").await;

    /// One request and the answer the web ACL in front of its stage earns it.
    struct Case {
        case: &'static str,
        path: String,
        headers: Vec<(&'static str, &'static str)>,
        status: u16,
        body: String,
        response_header: Option<(&'static str, &'static str)>,
    }

    let cases = vec![
        Case {
            case: "no web acl at all: the stage serves the request as it arrived",
            path: "/none/orders".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("orders"),
            response_header: None,
        },
        Case {
            case: "a web acl with no rules and the default Allow",
            path: "/allowed/orders".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("orders"),
            response_header: None,
        },
        Case {
            case: "a web acl whose default action blocks",
            path: "/blocked/orders".to_string(),
            headers: Vec::new(),
            status: 403,
            body: "Forbidden".to_string(),
            response_header: None,
        },
        Case {
            case: "a blocking rule, answered through the api's WAF_FILTERED response",
            path: "/filtered/orders".to_string(),
            headers: Vec::new(),
            status: 403,
            body: "Forbidden".to_string(),
            response_header: None,
        },
        Case {
            case: "the same web acl on a path the rule does not match",
            path: "/filtered/admin".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("admin"),
            response_header: None,
        },
        Case {
            case: "a blocking rule with the web acl's own custom response",
            path: "/custom/orders".to_string(),
            headers: Vec::new(),
            status: 429,
            body: "{\"waf\":\"too fast\"}".to_string(),
            response_header: Some(("x-waf-case", "custom")),
        },
        Case {
            case: "a required header the origin only sees because WAF inserted it",
            path: "/tagging/tagged".to_string(),
            headers: Vec::new(),
            status: 200,
            body: origin_body("tagged"),
            response_header: None,
        },
        Case {
            case: "the same method with no web acl inserting that header",
            path: "/none/tagged".to_string(),
            headers: Vec::new(),
            status: 400,
            body: "Missing required request parameters: [x-amzn-waf-tag]".to_string(),
            response_header: None,
        },
        Case {
            case: "a label a counted rule added, blocked on by a later rule",
            path: "/labelled/orders".to_string(),
            headers: vec![("x-tenant", "beta")],
            status: 403,
            body: "Forbidden".to_string(),
            response_header: None,
        },
        Case {
            case: "the same web acl for a request the counted rule does not mark",
            path: "/labelled/orders".to_string(),
            headers: vec![("x-tenant", "alpha")],
            status: 200,
            body: origin_body("orders"),
            response_header: None,
        },
    ];

    for case in &cases {
        let answered = edge
            .invoke(&api.id, "GET", &case.path, &case.headers, &[])
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with body {:?}",
            case.case, case.status, answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            case.body,
            "case {:?}: expected body {:?}, got {:?}",
            case.case,
            case.body,
            answered.body
        );
        if let Some((name, value)) = case.response_header {
            assert_eq!(
                answered.header(name),
                Some(value),
                "case {:?}: expected the custom response header {name}={value:?}, got {:?}",
                case.case,
                answered.headers
            );
        }
    }
}

/// The rule over how much of a BODY is inspected: inspection stops at the limit the
/// resource type documents, and the field's `OversizeHandling` decides a body that
/// exceeds it.
///
/// One row per handling — `CONTINUE`, `MATCH`, `NO_MATCH` — crossed with a body within
/// the limit and a body past it, with the searched-for fragment placed inside and
/// outside the inspected prefix.
#[tokio::test]
async fn body_inspection_stops_at_the_limit_and_oversize_handling_decides_past_it() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-body").await;

    /// The documented inspection limit for an API Gateway stage: `Body$OversizeHandling`
    /// gives "for CloudFront, API Gateway, Amazon Cognito, App Runner, and Verified
    /// Access, the default limit is 16 KB (16,384 bytes)".
    const LIMIT: usize = 16 * 1024;
    const NEEDLE: &str = "needle";

    for handling in ["CONTINUE", "MATCH", "NO_MATCH"] {
        let stage = handling.to_ascii_lowercase();
        api.deploy(&edge, &stage).await;
        let (_, arn, _) = create_web_acl(
            &edge,
            &format!("body-{stage}"),
            json!({ "Allow": {} }),
            json!([rule(
                "body",
                0,
                json!({
                    "ByteMatchStatement": {
                        "SearchString": base64_of(NEEDLE),
                        "FieldToMatch": { "Body": { "OversizeHandling": handling } },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                        "PositionalConstraint": "CONTAINS",
                    }
                }),
                json!({ "Block": {} }),
            )]),
            json!({}),
        )
        .await;
        associate(&edge, &arn, &api.id, &stage).await;
    }

    /// One body and the status the web ACL of `stage` answers it with.
    struct Case {
        case: &'static str,
        stage: &'static str,
        body: Vec<u8>,
        status: u16,
    }

    /// A body of `len` bytes of filler with `NEEDLE` at `at`, or no needle at all when
    /// `at` is past the end.
    fn body_with_needle(len: usize, at: Option<usize>) -> Vec<u8> {
        body_carrying(len, NEEDLE, at)
    }

    let cases = [
        Case {
            case: "CONTINUE: the fragment sits inside the inspected prefix",
            stage: "continue",
            body: body_with_needle(64, Some(8)),
            status: 403,
        },
        Case {
            case: "CONTINUE: a body within the limit that carries no fragment",
            stage: "continue",
            body: body_with_needle(64, None),
            status: 200,
        },
        Case {
            case: "CONTINUE: an oversize body whose fragment sits PAST the limit, so \
                   inspection never reaches it",
            stage: "continue",
            body: body_with_needle(LIMIT * 2, Some(LIMIT + 32)),
            status: 200,
        },
        Case {
            case: "CONTINUE: an oversize body whose fragment sits inside the prefix",
            stage: "continue",
            body: body_with_needle(LIMIT * 2, Some(16)),
            status: 403,
        },
        Case {
            case: "MATCH: an oversize body with no fragment at all still matches",
            stage: "match",
            body: body_with_needle(LIMIT * 2, None),
            status: 403,
        },
        Case {
            case: "MATCH: a body within the limit is inspected normally",
            stage: "match",
            body: body_with_needle(64, None),
            status: 200,
        },
        Case {
            case: "NO_MATCH: an oversize body whose fragment sits inside the prefix \
                   does not match",
            stage: "no_match",
            body: body_with_needle(LIMIT * 2, Some(16)),
            status: 200,
        },
        Case {
            case: "NO_MATCH: a body within the limit is inspected normally",
            stage: "no_match",
            body: body_with_needle(64, Some(8)),
            status: 403,
        },
    ];

    for case in &cases {
        let answered = edge
            .invoke(
                &api.id,
                "POST",
                &format!("/{}/upload", case.stage),
                &[("content-type", "application/json")],
                &case.body,
            )
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {} for a {}-byte body, got {} with {:?}",
            case.case,
            case.status,
            case.body.len(),
            answered.status,
            answered.body
        );
    }
}

/// The rule over WHERE body inspection stops: WAF inspects a request body only up to
/// the limit in force for the protected resource type — the 16 KB that type documents,
/// or the `SizeInspectionLimit` the web ACL's `AssociationConfig` names for it — the
/// inspected field's `OversizeHandling` decides a body that EXCEEDS that limit, the
/// protected stage answers the request either way, and every request the web ACL
/// decided is in the sample `GetSampledRequests` draws from.
///
/// One stage per limit in force, each driven AT the bound and one step past it: a
/// fragment ending on the last inspected byte is seen, the same fragment starting on the
/// first byte past the prefix is not, and a body one byte over the limit is the oversize
/// one `MATCH` and `NO_MATCH` decide without inspecting it.
#[tokio::test]
async fn body_inspection_stops_at_the_limit_in_force_and_the_stage_answers_either_way() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-body-limit").await;

    /// The fragment every stage's rule hunts for in the body.
    const NEEDLE: &str = "needle";

    /// The limit in force when the web ACL configures none: `Body$OversizeHandling`
    /// gives "for CloudFront, API Gateway, Amazon Cognito, App Runner, and Verified
    /// Access, the default limit is 16 KB (16,384 bytes)".
    const DEFAULT_LIMIT: usize = 16 * 1024;

    /// The rule name — and so the metric dimension a sample of a BLOCKED request is
    /// drawn under — every stage's web ACL carries.
    const RULE: &str = "inspect-body";

    /// One protected stage: the `OversizeHandling` its rule carries, and the
    /// `SizeInspectionLimit` its web ACL configures for `API_GATEWAY` (`None` configures
    /// none, leaving the documented default in force).
    struct Protected {
        stage: &'static str,
        handling: &'static str,
        configured: Option<&'static str>,
    }

    /// The byte count a configured `SizeInspectionLimit` names, and [`DEFAULT_LIMIT`] for a
    /// web ACL that configures none — so the limit a row is driven at is read off the same
    /// enum value the row configures, rather than restated beside it.
    fn inspection_limit(configured: Option<&str>) -> usize {
        match configured {
            None => DEFAULT_LIMIT,
            Some("KB_16") => 16 * 1024,
            Some("KB_32") => 32 * 1024,
            Some("KB_48") => 48 * 1024,
            Some("KB_64") => 64 * 1024,
            Some(other) => panic!(
                "case {other:?}: expected one of the values SizeInspectionLimit declares, \
                 which this table does not name"
            ),
        }
    }

    // Every value `SizeInspectionLimit` declares
    // (`smithy_query(query="shape",shape="SizeInspectionLimit")` → `KB_16`, `KB_32`,
    // `KB_48`, `KB_64`), plus a web ACL that configures none at all, crossed with the
    // three values of `OversizeHandling`.
    let protected = [
        Protected {
            stage: "unset",
            handling: "CONTINUE",
            configured: None,
        },
        Protected {
            stage: "kb16",
            handling: "CONTINUE",
            configured: Some("KB_16"),
        },
        Protected {
            stage: "kb32",
            handling: "CONTINUE",
            configured: Some("KB_32"),
        },
        Protected {
            stage: "kb48",
            handling: "CONTINUE",
            configured: Some("KB_48"),
        },
        Protected {
            stage: "kb64",
            handling: "CONTINUE",
            configured: Some("KB_64"),
        },
        Protected {
            stage: "matched",
            handling: "MATCH",
            configured: Some("KB_32"),
        },
        Protected {
            stage: "unmatched",
            handling: "NO_MATCH",
            configured: Some("KB_48"),
        },
    ];

    let mut web_acls: Vec<String> = Vec::new();
    for entry in &protected {
        api.deploy(&edge, entry.stage).await;
        let extra = match entry.configured {
            Some(limit) => json!({
                "AssociationConfig": {
                    "RequestBody": { "API_GATEWAY": { "DefaultSizeInspectionLimit": limit } }
                }
            }),
            None => json!({}),
        };
        let (_, arn, _) = create_web_acl(
            &edge,
            &format!("limit-{}", entry.stage),
            json!({ "Allow": {} }),
            json!([rule(
                RULE,
                0,
                json!({
                    "ByteMatchStatement": {
                        "SearchString": base64_of(NEEDLE),
                        "FieldToMatch": { "Body": { "OversizeHandling": entry.handling } },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                        "PositionalConstraint": "CONTAINS",
                    }
                }),
                json!({ "Block": {} }),
            )]),
            extra,
        )
        .await;
        associate(&edge, &arn, &api.id, entry.stage).await;
        web_acls.push(arn);
    }

    /// One request to a protected stage and the status its web ACL answers it with.
    struct Case {
        case: String,
        stage: &'static str,
        body: Vec<u8>,
        status: u16,
    }

    let mut cases: Vec<Case> = Vec::new();
    for entry in &protected {
        let (stage, limit) = (entry.stage, inspection_limit(entry.configured));
        let body_with_needle = |len: usize, at: Option<usize>| body_carrying(len, NEEDLE, at);
        match entry.handling {
            // `CONTINUE` — "inspect the available body contents normally, according to
            // the rule inspection criteria": the prefix the limit allows is inspected
            // and nothing past it is.
            "CONTINUE" => cases.extend([
                Case {
                    case: format!(
                        "{stage}: the fragment ENDS on the last inspected byte of a body \
                         at the {limit}-byte limit"
                    ),
                    stage,
                    body: body_with_needle(limit, Some(limit - NEEDLE.len())),
                    status: 403,
                },
                Case {
                    case: format!(
                        "{stage}: the fragment STARTS on the first byte past the \
                         {limit}-byte prefix"
                    ),
                    stage,
                    body: body_with_needle(limit + NEEDLE.len(), Some(limit)),
                    status: 200,
                },
                Case {
                    case: format!(
                        "{stage}: an oversize body whose fragment sits inside the \
                         {limit}-byte prefix"
                    ),
                    stage,
                    body: body_with_needle(limit + 4096, Some(0)),
                    status: 403,
                },
            ]),
            // `MATCH` — "treat the web request as matching the rule statement", which
            // only applies ONCE THE BODY IS OVER the limit: a body at the limit is
            // inspected normally and so carries no fragment to match.
            "MATCH" => cases.extend([
                Case {
                    case: format!(
                        "{stage}: a fragment-free body AT the {limit}-byte limit is not \
                         oversize, so nothing matches"
                    ),
                    stage,
                    body: body_with_needle(limit, None),
                    status: 200,
                },
                Case {
                    case: format!(
                        "{stage}: a fragment-free body ONE byte past the {limit}-byte \
                         limit is oversize, so the statement matches"
                    ),
                    stage,
                    body: body_with_needle(limit + 1, None),
                    status: 403,
                },
            ]),
            // `NO_MATCH` — "treat the web request as not matching the rule statement",
            // again only past the limit: the SAME fragment inside the prefix decides the
            // request one way at the limit and the other way one byte over it.
            _ => cases.extend([
                Case {
                    case: format!(
                        "{stage}: the fragment inside a body AT the {limit}-byte limit is \
                         inspected normally"
                    ),
                    stage,
                    body: body_with_needle(limit, Some(0)),
                    status: 403,
                },
                Case {
                    case: format!(
                        "{stage}: the fragment inside a body ONE byte past the \
                         {limit}-byte limit does not match"
                    ),
                    stage,
                    body: body_with_needle(limit + 1, Some(0)),
                    status: 200,
                },
            ]),
        }
    }

    for case in &cases {
        let answered = edge
            .invoke(
                &api.id,
                "POST",
                &format!("/{}/upload", case.stage),
                &[("content-type", "application/json")],
                &case.body,
            )
            .await;
        assert_eq!(
            answered.status,
            case.status,
            "case {:?}: expected status {} for a {}-byte body, got {} with {:?}",
            case.case,
            case.status,
            case.body.len(),
            answered.status,
            answered.body
        );
    }

    /// The samples WAF holds for the `metric` dimension of `web_acl_arn`.
    async fn samples(edge: &Edge, web_acl_arn: &str, metric: &str) -> Vec<Value> {
        let answered = edge
            .waf_ok(
                "GetSampledRequests",
                json!({
                    "WebAclArn": web_acl_arn,
                    "RuleMetricName": metric,
                    "Scope": SCOPE,
                    "MaxItems": 100,
                    "TimeWindow": sampling_window(),
                }),
            )
            .await;
        answered["SampledRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    // Every request the web ACL decided — at the limit and past it alike — is in the
    // population a sample is drawn from, under the dimension that ACTED on it: the rule's
    // own for one it blocked, the web ACL's for one the `DefaultAction` let through.
    for (entry, arn) in protected.iter().zip(web_acls.iter()) {
        let stage = entry.stage;
        let for_stage = |status: u16| {
            cases
                .iter()
                .filter(|case| case.stage == stage && case.status == status)
                .count()
        };
        let uri = format!("/{stage}/upload");

        let blocked = samples(&edge, arn, RULE).await;
        assert_eq!(
            blocked.len(),
            for_stage(403),
            "case {stage:?}: expected one sample of the {RULE} dimension per refused \
             request, got {blocked:?}"
        );
        for sample in &blocked {
            assert_eq!(
                sample["Action"].as_str(),
                Some("BLOCK"),
                "case {stage:?}: expected the refused request sampled as BLOCK, got {sample:?}"
            );
            assert_eq!(
                sample["Request"]["Method"].as_str(),
                Some("POST"),
                "case {stage:?}: expected the sampled request's method POST, got {sample:?}"
            );
            assert_eq!(
                sample["Request"]["URI"].as_str(),
                Some(uri.as_str()),
                "case {stage:?}: expected the sampled request's URI {uri:?}, got {sample:?}"
            );
        }

        let allowed = samples(&edge, arn, &format!("limit-{stage}")).await;
        assert_eq!(
            allowed.len(),
            for_stage(200),
            "case {stage:?}: expected one sample of the web ACL's own dimension per \
             request the default action let through, got {allowed:?}"
        );
        for sample in &allowed {
            assert_eq!(
                sample["Action"].as_str(),
                Some("ALLOW"),
                "case {stage:?}: expected the request the default action let through \
                 sampled as ALLOW, got {sample:?}"
            );
            assert_eq!(
                sample["Request"]["URI"].as_str(),
                Some(uri.as_str()),
                "case {stage:?}: expected the sampled request's URI {uri:?}, got {sample:?}"
            );
        }
    }
}

/// The rule over an IP-set reference: with an `IPSetForwardedIPConfig` the address
/// judged is the one at the configured POSITION of the named header, and the
/// `FallbackBehavior` decides a request that carries no address there.
///
/// One row per position (`FIRST`, `LAST`, `ANY`) crossed with where in the header the
/// listed address sits, plus a request carrying no such header at all.
#[tokio::test]
async fn an_ip_set_reference_judges_the_address_at_the_configured_header_position() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-forwarded").await;

    /// The one address the IP set holds; every row differs in WHERE it sits.
    const LISTED: &str = "198.51.100.9";
    const OTHER: &str = "203.0.113.7";

    let ip_set = edge
        .waf_ok(
            "CreateIPSet",
            json!({
                "Name": "listed",
                "Scope": SCOPE,
                "IPAddressVersion": "IPV4",
                "Addresses": [format!("{LISTED}/32")],
            }),
        )
        .await;
    let ip_set_arn = ip_set["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();

    for position in ["FIRST", "LAST", "ANY"] {
        let stage = position.to_ascii_lowercase();
        api.deploy(&edge, &stage).await;
        let (_, arn, _) = create_web_acl(
            &edge,
            &format!("forwarded-{stage}"),
            json!({ "Allow": {} }),
            json!([rule(
                "forwarded",
                0,
                json!({
                    "IPSetReferenceStatement": {
                        "ARN": ip_set_arn,
                        "IPSetForwardedIPConfig": {
                            "HeaderName": "X-Forwarded-For",
                            "FallbackBehavior": "NO_MATCH",
                            "Position": position,
                        }
                    }
                }),
                json!({ "Block": {} }),
            )]),
            json!({}),
        )
        .await;
        associate(&edge, &arn, &api.id, &stage).await;
    }

    /// One forwarded-for header and the status each position answers it with.
    struct Case {
        case: &'static str,
        stage: &'static str,
        forwarded: Option<String>,
        status: u16,
    }

    let cases = [
        Case {
            case: "FIRST: the listed address is first in the header",
            stage: "first",
            forwarded: Some(format!("{LISTED}, {OTHER}")),
            status: 403,
        },
        Case {
            case: "FIRST: the listed address is last in the header",
            stage: "first",
            forwarded: Some(format!("{OTHER}, {LISTED}")),
            status: 200,
        },
        Case {
            case: "LAST: the listed address is last in the header",
            stage: "last",
            forwarded: Some(format!("{OTHER}, {LISTED}")),
            status: 403,
        },
        Case {
            case: "LAST: the listed address is first in the header",
            stage: "last",
            forwarded: Some(format!("{LISTED}, {OTHER}")),
            status: 200,
        },
        Case {
            case: "ANY: the listed address sits in the middle of the header",
            stage: "any",
            forwarded: Some(format!("{OTHER}, {LISTED}, {OTHER}")),
            status: 403,
        },
        Case {
            case: "ANY: the header lists no address of the set",
            stage: "any",
            forwarded: Some(format!("{OTHER}, {OTHER}")),
            status: 200,
        },
        Case {
            case: "NO_MATCH fallback: the request carries no forwarded-for header",
            stage: "last",
            forwarded: None,
            status: 200,
        },
    ];

    for case in &cases {
        let headers: Vec<(&str, &str)> = case
            .forwarded
            .as_deref()
            .map(|value| vec![("x-forwarded-for", value)])
            .unwrap_or_default();
        let answered = edge
            .invoke(
                &api.id,
                "GET",
                &format!("/{}/orders", case.stage),
                &headers,
                &[],
            )
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with {:?}",
            case.case, case.status, answered.status, answered.body
        );
    }
}

/// The rule over what a forwarded-IP block ANSWERS: the refusal is generated at the
/// edge, so the origin's own body is absent from it, and the `FallbackBehavior` decides
/// a request whose configured position holds no valid address — the position is a
/// position in the header as the client wrote it, so an address behind a non-address
/// entry is not judged in its place.
///
/// One row per fallback behavior crossed with where the listed address sits, what the
/// entry at the position is, and whether the header is there at all; each row asserts
/// the status AND whether the origin's body came back, which is what says the
/// integration ran.
#[tokio::test]
async fn a_forwarded_ip_block_is_answered_at_the_edge_without_the_origin_body() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-forwarded-fallback").await;

    /// The one address the IP set holds.
    const LISTED: &str = "198.51.100.7";
    const OTHER: &str = "203.0.113.9";

    let ip_set = edge
        .waf_ok(
            "CreateIPSet",
            json!({
                "Name": "listed-fallback",
                "Scope": SCOPE,
                "IPAddressVersion": "IPV4",
                "Addresses": [format!("{LISTED}/32")],
            }),
        )
        .await;
    let ip_set_arn = ip_set["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();

    // One stage per fallback behavior, each with the same `FIRST` rule in front of it.
    for fallback in ["NO_MATCH", "MATCH"] {
        let stage = fallback.to_ascii_lowercase();
        api.deploy(&edge, &stage).await;
        let (_, arn, _) = create_web_acl(
            &edge,
            &format!("fallback-{stage}"),
            json!({ "Allow": {} }),
            json!([rule(
                "forwarded",
                0,
                json!({
                    "IPSetReferenceStatement": {
                        "ARN": ip_set_arn,
                        "IPSetForwardedIPConfig": {
                            "HeaderName": "X-Forwarded-For",
                            "FallbackBehavior": fallback,
                            "Position": "FIRST",
                        }
                    }
                }),
                json!({ "Block": {} }),
            )]),
            json!({}),
        )
        .await;
        associate(&edge, &arn, &api.id, &stage).await;
    }

    /// One request, the status it must be answered with, and whether the origin's body
    /// must be part of that answer.
    struct Case {
        case: &'static str,
        stage: &'static str,
        forwarded: Option<String>,
        status: u16,
        from_origin: bool,
    }

    let cases = [
        Case {
            case: "NO_MATCH: the listed address heads the chain",
            stage: "no_match",
            forwarded: Some(format!("{LISTED}, {OTHER}")),
            status: 403,
            from_origin: false,
        },
        Case {
            case: "NO_MATCH: the listed address sits second in the chain",
            stage: "no_match",
            forwarded: Some(format!("{OTHER}, {LISTED}")),
            status: 200,
            from_origin: true,
        },
        Case {
            case: "NO_MATCH: the request carries no forwarded-for header",
            stage: "no_match",
            forwarded: None,
            status: 200,
            from_origin: true,
        },
        Case {
            case: "NO_MATCH: the entry at the position is not an address, a listed one \
                   follows it",
            stage: "no_match",
            forwarded: Some(format!("unknown, {LISTED}")),
            status: 200,
            from_origin: true,
        },
        Case {
            case: "MATCH: the request carries no forwarded-for header",
            stage: "match",
            forwarded: None,
            status: 403,
            from_origin: false,
        },
        Case {
            case: "MATCH: the entry at the position is not an address",
            stage: "match",
            forwarded: Some(format!("unknown, {OTHER}")),
            status: 403,
            from_origin: false,
        },
        Case {
            case: "MATCH: the position holds an address outside the set",
            stage: "match",
            forwarded: Some(OTHER.to_string()),
            status: 200,
            from_origin: true,
        },
    ];

    for case in &cases {
        let headers: Vec<(&str, &str)> = case
            .forwarded
            .as_deref()
            .map(|value| vec![("x-forwarded-for", value)])
            .unwrap_or_default();
        let answered = edge
            .invoke(
                &api.id,
                "GET",
                &format!("/{}/orders", case.stage),
                &headers,
                &[],
            )
            .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with {:?}",
            case.case, case.status, answered.status, answered.body
        );
        let carries_origin = answered.body.contains(&origin_body("orders"));
        assert_eq!(
            carries_origin, case.from_origin,
            "case {:?}: expected the origin's body present={}, got {:?}",
            case.case, case.from_origin, answered.body
        );
    }
}

/// The rule over a MANAGED rule group: its rules run against the request, a
/// `ScopeDownStatement` limits WHICH requests reach them, and the sample WAF stores
/// names the rule within the group that decided.
///
/// One row per payload — an injected SQL fragment in a query argument, in the path, and
/// a benign value — crossed with a web ACL that scopes the group down to a URI prefix
/// and one that does not.
#[tokio::test]
async fn a_managed_rule_group_decides_and_the_sample_names_the_rule_that_did() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-managed").await;
    api.deploy(&edge, "sqli").await;
    api.deploy(&edge, "scoped").await;

    let managed = |scope_down: Option<Value>| {
        let mut statement = json!({
            "VendorName": "AWS",
            "Name": "AWSManagedRulesSQLiRuleSet",
        });
        if let Some(scope_down) = scope_down {
            statement["ScopeDownStatement"] = scope_down;
        }
        json!({ "ManagedRuleGroupStatement": statement })
    };

    // A rule-group rule carries no `Action` of its own: each of the group's rules does,
    // and the web ACL rule says only whether to override them all.
    let group_rule = |name: &str, statement: Value| {
        json!({
            "Name": name,
            "Priority": 0,
            "Statement": statement,
            "OverrideAction": { "None": {} },
            "VisibilityConfig": visibility(name),
        })
    };

    let (sqli_id, sqli_arn, _) = create_web_acl(
        &edge,
        "sqli-everywhere",
        json!({ "Allow": {} }),
        json!([group_rule("sql-database", managed(None))]),
        json!({}),
    )
    .await;
    associate(&edge, &sqli_arn, &api.id, "sqli").await;

    let (_, scoped_arn, _) = create_web_acl(
        &edge,
        "sqli-under-admin",
        json!({ "Allow": {} }),
        json!([group_rule(
            "sql-database",
            managed(Some(byte_match(
                json!({ "UriPath": {} }),
                "/scoped/admin",
                "STARTS_WITH",
            ))),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &scoped_arn, &api.id, "scoped").await;

    /// One request and the status the managed group earns it.
    struct Case {
        case: &'static str,
        path: &'static str,
        status: u16,
    }

    let cases = [
        Case {
            case: "an injected SQL fragment in a query argument",
            path: "/sqli/orders?q=%27%20OR%201%3D1%20--",
            status: 403,
        },
        Case {
            case: "a benign query argument",
            path: "/sqli/orders?q=alice",
            status: 200,
        },
        Case {
            case: "scoped down to /admin: the fragment on a path inside the scope",
            path: "/scoped/admin?q=%27%20OR%201%3D1%20--",
            status: 403,
        },
        Case {
            case: "scoped down to /admin: the same fragment OUTSIDE the scope, so the \
                   group never runs",
            path: "/scoped/orders?q=%27%20OR%201%3D1%20--",
            status: 200,
        },
    ];

    for case in &cases {
        let answered = edge.get(&api.id, case.path).await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected status {}, got {} with {:?}",
            case.case, case.status, answered.status, answered.body
        );
    }

    // The sample WAF stored for the web ACL rule that ran the group names the rule
    // WITHIN the group that decided, which is what tells an operator which managed
    // rule refused the request.
    let sampled = edge
        .waf_ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": sqli_arn,
                "RuleMetricName": "sql-database",
                "Scope": SCOPE,
                "MaxItems": 50,
                "TimeWindow": sampling_window(),
            }),
        )
        .await;
    let samples = sampled["SampledRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let blocked: Vec<&Value> = samples
        .iter()
        .filter(|sample| sample["Action"].as_str() == Some("BLOCK"))
        .collect();
    assert!(
        !blocked.is_empty(),
        "case the sample of the rule that blocked: expected at least one BLOCK sample \
         for web acl {sqli_id}, got {sampled}"
    );
    for sample in &blocked {
        let named = sample["RuleNameWithinRuleGroup"].as_str().unwrap_or_default();
        // `SampledHTTPRequest$RuleNameWithinRuleGroup`: "For managed rule groups, the
        // format for this name is <vendor name>#<managed rule group name>#<rule
        // name>" — so the terminating rule is named by the group that holds it, and
        // the group-local SQLi rule is the last segment.
        let segments: Vec<&str> = named.split('#').collect();
        assert!(
            segments.len() == 3
                && segments[0] == "AWS"
                && segments[1] == "AWSManagedRulesSQLiRuleSet"
                && segments[2].starts_with("SQLi"),
            "case the sample of the rule that blocked: expected the group's own SQLi \
             rule named AWS#AWSManagedRulesSQLiRuleSet#<rule>, got {named:?} in {sample}"
        );
    }
}

/// The rule over a RATE-BASED rule: requests are counted per aggregation instance over
/// the rule's evaluation window, WAF checks each instance's rate periodically — "WAF
/// checks the rate about every 10 seconds"
/// (`RateBasedStatement$EvaluationWindowSec`) — and the rule applies its action to the
/// requests it decides while the rate its last check found is OVER the `Limit`. So a
/// burst sent inside one check interval is served to its end, and the keys a check found
/// over the limit are what `GetRateBasedStatementManagedKeys` reports.
#[tokio::test]
async fn a_rate_based_rule_refuses_the_caller_past_its_limit_and_lists_its_key() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-rate").await;
    api.deploy(&edge, "rated").await;

    /// `RateBasedStatement$Limit` is modelled `range(min 10)`, so the smallest limit a
    /// rule can carry is the one this case drives.
    const LIMIT: usize = 10;

    let (web_acl_id, arn, _) = create_web_acl(
        &edge,
        "rate-limited",
        json!({ "Allow": {} }),
        json!([rule(
            "by-ip",
            0,
            json!({
                "RateBasedStatement": {
                    "Limit": LIMIT,
                    "AggregateKeyType": "IP",
                }
            }),
            json!({ "Block": {} }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &arn, &api.id, "rated").await;

    // A burst sent inside ONE check interval: the rate in force is the one the check at
    // its first request arrived at, so every request of it is served — the requests past
    // the limit are what the NEXT check finds.
    for sent in 1..=LIMIT + 2 {
        let answered = edge.get(&api.id, "/rated/orders").await;
        assert_eq!(
            answered.status, 200,
            "case request {sent} of a burst inside one check interval at a limit of \
             {LIMIT}: expected status 200, got {} with {:?}",
            answered.status, answered.body
        );
    }

    // Once the next check falls due it finds the caller over the limit, and from there
    // the rule acts on every request of the same aggregation instance.
    edge.due_rate_check();
    for sent in 1..=2 {
        let answered = edge.get(&api.id, "/rated/orders").await;
        assert_eq!(
            answered.status, 403,
            "case request {sent} past the check that found the caller over the limit of \
             {LIMIT}: expected status 403, got {} with {:?}",
            answered.status, answered.body
        );
    }

    let keys = edge
        .waf_ok(
            "GetRateBasedStatementManagedKeys",
            json!({
                "Scope": SCOPE,
                "WebACLName": "rate-limited",
                "WebACLId": web_acl_id,
                "RuleName": "by-ip",
            }),
        )
        .await;
    let listed = keys["ManagedKeysIPV4"]["Addresses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case the keys currently over the limit: expected exactly the one caller's \
         address, got {keys}"
    );
    let address = listed[0].as_str().unwrap_or_default();
    assert!(
        address.ends_with("/32"),
        "case the keys currently over the limit: an IPv4 managed key is reported in CIDR \
         notation, got {address:?}"
    );
}

/// The rule over the control-plane REFUSALS a live edge is asserted against: a web ACL
/// that is still associated, a pattern set a rule still references and an update
/// carrying a stale lock token are each refused, and the edge keeps deciding exactly
/// as it did before the refused call.
#[tokio::test]
async fn refused_control_plane_calls_leave_the_edge_deciding_as_it_did() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-control-plane").await;
    api.deploy(&edge, "live").await;

    // A pattern set the web ACL's rule references, so deleting it has to be refused
    // while the rule is live.
    let pattern_set = edge
        .waf_ok(
            "CreateRegexPatternSet",
            json!({
                "Name": "blocked-paths",
                "Scope": SCOPE,
                "RegularExpressionList": [{ "RegexString": "/orders$" }],
            }),
        )
        .await;
    let pattern_arn = pattern_set["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    let pattern_id = pattern_set["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let pattern_lock = pattern_set["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    let (web_acl_id, web_acl_arn, lock_token) = create_web_acl(
        &edge,
        "referencing",
        json!({ "Allow": {} }),
        json!([rule(
            "by-pattern",
            0,
            json!({
                "RegexPatternSetReferenceStatement": {
                    "ARN": pattern_arn,
                    "FieldToMatch": { "UriPath": {} },
                    "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                }
            }),
            json!({ "Block": {} }),
        )]),
        json!({}),
    )
    .await;
    associate(&edge, &web_acl_arn, &api.id, "live").await;

    /// What the edge decides, before and after each refused call: the blocked path and
    /// the allowed one.
    async fn edge_decides(edge: &Edge, api: &str, case: &str) {
        for (path, status) in [("/live/orders", 403), ("/live/admin", 200)] {
            let answered = edge.get(api, path).await;
            assert_eq!(
                answered.status, status,
                "case {case:?}: expected the edge to answer {path} with {status}, got {} \
                 with {:?}",
                answered.status, answered.body
            );
        }
    }

    edge_decides(&edge, &api.id, "before any refused call").await;

    /// One refused control-plane call: the operation, its body and the error code.
    struct Refusal {
        case: &'static str,
        operation: &'static str,
        body: Value,
        code: &'static str,
    }

    let refusals = [
        Refusal {
            case: "deleting a web acl that is still associated with a stage",
            operation: "DeleteWebACL",
            body: json!({
                "Name": "referencing",
                "Scope": SCOPE,
                "Id": web_acl_id,
                "LockToken": lock_token,
            }),
            code: "WAFAssociatedItemException",
        },
        Refusal {
            case: "deleting a pattern set a live rule still references",
            operation: "DeleteRegexPatternSet",
            body: json!({
                "Name": "blocked-paths",
                "Scope": SCOPE,
                "Id": pattern_id,
                "LockToken": pattern_lock,
            }),
            code: "WAFAssociatedItemException",
        },
        Refusal {
            case: "updating the web acl with a stale lock token",
            operation: "UpdateWebACL",
            body: json!({
                "Name": "referencing",
                "Scope": SCOPE,
                "Id": web_acl_id,
                "LockToken": "00000000-0000-4000-8000-000000000000",
                "DefaultAction": { "Block": {} },
                "VisibilityConfig": visibility("referencing"),
                "Rules": [],
            }),
            code: "WAFOptimisticLockException",
        },
    ];

    for refusal in &refusals {
        let (status, parsed) = edge.waf_call(refusal.operation, refusal.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 {}, got {status} with {parsed}",
            refusal.case, refusal.code
        );
        let reported = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            reported.ends_with(refusal.code),
            "case {:?}: expected the error code {}, got {reported:?} in {parsed}",
            refusal.case,
            refusal.code
        );
        edge_decides(&edge, &api.id, refusal.case).await;
    }
}

/// The prefix WAF puts on every header a `CustomRequestHandling` inserts.
///
/// `CustomHTTPHeader$Name`: "when WAF inserts the header into the request, it prefixes
/// this name `x-amzn-waf-`, to avoid confusion with the headers that are already in the
/// request. For example, for the header name `sample`, WAF inserts the header
/// `x-amzn-waf-sample`."
const INSERTED_HEADER_PREFIX: &str = "x-amzn-waf-";

/// The header whose value the rules below inspect, so one request matches a counting rule
/// and another does not.
const TRIGGER_HEADER: &str = "x-waf-trigger";

/// The name WAF inserts the configured header `name` under.
fn inserted_as(name: &str) -> String {
    format!("{INSERTED_HEADER_PREFIX}{name}")
}

/// A `CustomRequestHandling$InsertHeaders` list from `(configured name, value)` pairs.
fn insert_headers(headers: &[(String, String)]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// The members the echoing resource's template wrote: the value the ORIGIN saw under each
/// header name it reflects, with the empty string for a name the request did not carry.
fn echoed_headers(answered: &Answered, case: &str) -> std::collections::BTreeMap<String, String> {
    let parsed: Value = serde_json::from_str(&answered.body).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the origin's echo must be the JSON document its response \
             template renders, got {:?} ({error})",
            answered.body
        )
    });
    parsed
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(name, value)| (name, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// The rule over a NON-TERMINATING action's `CustomRequestHandling`: a rule whose action
/// COUNTS the request applies the custom request handling it configures — every header in
/// `InsertHeaders` is inserted into the web request under its configured name with
/// `x-amzn-waf-` prefixed and its value verbatim, never under the bare configured name —
/// and then lets the evaluation continue, so the request reaches the origin carrying those
/// headers and keeps the status the stage answers it with. A request the rule does not
/// match carries neither form. `GetSampledRequests` reports the same insertion on the
/// rule's own metric dimension, under the action that was applied to the request.
///
/// One row per thing the rule generalises over: how many headers one action configures,
/// what the configured name looks like (a name that already reads as a header name, a bare
/// word, a mixed-case name, a dotted one) and how long the name and the value are, at each
/// bound `CustomHTTPHeader` publishes. Each row gets its own stage and its own web ACL, so
/// the only thing deciding the row is that web ACL's one counting rule, and each row is
/// driven three times — the request the rule matches, a request carrying the inspected
/// header with another value, and a request carrying none of it.
#[tokio::test]
async fn a_counted_rules_custom_request_handling_reaches_the_origin_and_the_sample() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-count-insert").await;

    /// One counting rule's custom request handling, and the stage it decides.
    struct Row {
        case: &'static str,
        /// The stage this row's web ACL is associated with.
        stage: &'static str,
        /// The value of [`TRIGGER_HEADER`] the rule's `ByteMatchStatement` matches
        /// EXACTLY.
        trigger: &'static str,
        /// The headers the rule's `Count` action configures, by CONFIGURED name — the
        /// name before WAF prefixes it.
        insert: Vec<(String, String)>,
    }

    // The names and values below span what `CustomHTTPHeader` admits:
    // `CustomHTTPHeader$Name` is modelled `length(min 1, max 64)` with pattern
    // `^[a-zA-Z0-9._$-]+$`, and `CustomHTTPHeader$Value` `length(min 1, max 255)` with
    // pattern `.*` — so the rows carry a one-glyph name and a 64-glyph one, a one-glyph
    // value and a 255-glyph one, a dot inside a name and punctuation inside a value.
    let longest_name = "n".repeat(64);
    let longest_value = "v".repeat(255);
    let rows = vec![
        Row {
            case: "one header whose configured name already reads as a header name",
            stage: "recorded",
            trigger: "trip-recorded",
            insert: vec![("x-waf-inserted".to_string(), "counted-1".to_string())],
        },
        Row {
            case: "two headers at once: the whole list the action configures is inserted",
            stage: "pair",
            trigger: "trip-pair",
            insert: vec![
                ("tenant".to_string(), "acme".to_string()),
                ("score.v2".to_string(), "9".to_string()),
            ],
        },
        Row {
            case: "a mixed-case configured name and a punctuated value",
            stage: "mixedcase",
            trigger: "trip-mixed",
            insert: vec![("Rule-Match".to_string(), "yes:maybe=true".to_string())],
        },
        Row {
            case: "the shortest name and the longest value the member allows",
            stage: "shortest",
            trigger: "trip-short",
            insert: vec![("c".to_string(), longest_value.clone())],
        },
        Row {
            case: "the longest name and the shortest value the member allows",
            stage: "longest",
            trigger: "trip-long",
            insert: vec![(longest_name.clone(), "x".to_string())],
        },
    ];

    /// The stage whose web ACL counts and THEN blocks the same request.
    const BLOCKED_STAGE: &str = "later-block";
    /// The value the counting rule of [`BLOCKED_STAGE`] matches.
    const BLOCKED_TRIGGER: &str = "trip-blocked";
    /// The rule name — and so the metric dimension `GetSampledRequests` is asked for —
    /// of the counting rule in every web ACL below.
    const COUNTING_RULE: &str = "inserter";

    // The origin's own view: one resource reflecting BOTH forms of every configured name
    // any row inserts, plus the header the rules inspect. A name a row's rule did not
    // insert is echoed as the empty string, which is how the absent form is asserted.
    let mut echoed: Vec<String> = vec![TRIGGER_HEADER.to_string()];
    for row in &rows {
        for (name, _) in &row.insert {
            echoed.push(inserted_as(name));
            echoed.push(name.clone());
        }
    }
    api.echoing_resource(&edge, "echo", &echoed).await;

    for stage in rows
        .iter()
        .map(|row| row.stage)
        .chain(std::iter::once(BLOCKED_STAGE))
    {
        api.deploy(&edge, stage).await;
    }

    /// The counting rule of a row: it inspects [`TRIGGER_HEADER`] and, on a match,
    /// counts the request with the custom request handling the row configures.
    fn counting_rule(trigger: &str, insert: &[(String, String)]) -> Value {
        rule(
            COUNTING_RULE,
            0,
            byte_match(
                json!({ "SingleHeader": { "Name": TRIGGER_HEADER } }),
                trigger,
                "EXACTLY",
            ),
            json!({
                "Count": {
                    "CustomRequestHandling": { "InsertHeaders": insert_headers(insert) }
                }
            }),
        )
    }

    for row in &rows {
        let (_, web_acl_arn, _) = create_web_acl(
            &edge,
            row.stage,
            json!({ "Allow": {} }),
            json!([counting_rule(row.trigger, &row.insert)]),
            json!({}),
        )
        .await;
        associate(&edge, &web_acl_arn, &api.id, row.stage).await;

        let path = format!("/{}/echo", row.stage);

        // The request the counting rule matches: `Count` "enables WAF to count requests
        // that match the rule, without otherwise affecting the request", so the stage
        // still serves it, and the origin sees every configured header under its
        // prefixed name with its value verbatim.
        let matched = edge
            .invoke(
                &api.id,
                "GET",
                &path,
                &[(TRIGGER_HEADER, row.trigger)],
                &[],
            )
            .await;
        assert_eq!(
            matched.status, 200,
            "case {:?}: a counted request is not settled by the count, so the stage \
             answers it 200, got {} with {:?}",
            row.case, matched.status, matched.body
        );
        let seen = echoed_headers(&matched, row.case);
        for (name, value) in &row.insert {
            let prefixed = inserted_as(name);
            assert_eq!(
                seen.get(&prefixed).map(String::as_str),
                Some(value.as_str()),
                "case {:?}: the origin must see the configured header under {prefixed:?} \
                 with its value verbatim, got {:?}",
                row.case,
                seen.get(&prefixed)
            );
            assert_eq!(
                seen.get(name).map(String::as_str),
                Some(""),
                "case {:?}: the BARE configured name {name:?} is never what WAF inserts, \
                 got {:?}",
                row.case,
                seen.get(name)
            );
        }
        assert_eq!(
            seen.get(TRIGGER_HEADER).map(String::as_str),
            Some(row.trigger),
            "case {:?}: the header the client sent reaches the origin unchanged, got {:?}",
            row.case,
            seen.get(TRIGGER_HEADER)
        );

        // The same stage for requests the rule does NOT match: neither form of any
        // configured name is inserted.
        for (control, headers) in [
            (
                "the inspected header carrying another value",
                vec![(TRIGGER_HEADER, "not-the-trigger")],
            ),
            ("no inspected header at all", Vec::new()),
        ] {
            let answered = edge.invoke(&api.id, "GET", &path, &headers, &[]).await;
            assert_eq!(
                answered.status, 200,
                "case {:?} with {control}: expected 200, got {} with {:?}",
                row.case, answered.status, answered.body
            );
            let seen = echoed_headers(&answered, row.case);
            for (name, _) in &row.insert {
                for form in [inserted_as(name), name.clone()] {
                    assert_eq!(
                        seen.get(&form).map(String::as_str),
                        Some(""),
                        "case {:?} with {control}: a request the rule does not match \
                         carries no {form:?}, got {:?}",
                        row.case,
                        seen.get(&form)
                    );
                }
            }
        }

        // WAF's own account of the same insertion. The population of the counting rule's
        // dimension is the one request it matched — the two control requests belong to no
        // dimension of it — and `SampledHTTPRequest$RequestHeadersInserted` is "custom
        // request headers inserted by WAF into the request, according to the custom
        // request configuration for the matching rule action".
        let sampled = edge
            .waf_ok(
                "GetSampledRequests",
                json!({
                    "WebAclArn": web_acl_arn,
                    "RuleMetricName": COUNTING_RULE,
                    "Scope": SCOPE,
                    "MaxItems": 500,
                    "TimeWindow": sampling_window(),
                }),
            )
            .await;
        let samples = sampled["SampledRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            samples.len(),
            1,
            "case {:?}: the counting rule's dimension holds exactly the one request it \
             matched, got {sampled}",
            row.case
        );
        let sample = &samples[0];
        assert_eq!(
            sample["Action"].as_str(),
            Some("COUNT"),
            "case {:?}: the action WAF applied to the sampled request is the count, got \
             {sample}",
            row.case
        );
        let reported: Vec<(String, String)> = sample["RequestHeadersInserted"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|header| {
                (
                    header["Name"].as_str().unwrap_or_default().to_string(),
                    header["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = row
            .insert
            .iter()
            .map(|(name, value)| (inserted_as(name), value.clone()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {:?}: the sample must report exactly the headers WAF inserted, under \
             the names it inserted them with, got {sample}",
            row.case
        );
    }

    // The prefix is what lets the origin TRUST that the header came from WAF: a client
    // that sends the prefixed name itself has its value REPLACED by the one the rule's
    // action configures, rather than added to.
    let spoofing = rows
        .iter()
        .find(|row| row.stage == "pair")
        .expect("the row whose action inserts a tenant header");
    let (spoofed_name, spoofed_value) = &spoofing.insert[0];
    let answered = edge
        .invoke(
            &api.id,
            "GET",
            &format!("/{}/echo", spoofing.stage),
            &[
                (TRIGGER_HEADER, spoofing.trigger),
                ("x-amzn-waf-tenant", "sent-by-the-client"),
            ],
            &[],
        )
        .await;
    let case = "a client sending the prefixed name itself";
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected 200, got {} with {:?}",
        answered.status, answered.body
    );
    let seen = echoed_headers(&answered, case);
    let prefixed = inserted_as(spoofed_name);
    assert_eq!(
        seen.get(&prefixed).map(String::as_str),
        Some(spoofed_value.as_str()),
        "case {case:?}: the origin sees the value the rule's action configures, not the \
         one the client sent, got {:?}",
        seen.get(&prefixed)
    );

    // A COUNT does not settle the request: the rules after it still run. The same
    // counting rule, followed by one that BLOCKS the same request, is answered by the
    // block — and the request never reaches an origin to carry the inserted headers to.
    let blocked_insert = vec![("tenant".to_string(), "acme".to_string())];
    let (_, blocked_arn, _) = create_web_acl(
        &edge,
        BLOCKED_STAGE,
        json!({ "Allow": {} }),
        json!([
            counting_rule(BLOCKED_TRIGGER, &blocked_insert),
            rule(
                "after-the-count",
                1,
                byte_match(
                    json!({ "SingleHeader": { "Name": TRIGGER_HEADER } }),
                    BLOCKED_TRIGGER,
                    "EXACTLY",
                ),
                json!({ "Block": {} }),
            ),
        ]),
        json!({}),
    )
    .await;
    associate(&edge, &blocked_arn, &api.id, BLOCKED_STAGE).await;

    let path = format!("/{BLOCKED_STAGE}/echo");
    let settled = edge
        .invoke(&api.id, "GET", &path, &[(TRIGGER_HEADER, BLOCKED_TRIGGER)], &[])
        .await;
    assert_eq!(
        settled.status, 403,
        "case a count followed by a block: the count does not settle the request, so the \
         later block does, got {} with {:?}",
        settled.status, settled.body
    );
    assert_eq!(
        settled.message(),
        "Forbidden",
        "case a count followed by a block: the blocked request is answered through the \
         api's own WAF_FILTERED gateway response, got {:?}",
        settled.body
    );
    let admitted = edge.get(&api.id, &path).await;
    assert_eq!(
        admitted.status, 200,
        "case a count followed by a block, for a request neither rule matches: expected \
         200, got {} with {:?}",
        admitted.status, admitted.body
    );
    let seen = echoed_headers(&admitted, "a count followed by a block, unmatched");
    for (name, _) in &blocked_insert {
        for form in [inserted_as(name), name.clone()] {
            assert_eq!(
                seen.get(&form).map(String::as_str),
                Some(""),
                "case a count followed by a block, for a request neither rule matches: \
                 no {form:?} is inserted, got {:?}",
                seen.get(&form)
            );
        }
    }
}

/// Every `ResponseContentType` a `CustomResponseBody` can declare, with the media type a
/// response carrying that body is served under
/// (`smithy_query(query="shape",shape="ResponseContentType")`: `TEXT_PLAIN`, `TEXT_HTML`,
/// `APPLICATION_JSON`).
const RESPONSE_CONTENT_TYPES: [(&str, &str); 3] = [
    ("TEXT_PLAIN", "text/plain"),
    ("TEXT_HTML", "text/html"),
    ("APPLICATION_JSON", "application/json"),
];

/// The bounds `CustomResponse$ResponseCode` publishes
/// (`smithy_query(query="shape",shape="ResponseStatusCode")`: `@range(min: 200, max:
/// 599)`).
const RESPONSE_CODE_BOUNDS: (u16, u16) = (200, 599);

/// The longest `CustomResponseBody$Content` the member allows
/// (`smithy_query(query="shape",shape="CustomResponseBody")`: `@length(min: 1, max:
/// 10240)`).
const LONGEST_CONTENT: usize = 10_240;

/// The media type the `ResponseContentType` value `declared` names.
fn media_type(declared: &str) -> &'static str {
    RESPONSE_CONTENT_TYPES
        .iter()
        .find(|(value, _)| *value == declared)
        .map(|(_, media)| *media)
        .unwrap_or_else(|| panic!("{declared:?} is not a ResponseContentType this suite declares"))
}

/// One blocking rule's own custom response, and the stage its web ACL decides.
struct Refused {
    case: &'static str,
    /// The stage this row's web ACL is associated with, and the web ACL's own name.
    stage: &'static str,
    /// `CustomResponse$ResponseCode`.
    status: u16,
    /// The `CustomResponseBodies` entry the action names, as `(key, ContentType,
    /// Content)`, or `None` for an action that names no body at all.
    body: Option<(&'static str, &'static str, String)>,
    /// `CustomResponse$ResponseHeaders`, in the order they are configured.
    headers: Vec<(&'static str, &'static str)>,
}

/// The rule over a TERMINATING action's own `CustomResponse`: a request a blocking rule
/// matches is answered by WAF at the edge, so the integration never runs and the api's own
/// gateway response is not what the caller gets — the status is the action's
/// `ResponseCode`, the body is the web ACL's `CustomResponseBodies` entry the action's
/// `CustomResponseBodyKey` names, byte for byte, served under the media type that entry's
/// `ContentType` declares, and every header the action configures is on the response. A
/// request the rule does not match is served by the origin and carries none of them.
///
/// One row per thing the rule generalises over: which `ResponseContentType` the body
/// declares, what the status code is at each bound `ResponseStatusCode` publishes and
/// between them, how long the body is at each bound `CustomResponseBody$Content`
/// publishes, how many headers the action configures (none, one, two), and an action that
/// names no body at all. Each row gets its own stage and its own web ACL, so the only
/// thing deciding the row is that web ACL's one blocking rule. A status code past either
/// published bound is refused by the control plane instead.
#[tokio::test]
async fn a_blocking_rules_custom_response_is_served_at_the_edge_from_the_web_acls_bodies() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-custom-response").await;

    // A body exactly as long as the member allows, so the row proves the bytes come back
    // whole rather than clipped: the JSON envelope is ten glyphs of the budget.
    let longest = format!("{{\"pad\":\"{}\"}}", "z".repeat(LONGEST_CONTENT - 10));
    let rows = vec![
        Refused {
            case: "a JSON body and one configured header",
            stage: "json-body",
            status: 418,
            body: Some((
                "denied",
                "APPLICATION_JSON",
                "{\"waf\":\"denied\",\"at\":\"the edge\"}".to_string(),
            )),
            headers: vec![("x-waf-probe", "denied")],
        },
        Refused {
            case: "a plain-text body and two configured headers",
            stage: "plain-body",
            status: 503,
            body: Some((
                "too-busy",
                "TEXT_PLAIN",
                "denied at the edge: try later".to_string(),
            )),
            headers: vec![("x-waf-probe", "plain"), ("retry-after", "30")],
        },
        Refused {
            case: "an HTML body and no configured header, at the lowest status code the \
                   member allows",
            stage: "html-body",
            status: RESPONSE_CODE_BOUNDS.0,
            body: Some((
                "markup",
                "TEXT_HTML",
                "<html><body>denied</body></html>".to_string(),
            )),
            headers: Vec::new(),
        },
        Refused {
            case: "the shortest body the member allows, at the highest status code it \
                   allows",
            stage: "shortest-body",
            status: RESPONSE_CODE_BOUNDS.1,
            body: Some(("tiny", "TEXT_PLAIN", "x".to_string())),
            headers: vec![("x-waf-probe", "tiny")],
        },
        Refused {
            case: "the longest body the member allows",
            stage: "longest-body",
            status: 451,
            body: Some(("long", "APPLICATION_JSON", longest.clone())),
            headers: vec![("x-waf-probe", "long")],
        },
        Refused {
            case: "a blocking action that names no response body at all",
            stage: "no-body",
            status: 429,
            body: None,
            headers: vec![("x-waf-probe", "empty")],
        },
    ];

    // Every published `ResponseContentType` is exercised by some row: the catalogue is
    // the member's own value list, so a row per value is what spans it.
    for (declared, _) in RESPONSE_CONTENT_TYPES {
        assert!(
            rows.iter().any(|row| row
                .body
                .as_ref()
                .is_some_and(|(_, content_type, _)| *content_type == declared)),
            "case the ResponseContentType catalogue: no row declares {declared:?}"
        );
    }

    for row in &rows {
        api.deploy(&edge, row.stage).await;
    }

    let mut created: Vec<(&Refused, String, String)> = Vec::new();
    for row in &rows {
        let mut custom = json!({ "ResponseCode": row.status });
        let mut extra = json!({});
        if let Some((key, content_type, content)) = &row.body {
            custom["CustomResponseBodyKey"] = json!(key);
            let mut bodies = serde_json::Map::new();
            bodies.insert(
                (*key).to_string(),
                json!({ "ContentType": content_type, "Content": content }),
            );
            // `CustomResponseBodies` is "a map of custom response keys and content
            // bodies … then use them in the rules", so the body lives on the web ACL and
            // the action only names it.
            extra["CustomResponseBodies"] = Value::Object(bodies);
        }
        if !row.headers.is_empty() {
            custom["ResponseHeaders"] = Value::Array(
                row.headers
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            );
        }
        let (id, arn, _) = create_web_acl(
            &edge,
            row.stage,
            json!({ "Allow": {} }),
            json!([rule(
                "orders",
                0,
                byte_match(json!({ "UriPath": {} }), "/orders", "ENDS_WITH"),
                json!({ "Block": { "CustomResponse": custom } }),
            )]),
            extra,
        )
        .await;
        associate(&edge, &arn, &api.id, row.stage).await;
        created.push((row, id, arn));
    }

    for (row, _, _) in &created {
        let blocked = edge.get(&api.id, &format!("/{}/orders", row.stage)).await;
        assert_eq!(
            blocked.status, row.status,
            "case {:?}: the caller gets the status the action's CustomResponse \
             configures, got {} with {:?}",
            row.case, blocked.status, blocked.body
        );
        let expected_body = row
            .body
            .as_ref()
            .map(|(_, _, content)| content.clone())
            .unwrap_or_default();
        assert_eq!(
            blocked.body, expected_body,
            "case {:?}: the body is the web ACL's own entry byte for byte, and never the \
             origin's",
            row.case
        );
        assert_ne!(
            blocked.body,
            origin_body("orders"),
            "case {:?}: the origin is never reached by a blocked request",
            row.case
        );
        if let Some((_, content_type, _)) = &row.body {
            assert_eq!(
                blocked.header("content-type"),
                Some(media_type(content_type)),
                "case {:?}: the body is served under the media type its ContentType \
                 declares, got {:?}",
                row.case,
                blocked.headers
            );
        }
        for (name, value) in &row.headers {
            assert_eq!(
                blocked.header(name),
                Some(*value),
                "case {:?}: the response carries the configured header {name}={value:?}, \
                 got {:?}",
                row.case,
                blocked.headers
            );
        }

        // A sibling path the rule does not match: served by the origin, carrying none of
        // the custom response's headers.
        let served = edge.get(&api.id, &format!("/{}/admin", row.stage)).await;
        assert_eq!(
            served.status, 200,
            "case {:?}: a path the blocking rule does not match is served by the origin, \
             got {} with {:?}",
            row.case, served.status, served.body
        );
        assert_eq!(
            served.body,
            origin_body("admin"),
            "case {:?}: the unmatched path is answered by the integration, got {:?}",
            row.case,
            served.body
        );
        for (name, _) in &row.headers {
            assert_eq!(
                served.header(name),
                None,
                "case {:?}: a request WAF did not refuse carries no {name:?}, got {:?}",
                row.case,
                served.headers
            );
        }
    }

    // Serving a custom response changes nothing about the web ACL: its body map reads
    // back as it was stored, and the stage is still the resource it is associated with.
    for (row, id, arn) in &created {
        let stored = edge
            .waf_ok(
                "GetWebACL",
                json!({ "Name": row.stage, "Scope": SCOPE, "Id": id }),
            )
            .await;
        if let Some((key, content_type, content)) = &row.body {
            let entry = &stored["WebACL"]["CustomResponseBodies"][*key];
            assert_eq!(
                entry["ContentType"].as_str(),
                Some(*content_type),
                "case {:?}: the stored body's ContentType reads back, got {stored}",
                row.case
            );
            assert_eq!(
                entry["Content"].as_str(),
                Some(content.as_str()),
                "case {:?}: the stored body's Content reads back byte for byte",
                row.case
            );
        }
        let for_resource = edge
            .waf_ok(
                "GetWebACLForResource",
                json!({ "ResourceArn": stage_arn(&api.id, row.stage) }),
            )
            .await;
        assert_eq!(
            for_resource["WebACL"]["ARN"].as_str(),
            Some(arn.as_str()),
            "case {:?}: the stage still reports the web ACL in front of it, got \
             {for_resource}",
            row.case
        );
    }

    // A status code past either published bound is not a custom response WAF can serve,
    // so the web ACL carrying it is refused at creation.
    for (case, code) in [
        (
            "one below the lowest status code the member allows",
            RESPONSE_CODE_BOUNDS.0 - 1,
        ),
        (
            "one above the highest status code the member allows",
            RESPONSE_CODE_BOUNDS.1 + 1,
        ),
    ] {
        let (status, parsed) = edge
            .waf_call(
                "CreateWebACL",
                json!({
                    "Name": format!("out-of-range-{code}"),
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility("out-of-range"),
                    "Rules": [rule(
                        "orders",
                        0,
                        byte_match(json!({ "UriPath": {} }), "/orders", "ENDS_WITH"),
                        json!({ "Block": { "CustomResponse": { "ResponseCode": code } } }),
                    )],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a ResponseCode of {code} is outside the published range, so \
             the web ACL is refused, got {status} with {parsed}"
        );
    }
}

/// An injected SQL fragment in a query argument, percent-encoded as a client sends it:
/// what the managed SQLi rule group refuses, when it is the group that evaluates the
/// request at all.
const SQLI_ARGUMENT: &str = "q=%27%20OR%201%3D1%20--";

/// The same argument carrying a benign value, which is what tells a request the group
/// evaluated and found nothing from one the group never evaluated.
const BENIGN_ARGUMENT: &str = "q=alice";

/// One request a scoped-down managed rule group decides, and what it is answered.
struct Probe {
    probe: &'static str,
    /// The path within the stage, query string included.
    target: String,
    headers: Vec<(&'static str, &'static str)>,
    /// The resource whose `MOCK` integration answers the request, when it reaches the
    /// origin at all; `None` when the group refuses it at the edge.
    origin: Option<&'static str>,
}

/// One managed-rule-group rule's scope-down predicate, and the stage its web ACL decides.
struct Scoped {
    case: &'static str,
    /// The stage this row's web ACL is associated with, and the web ACL's own name.
    stage: &'static str,
    /// The `ScopeDownStatement` the group statement carries, or `None` for a group that
    /// evaluates every request the web ACL sees.
    scope_down: Option<Value>,
    probes: Vec<Probe>,
}

/// A `ManagedRuleGroupStatement` over the SQLi rule set, narrowed by `scope_down` when it
/// carries one.
fn managed_sqli_group(scope_down: Option<Value>) -> Value {
    let mut statement = json!({
        "VendorName": "AWS",
        "Name": "AWSManagedRulesSQLiRuleSet",
    });
    if let Some(scope_down) = scope_down {
        statement["ScopeDownStatement"] = scope_down;
    }
    json!({ "ManagedRuleGroupStatement": statement })
}

/// The metric name — and so the rule name — of the web ACL rule that runs the group.
const GROUP_RULE: &str = "sql-database";

/// A web ACL rule running `statement`: a rule-group rule carries no `Action` of its own,
/// only whether to override the actions of the rules inside the group.
fn group_rule(statement: Value) -> Value {
    json!({
        "Name": GROUP_RULE,
        "Priority": 0,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(GROUP_RULE),
    })
}

/// The rule over a `ManagedRuleGroupStatement$ScopeDownStatement`: it "narrows the scope
/// of the web requests that are evaluated by the managed rule group", so only a request
/// matching that nested statement is evaluated by the group's rules at all — a request
/// outside the predicate reaches the origin even when the group would have refused it,
/// one inside it is refused, and a benign request inside it is served, which is what
/// separates a group that ran from a group that never did. The predicate is read from the
/// web ACL the control plane holds NOW, so removing it widens the group and putting it
/// back narrows it again.
///
/// One row per thing the predicate generalises over: no predicate at all, a URI-path
/// substring, a header value, a query-string substring, and the negation of a predicate —
/// each on its own stage of the same api, with the same payload probed inside and outside
/// the predicate so the only difference is whether the group was asked.
#[tokio::test]
async fn a_scope_down_statement_decides_which_requests_the_managed_group_evaluates() {
    let edge = Edge::new();
    let api = Api::build(&edge, "edge-scope-down").await;
    // A resource one segment deeper, so a predicate over the URI path has a path inside
    // it and a sibling outside it to separate.
    let nested = api
        .mock_resource(
            &edge,
            &api.root,
            "api",
            "GET",
            json!({}),
            &origin_body("api"),
        )
        .await;
    api.mock_resource(
        &edge,
        &nested,
        "orders",
        "GET",
        json!({}),
        &origin_body("api/orders"),
    )
    .await;

    /// The header a row's predicate inspects.
    const SCOPE_HEADER: &str = "x-waf-scope";

    let rows = vec![
        Scoped {
            case: "no scope-down at all: the group evaluates every request",
            stage: "everywhere",
            scope_down: None,
            probes: vec![
                Probe {
                    probe: "the payload on a path one segment deep",
                    target: format!("/api/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: None,
                },
                Probe {
                    probe: "the identical payload at the top level",
                    target: format!("/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: None,
                },
                Probe {
                    probe: "a benign argument",
                    target: format!("/api/orders?{BENIGN_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("api/orders"),
                },
            ],
        },
        Scoped {
            case: "scoped down to the uri paths containing a prefix",
            stage: "contained",
            scope_down: Some(byte_match(json!({ "UriPath": {} }), "/api/", "CONTAINS")),
            probes: vec![
                Probe {
                    probe: "the payload inside the predicate",
                    target: format!("/api/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: None,
                },
                Probe {
                    probe: "the identical payload outside the predicate",
                    target: format!("/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("orders"),
                },
                Probe {
                    probe: "a benign argument inside the predicate",
                    target: format!("/api/orders?{BENIGN_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("api/orders"),
                },
                Probe {
                    probe: "a benign argument outside the predicate",
                    target: format!("/orders?{BENIGN_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("orders"),
                },
            ],
        },
        Scoped {
            case: "scoped down to the requests carrying a header exactly",
            stage: "headered",
            scope_down: Some(byte_match(
                json!({ "SingleHeader": { "Name": SCOPE_HEADER } }),
                "inspect",
                "EXACTLY",
            )),
            probes: vec![
                Probe {
                    probe: "the payload carrying the inspected header",
                    target: format!("/orders?{SQLI_ARGUMENT}"),
                    headers: vec![(SCOPE_HEADER, "inspect")],
                    origin: None,
                },
                Probe {
                    probe: "the identical payload carrying another value",
                    target: format!("/orders?{SQLI_ARGUMENT}"),
                    headers: vec![(SCOPE_HEADER, "skip")],
                    origin: Some("orders"),
                },
                Probe {
                    probe: "the identical payload carrying no such header",
                    target: format!("/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("orders"),
                },
                Probe {
                    probe: "a benign argument carrying the inspected header",
                    target: format!("/orders?{BENIGN_ARGUMENT}"),
                    headers: vec![(SCOPE_HEADER, "inspect")],
                    origin: Some("orders"),
                },
            ],
        },
        Scoped {
            case: "scoped down to the query strings naming one tenant",
            stage: "tenanted",
            scope_down: Some(byte_match(
                json!({ "QueryString": {} }),
                "tenant=acme",
                "CONTAINS",
            )),
            probes: vec![
                Probe {
                    probe: "the payload inside the predicate",
                    target: format!("/orders?tenant=acme&{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: None,
                },
                Probe {
                    probe: "the identical payload outside the predicate",
                    target: format!("/orders?tenant=other&{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("orders"),
                },
            ],
        },
        Scoped {
            case: "scoped down to the NEGATION of a uri-path predicate",
            stage: "negated",
            scope_down: Some(json!({
                "NotStatement": {
                    "Statement": byte_match(json!({ "UriPath": {} }), "/api/", "CONTAINS")
                }
            })),
            probes: vec![
                Probe {
                    probe: "the payload on the path the negated predicate excludes",
                    target: format!("/api/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: Some("api/orders"),
                },
                Probe {
                    probe: "the identical payload on a path it includes",
                    target: format!("/orders?{SQLI_ARGUMENT}"),
                    headers: Vec::new(),
                    origin: None,
                },
            ],
        },
    ];

    for row in &rows {
        api.deploy(&edge, row.stage).await;
    }

    /// Drive one row's probes against the stage its web ACL decides.
    async fn probe_all(edge: &Edge, api: &str, case: &str, stage: &str, probes: &[Probe]) {
        for probe in probes {
            let answered = edge
                .invoke(
                    api,
                    "GET",
                    &format!("/{stage}{}", probe.target),
                    &probe.headers,
                    &[],
                )
                .await;
            match probe.origin {
                Some(resource) => {
                    assert_eq!(
                        answered.status, 200,
                        "case {case:?} with {:?}: a request the group did not refuse is \
                         served by the origin, got {} with {:?}",
                        probe.probe, answered.status, answered.body
                    );
                    assert_eq!(
                        answered.body,
                        origin_body(resource),
                        "case {case:?} with {:?}: the integration answers the request, \
                         got {:?}",
                        probe.probe,
                        answered.body
                    );
                }
                None => {
                    assert_eq!(
                        answered.status, 403,
                        "case {case:?} with {:?}: the group refuses the request, got {} \
                         with {:?}",
                        probe.probe, answered.status, answered.body
                    );
                    assert_eq!(
                        answered.message(),
                        "Forbidden",
                        "case {case:?} with {:?}: a refusal with no custom response is \
                         answered through the api's own WAF_FILTERED gateway response, \
                         got {:?}",
                        probe.probe,
                        answered.body
                    );
                }
            }
        }
    }

    let mut narrowed: Option<(String, String)> = None;
    for row in &rows {
        let (id, arn, lock) = create_web_acl(
            &edge,
            row.stage,
            json!({ "Allow": {} }),
            json!([group_rule(managed_sqli_group(row.scope_down.clone()))]),
            json!({}),
        )
        .await;
        associate(&edge, &arn, &api.id, row.stage).await;
        probe_all(&edge, &api.id, row.case, row.stage, &row.probes).await;
        if row.stage == "contained" {
            narrowed = Some((id, lock));
        }
    }

    // Removing ONLY the scope-down widens the group to every request the web ACL sees:
    // the payload that was out of scope is refused now, and a benign request is still
    // served, so what changed is which requests the group was asked about.
    let (id, lock) = narrowed.expect("the row whose group is scoped down to a uri path");
    let stage = "contained";
    let widened = edge
        .waf_ok(
            "UpdateWebACL",
            json!({
                "Name": stage,
                "Scope": SCOPE,
                "Id": id,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(stage),
                "Rules": [group_rule(managed_sqli_group(None))],
                "LockToken": lock,
            }),
        )
        .await;
    let next_lock = widened["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();
    probe_all(
        &edge,
        &api.id,
        "the scope-down removed by UpdateWebACL",
        stage,
        &[
            Probe {
                probe: "the payload that was outside the predicate",
                target: format!("/orders?{SQLI_ARGUMENT}"),
                headers: Vec::new(),
                origin: None,
            },
            Probe {
                probe: "a benign argument on the same path",
                target: format!("/orders?{BENIGN_ARGUMENT}"),
                headers: Vec::new(),
                origin: Some("orders"),
            },
        ],
    )
    .await;

    // And putting it back narrows the group again, so the edge reads the predicate the
    // control plane holds now rather than the one it held at association time.
    edge.waf_ok(
        "UpdateWebACL",
        json!({
            "Name": stage,
            "Scope": SCOPE,
            "Id": id,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility(stage),
            "Rules": [group_rule(managed_sqli_group(Some(byte_match(
                json!({ "UriPath": {} }),
                "/api/",
                "CONTAINS",
            ))))],
            "LockToken": next_lock,
        }),
    )
    .await;
    probe_all(
        &edge,
        &api.id,
        "the scope-down restored by UpdateWebACL",
        stage,
        &[
            Probe {
                probe: "the payload outside the predicate again",
                target: format!("/orders?{SQLI_ARGUMENT}"),
                headers: Vec::new(),
                origin: Some("orders"),
            },
            Probe {
                probe: "the identical payload inside it",
                target: format!("/api/orders?{SQLI_ARGUMENT}"),
                headers: Vec::new(),
                origin: None,
            },
        ],
    )
    .await;
}
