// SPDX-License-Identifier: CC0-1.0

//! What a WAFv2 web ACL in front of a deployed REST API stage LOGS about the requests it
//! decides, end to end over the wire.
//!
//! The rule this suite pins, one level above the recorded case:
//!
//! > While a web ACL carries a logging configuration, EVERY request it evaluates leaves
//! > one record at the destination that configuration names — the request it refused at
//! > the edge as much as the one it let through to the origin — naming the action WAF
//! > applied and the rule that applied it, and showing each component the configuration's
//! > `RedactedFields` names as `REDACTED` in place of what the client sent. A web ACL with
//! > no logging configuration logs nothing; a configuration whose `LoggingFilter` drops a
//! > record delivers none for that request; a destination naming a resource of another kind
//! > than a log group has nothing written into a log group.
//!
//! Only a cross-service suite can pin this: the api and its stage are API Gateway's, the
//! web ACL and its logging configuration are WAFv2's, and the group the record lands in is
//! CloudWatch Logs' — all three on ONE engine, as `core/router/src/lib.rs` mounts them.
//! Every assertion reads the outcome back through CloudWatch Logs' own reader
//! (`FilterLogEvents`), so a record that did not reach that store cannot pass.
//!
//! The rows differ along what the rule generalises over: which component is redacted (each
//! of the four `RedactedFields` admits, and all of them at once), the ACTION the web ACL
//! applied (a rule that blocked, a rule that allowed, the default action), whether a
//! logging configuration is in place at all, what its `LoggingFilter` says, and what kind
//! of destination it names.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the invoke host, both control planes and every ARN name.
const REGION: &str = "us-east-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource.
const SCOPE: &str = "REGIONAL";

/// What a redacted component shows in a record instead of what the client sent, from
/// `LoggingConfiguration$RedactedFields`: "the `HEADER` field in the logs will be
/// `REDACTED`".
const REDACTED: &str = "REDACTED";

/// The `terminatingRuleId` a record names for a request no rule terminated, which the web
/// ACL's `DefaultAction` decided.
const DEFAULT_ACTION: &str = "Default_Action";

/// The resource path every case drives, and the query string it carries.
const RESOURCE_PATH: &str = "orders";
const QUERY: &str = "id=7";

/// The header every case sends and some redact: a caller's credential, which is exactly
/// what a `SingleHeader` redaction is for.
const SECRET_HEADER: &str = "authorization";
const SECRET_VALUE: &str = "Bearer a-caller-token";

/// A header no case redacts, so every record shows what the client sent under it.
const KEPT_HEADER: &str = "user-agent";
const KEPT_VALUE: &str = "waf-logging-suite";

/// One emulator with API Gateway, WAFv2 and CloudWatch Logs mounted on a single shared
/// engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises the services, not SigV4: the control-plane calls carry a
        // credential scope so the router can name the target service, and the invokes
        // carry no credential at all, as a client of a published invoke URL sends none.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request as it arrives on the wire, answering its status and body.
    async fn dispatch(
        &self,
        method: &str,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Bytes,
    ) -> (u16, Value, String) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(builder.body(body).expect("a well-formed request"))
            .await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, parsed, text)
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what
    /// routes the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// One API Gateway control-plane call that has to be accepted with `expected`.
    async fn api_ok(&self, method: &str, path: &str, body: Option<Value>, expected: u16) -> Value {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let (status, parsed, text) = self
            .dispatch(
                method,
                &format!("apigateway.{REGION}.amazonaws.com"),
                path,
                &[
                    ("content-type", "application/json"),
                    ("authorization", &Self::authorization("apigateway")),
                ],
                Bytes::from(payload),
            )
            .await;
        assert_eq!(
            status, expected,
            "setup {method} {path}: expected HTTP {expected}, got {status} with {text:?}"
        );
        parsed
    }

    /// One WAFv2 call that has to succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed, text) = self
            .dispatch(
                "POST",
                &format!("wafv2.{REGION}.amazonaws.com"),
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("authorization", &Self::authorization("wafv2")),
                    ("x-amz-target", &format!("AWSWAF_20190729.{operation}")),
                ],
                Bytes::from(body.to_string()),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup {operation}: expected HTTP 200, got {status} with {text:?}"
        );
        parsed
    }

    /// One CloudWatch Logs call, answering its status and parsed body.
    async fn logs(&self, operation: &str, body: Value) -> (u16, Value) {
        let (status, parsed, _) = self
            .dispatch(
                "POST",
                &format!("logs.{REGION}.amazonaws.com"),
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("authorization", &Self::authorization("logs")),
                    ("x-amz-target", &format!("Logs_20140328.{operation}")),
                ],
                Bytes::from(body.to_string()),
            )
            .await;
        (status, parsed)
    }

    /// One unsigned request to a deployed stage's invoke URL: the api's own host, no
    /// credential, the headers every case sends.
    async fn invoke(&self, api: &str, stage: &str) -> (u16, String) {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let (status, _, text) = self
            .dispatch(
                "GET",
                &host,
                &format!("/{stage}/{RESOURCE_PATH}?{QUERY}"),
                &[
                    (SECRET_HEADER, SECRET_VALUE),
                    (KEPT_HEADER, KEPT_VALUE),
                    ("content-type", "application/json"),
                ],
                Bytes::new(),
            )
            .await;
        (status, text)
    }

    /// The records `FilterLogEvents` reports for `group`, parsed as the JSON lines a WAF
    /// log record is, or `None` when the account holds no such group at all — which is
    /// what "nothing was delivered and nothing was created" looks like to a client.
    async fn delivered(&self, group: &str) -> Option<Vec<Value>> {
        let (status, body) = self
            .logs("FilterLogEvents", json!({ "logGroupName": group }))
            .await;
        if status != 200 {
            return None;
        }
        Some(
            body.get("events")
                .and_then(Value::as_array)
                .map(|events| {
                    events
                        .iter()
                        .map(|event| {
                            let message = event
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            serde_json::from_str(message).unwrap_or(Value::Null)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    }
}

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by, the one form
/// `AssociateWebACLRequest$ResourceArn` documents for an API Gateway REST API stage.
fn stage_arn(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// The ARN of the log group `name`, as a `LogDestinationConfigs` entry names one.
fn group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{account}:log-group:{name}",
        account = engine::emulator_account_id()
    )
}

/// A `VisibilityConfig` for a dimension, which every rule and web ACL needs.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// A rule named `name` that applies `action` to a request whose URI path ends in the
/// resource path every case drives.
fn rule_on_the_path(name: &str, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": {
            "ByteMatchStatement": {
                "SearchString": base64_of(&format!("/{RESOURCE_PATH}")),
                "FieldToMatch": { "UriPath": {} },
                "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                "PositionalConstraint": "ENDS_WITH",
            }
        },
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A `SingleHeader` redaction naming `header`.
fn single_header(header: &str) -> Value {
    json!({ "SingleHeader": { "Name": header } })
}

/// The api every case is driven against: one `GET` resource on a `MOCK` integration that
/// answers `200`, so the answer a request gets says whether it reached the origin.
async fn deployed_api(emulator: &Emulator, name: &str, stages: &[&str]) -> String {
    let created = emulator
        .api_ok("POST", "/restapis", Some(json!({ "name": name })), 201)
        .await;
    let api = created["id"].as_str().expect("the api id").to_string();
    let root = created["rootResourceId"]
        .as_str()
        .expect("the root resource id")
        .to_string();
    let resource = emulator
        .api_ok(
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": RESOURCE_PATH })),
            201,
        )
        .await;
    let resource = resource["id"]
        .as_str()
        .expect("the resource id")
        .to_string();
    emulator
        .api_ok(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
    emulator
        .api_ok(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(json!({ "type": "MOCK" })),
            201,
        )
        .await;
    emulator
        .api_ok(
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"
            ),
            Some(json!({
                "responseTemplates": { "application/json": r#"{"at":"origin"}"# }
            })),
            201,
        )
        .await;
    for stage in stages {
        emulator
            .api_ok(
                "POST",
                &format!("/restapis/{api}/deployments"),
                Some(json!({ "stageName": stage })),
                201,
            )
            .await;
    }
    api
}

/// Create a web ACL with `rules` and the default `Allow`, put it in front of the stage,
/// and answer its ARN.
async fn web_acl_in_front(
    emulator: &Emulator,
    name: &str,
    default_action: Value,
    rules: Value,
    api: &str,
    stage: &str,
) -> String {
    let created = emulator
        .waf_ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": default_action,
                "VisibilityConfig": visibility(name),
                "Rules": rules,
            }),
        )
        .await;
    let arn = created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    emulator
        .waf_ok(
            "AssociateWebACL",
            json!({ "WebACLArn": arn, "ResourceArn": stage_arn(api, stage) }),
        )
        .await;
    arn
}

/// Put a logging configuration in front of the web ACL `arn`: `destination` as the one
/// `LogDestinationConfigs` entry, `redacted` as its `RedactedFields` and `filter` as its
/// `LoggingFilter`.
async fn configure_logging(
    emulator: &Emulator,
    arn: &str,
    destination: &str,
    redacted: Option<Vec<Value>>,
    filter: Option<Value>,
) {
    let mut configuration = json!({
        "ResourceArn": arn,
        "LogDestinationConfigs": [destination],
    });
    if let Some(redacted) = redacted {
        configuration["RedactedFields"] = json!(redacted);
    }
    if let Some(filter) = filter {
        configuration["LoggingFilter"] = filter;
    }
    emulator
        .waf_ok(
            "PutLoggingConfiguration",
            json!({ "LoggingConfiguration": configuration }),
        )
        .await;
}

/// The one record delivered into `group`, which every row below expects exactly one of.
async fn one_record(emulator: &Emulator, case: &str, group: &str) -> Value {
    let delivered = emulator
        .delivered(group)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the destination group {group:?} was not created"));
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected exactly one record in {group:?}, got {delivered:?}"
    );
    delivered.into_iter().next().unwrap_or(Value::Null)
}

/// The `value` member of the header `name` in a record's `httpRequest.headers`.
fn logged_header(record: &Value, name: &str) -> String {
    record["httpRequest"]["headers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .find(|header| {
            header["name"]
                .as_str()
                .is_some_and(|logged| logged.eq_ignore_ascii_case(name))
        })
        .and_then(|header| header["value"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// One row of the redaction table: which components the configuration keeps out of the
/// logs, and what the record then shows for each of them.
struct RedactionRow {
    case: &'static str,
    /// The stage, the web ACL and the destination group this row drives, all named for it.
    name: &'static str,
    /// The `RedactedFields` the configuration carries.
    redacted: Vec<Value>,
    /// What `httpRequest.httpMethod` shows.
    method: &'static str,
    /// What `httpRequest.uri` shows.
    uri: String,
    /// What `httpRequest.args` shows.
    args: &'static str,
    /// What the `authorization` header shows.
    secret: &'static str,
}

/// The rows: nothing redacted, each of the four components `RedactedFields` admits on its
/// own, and all of them at once.
fn redaction_rows() -> Vec<RedactionRow> {
    let path = |stage: &str| format!("/{stage}/{RESOURCE_PATH}");
    vec![
        RedactionRow {
            case: "no redacted fields at all: the record shows the request as it arrived",
            name: "plain",
            redacted: vec![],
            method: "GET",
            uri: path("plain"),
            args: QUERY,
            secret: SECRET_VALUE,
        },
        RedactionRow {
            case: "the HTTP method redacted",
            name: "method",
            redacted: vec![json!({ "Method": {} })],
            method: REDACTED,
            uri: path("method"),
            args: QUERY,
            secret: SECRET_VALUE,
        },
        RedactionRow {
            case: "the URI path redacted",
            name: "uripath",
            redacted: vec![json!({ "UriPath": {} })],
            method: "GET",
            uri: REDACTED.to_string(),
            args: QUERY,
            secret: SECRET_VALUE,
        },
        RedactionRow {
            case: "the query string redacted",
            name: "querystring",
            redacted: vec![json!({ "QueryString": {} })],
            method: "GET",
            uri: path("querystring"),
            args: REDACTED,
            secret: SECRET_VALUE,
        },
        RedactionRow {
            case: "one header redacted, by a name in another case than it arrived in",
            name: "header",
            redacted: vec![single_header("Authorization")],
            method: "GET",
            uri: path("header"),
            args: QUERY,
            secret: REDACTED,
        },
        RedactionRow {
            case: "every component the member admits for redaction, at once",
            name: "all",
            redacted: vec![
                json!({ "Method": {} }),
                json!({ "UriPath": {} }),
                json!({ "QueryString": {} }),
                single_header(SECRET_HEADER),
            ],
            method: REDACTED,
            uri: REDACTED.to_string(),
            args: REDACTED,
            secret: REDACTED,
        },
    ]
}

/// A blocked request leaves a record at the configured destination, and each component the
/// configuration redacts shows `REDACTED` in it while every other one shows what the
/// client sent.
#[tokio::test]
async fn a_blocked_request_leaves_a_record_with_the_declared_fields_redacted() {
    let emulator = Emulator::new();
    let rows = redaction_rows();
    let stages: Vec<&str> = rows.iter().map(|row| row.name).collect();
    let api = deployed_api(&emulator, "waf-logged-api", &stages).await;

    for row in &rows {
        let group = format!("aws-waf-logs-{}", row.name);
        let arn = web_acl_in_front(
            &emulator,
            &format!("block-{}", row.name),
            json!({ "Allow": {} }),
            json!([rule_on_the_path(
                &format!("deny-{}", row.name),
                json!({ "Block": {} })
            )]),
            &api,
            row.name,
        )
        .await;
        configure_logging(
            &emulator,
            &arn,
            &group_arn(&group),
            Some(row.redacted.clone()),
            None,
        )
        .await;

        // The request is answered at the edge: the api's own `WAF_FILTERED` gateway
        // response, which is a `403`, and the origin is never reached.
        let (status, body) = emulator.invoke(&api, row.name).await;
        assert_eq!(
            status, 403,
            "case {:?}: a blocked request must be answered 403 at the stage, got {status} \
             with {body:?}",
            row.case
        );

        let record = one_record(&emulator, row.case, &group).await;
        assert_eq!(
            record["action"],
            json!("BLOCK"),
            "case {:?}: the record must name the action WAF applied, got {record}",
            row.case
        );
        assert_eq!(
            record["terminatingRuleId"],
            json!(format!("deny-{}", row.name)),
            "case {:?}: the record must name the rule that terminated the evaluation, got \
             {record}",
            row.case
        );
        assert_eq!(
            record["terminatingRuleType"],
            json!("REGULAR"),
            "case {:?}: a rule that inspects the request itself is REGULAR, got {record}",
            row.case
        );
        assert_eq!(
            record["webaclId"],
            json!(arn),
            "case {:?}: the record must name the web ACL that decided, got {record}",
            row.case
        );
        assert_eq!(
            record["httpSourceName"],
            json!("APIGW"),
            "case {:?}: the record must name the kind of resource the request arrived at, \
             got {record}",
            row.case
        );
        assert_eq!(
            record["formatVersion"],
            json!(1),
            "case {:?}: the record must name the log format it is written in, got {record}",
            row.case
        );
        assert_eq!(
            record["httpRequest"]["httpMethod"],
            json!(row.method),
            "case {:?}: expected httpMethod {:?}, got {record}",
            row.case,
            row.method
        );
        assert_eq!(
            record["httpRequest"]["uri"],
            json!(row.uri),
            "case {:?}: expected uri {:?}, got {record}",
            row.case,
            row.uri
        );
        assert_eq!(
            record["httpRequest"]["args"],
            json!(row.args),
            "case {:?}: expected args {:?}, got {record}",
            row.case,
            row.args
        );
        let secret = logged_header(&record, SECRET_HEADER);
        assert_eq!(
            secret, row.secret,
            "case {:?}: expected the {SECRET_HEADER} header {:?}, got {secret:?} in \
             {record}",
            row.case, row.secret
        );
        // Redaction applies only to the component named, so a header no entry names
        // always shows what the client sent.
        let kept = logged_header(&record, KEPT_HEADER);
        assert_eq!(
            kept, KEPT_VALUE,
            "case {:?}: a header no redaction names must show what the client sent, got \
             {kept:?} in {record}",
            row.case
        );
    }
}

/// One row of the delivery table: how the web ACL decided the request, and what it logs.
struct DeliveryRow {
    case: &'static str,
    /// The stage, the web ACL and the destination group this row drives.
    name: &'static str,
    /// The web ACL's `DefaultAction`.
    default_action: Value,
    /// The rules it carries.
    rules: Value,
    /// The status the stage answers the request with.
    status: u16,
    /// The `action` and `terminatingRuleId` the delivered record names, or `None` when the
    /// web ACL delivers no record for this request at all.
    logged: Option<(&'static str, &'static str)>,
    /// The destination the configuration names, when it carries one.
    destination: Option<String>,
    /// Its `LoggingFilter`, when it carries one.
    filter: Option<Value>,
}

/// The rows: a request each terminating action decided, one the default action decided, a
/// web ACL with no logging configuration at all, a filter that drops the record of a
/// blocked request, a filter that keeps it, and a destination naming a bucket rather than a
/// log group (the delivery a bucket destination takes is the WAFv2 crate's
/// `tests/all/logging_destination_delivery.rs` subject).
fn delivery_rows() -> Vec<DeliveryRow> {
    let keeping = |behavior: &str, action: &str| {
        json!({
            "DefaultBehavior": behavior,
            "Filters": [{
                "Behavior": if behavior == "KEEP" { "DROP" } else { "KEEP" },
                "Requirement": "MEETS_ANY",
                "Conditions": [{ "ActionCondition": { "Action": action } }],
            }],
        })
    };
    vec![
        DeliveryRow {
            case: "a request a rule blocked",
            name: "blocked",
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path("deny", json!({ "Block": {} }))]),
            status: 403,
            logged: Some(("BLOCK", "deny")),
            destination: Some(group_arn("aws-waf-logs-blocked")),
            filter: None,
        },
        DeliveryRow {
            case: "a request a rule allowed, which reached the origin",
            name: "allowed",
            default_action: json!({ "Block": {} }),
            rules: json!([rule_on_the_path("permit", json!({ "Allow": {} }))]),
            status: 200,
            logged: Some(("ALLOW", "permit")),
            destination: Some(group_arn("aws-waf-logs-allowed")),
            filter: None,
        },
        DeliveryRow {
            case: "a request no rule matched, which the default action decided",
            name: "defaulted",
            default_action: json!({ "Allow": {} }),
            rules: json!([]),
            status: 200,
            logged: Some(("ALLOW", DEFAULT_ACTION)),
            destination: Some(group_arn("aws-waf-logs-defaulted")),
            filter: None,
        },
        DeliveryRow {
            case: "a web ACL with no logging configuration at all logs nothing",
            name: "unlogged",
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path("deny", json!({ "Block": {} }))]),
            status: 403,
            logged: None,
            destination: None,
            filter: None,
        },
        DeliveryRow {
            case: "a filter that drops the records of blocked requests",
            name: "dropped",
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path("deny", json!({ "Block": {} }))]),
            status: 403,
            logged: None,
            destination: Some(group_arn("aws-waf-logs-dropped")),
            filter: Some(keeping("KEEP", "BLOCK")),
        },
        DeliveryRow {
            case: "a filter that keeps only the records of blocked requests",
            name: "kept",
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path("deny", json!({ "Block": {} }))]),
            status: 403,
            logged: Some(("BLOCK", "deny")),
            destination: Some(group_arn("aws-waf-logs-kept")),
            filter: Some(keeping("DROP", "BLOCK")),
        },
        DeliveryRow {
            case: "a destination naming a bucket writes nothing into a log group of that name",
            name: "bucketed",
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path("deny", json!({ "Block": {} }))]),
            status: 403,
            logged: None,
            destination: Some("arn:aws:s3:::aws-waf-logs-bucketed".to_string()),
            filter: None,
        },
    ]
}

/// Every request a configured web ACL evaluates leaves one record at the destination the
/// configuration names, naming the action applied and the rule that applied it; a web ACL
/// that logs nothing about a request creates no group and leaves no record.
#[tokio::test]
async fn every_evaluated_request_leaves_the_record_its_configuration_asks_for() {
    let emulator = Emulator::new();
    let rows = delivery_rows();
    let stages: Vec<&str> = rows.iter().map(|row| row.name).collect();
    let api = deployed_api(&emulator, "waf-delivery-api", &stages).await;

    for row in &rows {
        let group = format!("aws-waf-logs-{}", row.name);
        let arn = web_acl_in_front(
            &emulator,
            &format!("acl-{}", row.name),
            row.default_action.clone(),
            row.rules.clone(),
            &api,
            row.name,
        )
        .await;
        if let Some(destination) = row.destination.as_deref() {
            configure_logging(&emulator, &arn, destination, None, row.filter.clone()).await;
        }

        let (status, body) = emulator.invoke(&api, row.name).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected the stage to answer {}, got {status} with {body:?}",
            row.case, row.status
        );

        match row.logged {
            Some((action, terminating)) => {
                let record = one_record(&emulator, row.case, &group).await;
                assert_eq!(
                    record["action"],
                    json!(action),
                    "case {:?}: expected the action {action:?}, got {record}",
                    row.case
                );
                assert_eq!(
                    record["terminatingRuleId"],
                    json!(terminating),
                    "case {:?}: expected the terminating rule {terminating:?}, got {record}",
                    row.case
                );
                assert!(
                    record["httpRequest"]["clientIp"].as_str().is_some(),
                    "case {:?}: the record must name the address the request came from, got \
                     {record}",
                    row.case
                );
            }
            None => {
                let delivered = emulator.delivered(&group).await;
                assert!(
                    delivered.as_ref().is_none_or(Vec::is_empty),
                    "case {:?}: expected no record in {group:?}, got {delivered:?}",
                    row.case
                );
            }
        }
    }
}

/// Reading a logging configuration back, and reading the records already delivered, change
/// nothing a later caller sees: a second invoke leaves a second record and no more.
#[tokio::test]
async fn delivering_a_record_leaves_the_configuration_and_the_earlier_records_alone() {
    let emulator = Emulator::new();
    let stage = "repeat";
    let group = "aws-waf-logs-repeat";
    let api = deployed_api(&emulator, "waf-repeat-api", &[stage]).await;
    let arn = web_acl_in_front(
        &emulator,
        "acl-repeat",
        json!({ "Allow": {} }),
        json!([rule_on_the_path("deny", json!({ "Block": {} }))]),
        &api,
        stage,
    )
    .await;
    configure_logging(
        &emulator,
        &arn,
        &group_arn(group),
        Some(vec![single_header(SECRET_HEADER)]),
        None,
    )
    .await;

    let stored = emulator
        .waf_ok("GetLoggingConfiguration", json!({ "ResourceArn": arn }))
        .await;

    for expected in 1..=3usize {
        let (status, body) = emulator.invoke(&api, stage).await;
        assert_eq!(
            status, 403,
            "case invoke {expected}: expected 403, got {status} with {body:?}"
        );
        let delivered = emulator
            .delivered(group)
            .await
            .unwrap_or_else(|| panic!("case invoke {expected}: the group was not created"));
        assert_eq!(
            delivered.len(),
            expected,
            "case invoke {expected}: one request is one record, got {delivered:?}"
        );
        for record in &delivered {
            let secret = logged_header(record, SECRET_HEADER);
            assert_eq!(
                secret, REDACTED,
                "case invoke {expected}: every record keeps the redacted header out, got \
                 {secret:?} in {record}"
            );
        }
    }

    let read_again = emulator
        .waf_ok("GetLoggingConfiguration", json!({ "ResourceArn": arn }))
        .await;
    assert_eq!(
        read_again, stored,
        "delivering records must not change the stored configuration, got {read_again}"
    );
}
