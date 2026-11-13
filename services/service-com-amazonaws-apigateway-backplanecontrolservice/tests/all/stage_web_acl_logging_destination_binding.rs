// SPDX-License-Identifier: CC0-1.0

//! The BINDING between a WAFv2 web ACL in front of a deployed REST API stage and the
//! CloudWatch Logs destination its logging configuration names, end to end over the wire.
//!
//! The rule this suite pins, one level above the recorded case:
//!
//! > A request to a stage a web ACL is associated with is always answered with the bounded
//! > status that web ACL's decision implies — the api's own `403` for a terminating block,
//! > the origin's status for anything the ACL lets through — and while the web ACL carries
//! > a logging configuration naming a CloudWatch Logs log group, EACH request it inspects
//! > leaves one more record in that group, in whichever of the two published forms of a
//! > log-group ARN the destination was named by, with every component the configuration's
//! > `RedactedFields` names written as `REDACTED`. A `PutLoggingConfiguration` whose
//! > destination name does not begin with `aws-waf-logs-` is refused with
//! > `WAFInvalidParameterException` (HTTP 400) and changes nothing: not the configuration
//! > already bound, not the records already delivered, and not what the stage serves.
//!
//! Only a cross-service suite can pin this: the api and its stage are API Gateway's, the
//! web ACL and its logging configuration are WAFv2's, and the group the record lands in is
//! CloudWatch Logs' — all three on ONE engine, as `core/router/src/lib.rs` mounts them.
//! Every delivery assertion reads the outcome back through CloudWatch Logs' own
//! `FilterLogEvents`, so a record that did not reach that store cannot pass, and the
//! refusal is read as a client reads it: the status and the body of the call.
//!
//! The rows differ along what the rule generalises over: which FORM of a log-group ARN the
//! destination is named by (the group on its own, and the group with everything under it),
//! what the web ACL DECIDED (a rule that blocked, a rule that allowed, a rule that only
//! counted, the default action), how many requests have been inspected so far, and — on
//! the refusal side — the KIND of destination and the WAY its name fails the prefix.
//!
//! The sibling [`crate::stage_web_acl_logging`] suite pins what ONE record shows; this one
//! pins that the destination the configuration named is the one that receives them, that
//! the count tracks the requests inspected, and that a refused destination leaves all of
//! it alone.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the invoke host, all three control planes and every ARN name.
const REGION: &str = "eu-west-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource.
const SCOPE: &str = "REGIONAL";

/// What a redacted component shows in a record instead of what the client sent, from
/// `LoggingConfiguration$RedactedFields`: "the `HEADER` field in the logs will be
/// `REDACTED`".
const REDACTED: &str = "REDACTED";

/// The prefix the NAME of a logging destination must begin with
/// (`smithy_query(query="doc",shape="LoggingConfiguration")`: "the name that you give the
/// destination must start with `aws-waf-logs-`").
const WAF_LOGS_PREFIX: &str = "aws-waf-logs-";

/// The resource path every case drives, and the query string it carries.
const RESOURCE_PATH: &str = "invoices";
const QUERY: &str = "page=2";

/// The header every case sends and every case redacts: a caller's credential, which is
/// exactly what a `SingleHeader` redaction is for.
const SECRET_HEADER: &str = "authorization";
const SECRET_VALUE: &str = "Bearer a-caller-token-7f3b";

/// A header no case redacts, so every record shows what the client sent under it.
const KEPT_HEADER: &str = "x-waf-keep";
const KEPT_VALUE: &str = "kept-verbatim";

/// The `ParameterExceptionField` WAF reports a destination it will not accept against
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists `LOG_DESTINATION`
/// among its values).
const FIELD_LOG_DESTINATION: &str = "LOG_DESTINATION";

/// The `ErrorReason` WAF carries for an ARN it will not accept. Spelled out here rather
/// than read off the service, so this suite pins the sentence and not merely its reuse.
const ARN_NOT_VALID: &str = "The ARN isn't valid. A valid ARN begins with arn: and includes \
     other information separated by colons or slashes.";

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

    /// Dispatch one request as it arrives on the wire, answering its status, parsed body
    /// and raw text.
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

    /// One WAFv2 call, answering its status and parsed body.
    async fn waf(&self, operation: &str, body: Value) -> (u16, Value) {
        let (status, parsed, _) = self
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
        (status, parsed)
    }

    /// One WAFv2 call that has to succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.waf(operation, body).await;
        assert_eq!(
            status, 200,
            "setup {operation}: expected HTTP 200, got {status} with {parsed}"
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

/// The two forms the ARN of ONE log group is read in, both of which name that group as a
/// destination: the group on its own, and the group together with everything under it
/// (the form `DescribeLogGroups` reports a group's `arn` in).
#[derive(Clone, Copy, Debug)]
enum GroupArnForm {
    Group,
    GroupAndEverythingUnderIt,
}

/// The ARN of the log group `name`, in `form`, as a `LogDestinationConfigs` entry names
/// one.
fn group_arn(name: &str, form: GroupArnForm) -> String {
    let group = format!(
        "arn:aws:logs:{REGION}:{account}:log-group:{name}",
        account = engine::emulator_account_id()
    );
    match form {
        GroupArnForm::Group => group,
        GroupArnForm::GroupAndEverythingUnderIt => format!("{group}:*"),
    }
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
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"),
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

/// Create a web ACL with `rules` and `default_action`, put it in front of the stage, and
/// answer its ARN.
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

/// The `PutLoggingConfiguration` request binding `destination` in front of the web ACL
/// `arn`, with the caller's credential header redacted.
fn put_request(arn: &str, destination: &str) -> Value {
    json!({
        "LoggingConfiguration": {
            "ResourceArn": arn,
            "LogDestinationConfigs": [destination],
            // Named in another case than the header arrives in, because a header is
            // named, not spelled: the redaction applies either way.
            "RedactedFields": [single_header("Authorization")],
        }
    })
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

/// Assert of every record in `delivered` that the redaction held: the credential header
/// shows `REDACTED`, the value the client sent appears nowhere in the record at all, and a
/// header no entry names shows what the client sent.
fn redaction_held(case: &str, delivered: &[Value]) {
    for record in delivered {
        let secret = logged_header(record, SECRET_HEADER);
        assert_eq!(
            secret, REDACTED,
            "case {case:?}: the redacted header must show {REDACTED:?}, got {secret:?} in \
             {record}"
        );
        let text = record.to_string();
        assert!(
            !text.contains(SECRET_VALUE),
            "case {case:?}: the value the client sent must appear nowhere in the record, got \
             {record}"
        );
        let kept = logged_header(record, KEPT_HEADER);
        assert_eq!(
            kept, KEPT_VALUE,
            "case {case:?}: a header no redaction names must show what the client sent, got \
             {kept:?} in {record}"
        );
    }
}

/// One row of the binding table: the form the destination was named by, how the web ACL
/// decides, and what that makes of the request and of the record.
struct BindingRow {
    case: &'static str,
    /// The stage, the web ACL and the destination group this row drives, all named for it.
    name: &'static str,
    /// The form the destination's ARN is written in.
    form: GroupArnForm,
    /// The web ACL's `DefaultAction`.
    default_action: Value,
    /// The rules it carries.
    rules: Value,
    /// The status the stage answers each request with.
    status: u16,
    /// The `action` the delivered record names.
    action: &'static str,
    /// Its `terminatingRuleId`.
    terminating: &'static str,
}

/// The rows: both published forms of a log-group ARN, against a rule that blocks, a rule
/// that allows, a rule that only counts (so the default action decides) and a web ACL with
/// no rules at all.
fn binding_rows() -> Vec<BindingRow> {
    vec![
        BindingRow {
            case: "a blocked request, the destination named by the group's own ARN",
            name: "blockedgroup",
            form: GroupArnForm::Group,
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path("deny-blockedgroup", json!({ "Block": {} }))]),
            status: 403,
            action: "BLOCK",
            terminating: "deny-blockedgroup",
        },
        BindingRow {
            case: "a blocked request, the destination named by the group with everything \
                   under it",
            name: "blockedwildcard",
            form: GroupArnForm::GroupAndEverythingUnderIt,
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path(
                "deny-blockedwildcard",
                json!({ "Block": {} })
            )]),
            status: 403,
            action: "BLOCK",
            terminating: "deny-blockedwildcard",
        },
        BindingRow {
            case: "a request a rule allowed through to the origin, named by the group with \
                   everything under it",
            name: "allowedwildcard",
            form: GroupArnForm::GroupAndEverythingUnderIt,
            default_action: json!({ "Block": {} }),
            rules: json!([rule_on_the_path(
                "permit-allowedwildcard",
                json!({ "Allow": {} })
            )]),
            status: 200,
            action: "ALLOW",
            terminating: "permit-allowedwildcard",
        },
        BindingRow {
            case: "a request only a counting rule matched, which the default action decided",
            name: "countedgroup",
            form: GroupArnForm::Group,
            default_action: json!({ "Allow": {} }),
            rules: json!([rule_on_the_path(
                "count-countedgroup",
                json!({ "Count": {} })
            )]),
            status: 200,
            action: "ALLOW",
            // "The action to perform if none of the Rules contained in the WebACL match":
            // a counting rule terminates nothing, so the default action decided.
            terminating: "Default_Action",
        },
        BindingRow {
            case: "a request no rule matched at all, which the default action blocked",
            name: "defaultblocked",
            form: GroupArnForm::Group,
            default_action: json!({ "Block": {} }),
            rules: json!([]),
            status: 403,
            action: "BLOCK",
            terminating: "Default_Action",
        },
    ]
}

/// The destination the configuration named receives one more record for each request the
/// web ACL inspects, whichever form of the group's ARN named it, and the stage answers
/// every one of those requests with the status the decision implies.
#[tokio::test]
async fn the_named_group_receives_one_record_for_each_inspected_request() {
    let emulator = Emulator::new();
    let rows = binding_rows();
    let stages: Vec<&str> = rows.iter().map(|row| row.name).collect();
    let api = deployed_api(&emulator, "waf-destination-api", &stages).await;

    for row in &rows {
        let group = format!("{WAF_LOGS_PREFIX}{}", row.name);
        let arn = web_acl_in_front(
            &emulator,
            &format!("acl-{}", row.name),
            row.default_action.clone(),
            row.rules.clone(),
            &api,
            row.name,
        )
        .await;
        emulator
            .waf_ok(
                "PutLoggingConfiguration",
                put_request(&arn, &group_arn(&group, row.form)),
            )
            .await;

        // Each request in turn: the stage answers it, and the group the destination named
        // — whichever form named it — holds exactly one record per request inspected so
        // far.
        for inspected in 1..=3usize {
            let (status, body) = emulator.invoke(&api, row.name).await;
            assert_eq!(
                status, row.status,
                "case {:?}: request {inspected} must be answered {}, got {status} with \
                 {body:?}",
                row.case, row.status
            );
            let delivered = emulator.delivered(&group).await.unwrap_or_else(|| {
                panic!(
                    "case {:?}: the destination group {group:?} was not created",
                    row.case
                )
            });
            assert_eq!(
                delivered.len(),
                inspected,
                "case {:?}: {inspected} inspected requests must leave {inspected} records in \
                 {group:?}, got {delivered:?}",
                row.case
            );
            for record in &delivered {
                assert_eq!(
                    record["action"],
                    json!(row.action),
                    "case {:?}: expected the action {:?}, got {record}",
                    row.case,
                    row.action
                );
                assert_eq!(
                    record["terminatingRuleId"],
                    json!(row.terminating),
                    "case {:?}: expected the terminating rule {:?}, got {record}",
                    row.case,
                    row.terminating
                );
                assert_eq!(
                    record["webaclId"],
                    json!(arn),
                    "case {:?}: the record must name the web ACL that decided, got {record}",
                    row.case
                );
                assert_eq!(
                    record["httpRequest"]["uri"],
                    json!(format!("/{}/{RESOURCE_PATH}", row.name)),
                    "case {:?}: the record must name the path the request asked for, got \
                     {record}",
                    row.case
                );
            }
            redaction_held(row.case, &delivered);
        }
    }
}

/// One destination a put may not name, and what it is.
struct RefusedDestination {
    case: &'static str,
    destination: String,
    /// The log group the refused destination named, when it named one at all: no such
    /// group may be created by a put that was refused.
    group: Option<String>,
}

/// The rows: a log group named by either form of its ARN, the prefix present but not at
/// the start, the prefix in another case, destinations of the two other kinds WAF
/// delivers to, and a value that is no ARN at all.
fn refused_destinations() -> Vec<RefusedDestination> {
    vec![
        RefusedDestination {
            case: "a log group whose name lacks the prefix, by the group's own ARN",
            destination: group_arn("audit-trail", GroupArnForm::Group),
            group: Some("audit-trail".to_string()),
        },
        RefusedDestination {
            case: "the same group with everything under it",
            destination: group_arn("audit-trail", GroupArnForm::GroupAndEverythingUnderIt),
            group: Some("audit-trail".to_string()),
        },
        RefusedDestination {
            case: "a log group carrying the prefix, but not at the start of its name",
            destination: group_arn("team-aws-waf-logs-audit", GroupArnForm::Group),
            group: Some("team-aws-waf-logs-audit".to_string()),
        },
        RefusedDestination {
            case: "a log group carrying the prefix in another case",
            destination: group_arn("AWS-WAF-LOGS-audit", GroupArnForm::Group),
            group: Some("AWS-WAF-LOGS-audit".to_string()),
        },
        RefusedDestination {
            case: "an Amazon S3 bucket whose name lacks the prefix",
            destination: "arn:aws:s3:::waf-audit-bucket".to_string(),
            group: None,
        },
        RefusedDestination {
            case: "a Firehose delivery stream whose name lacks the prefix",
            destination: format!(
                "arn:aws:firehose:{REGION}:{}:deliverystream/waf-audit-stream",
                engine::emulator_account_id()
            ),
            group: None,
        },
        RefusedDestination {
            case: "a destination that is not an ARN at all",
            destination: "waf-audit-destination".to_string(),
            group: None,
        },
    ]
}

/// A put naming a destination without the prefix is refused with the parameter failure and
/// changes nothing: the configuration already bound is read back as it was, the stage keeps
/// answering, the records keep arriving in the group that WAS named, and no group the
/// refused destination named is created.
#[tokio::test]
async fn a_destination_without_the_prefix_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let stage = "refused";
    let group = format!("{WAF_LOGS_PREFIX}{stage}");
    let api = deployed_api(&emulator, "waf-refused-api", &[stage]).await;
    let arn = web_acl_in_front(
        &emulator,
        "acl-refused",
        json!({ "Allow": {} }),
        json!([rule_on_the_path("deny-refused", json!({ "Block": {} }))]),
        &api,
        stage,
    )
    .await;
    emulator
        .waf_ok(
            "PutLoggingConfiguration",
            put_request(&arn, &group_arn(&group, GroupArnForm::Group)),
        )
        .await;
    let stored = emulator
        .waf_ok("GetLoggingConfiguration", json!({ "ResourceArn": arn }))
        .await;

    let (status, body) = emulator.invoke(&api, stage).await;
    assert_eq!(
        status, 403,
        "a blocked request must be answered 403 at the stage, got {status} with {body:?}"
    );

    for (refused, index) in refused_destinations().iter().zip(1usize..) {
        let (status, body) = emulator
            .waf("PutLoggingConfiguration", put_request(&arn, &refused.destination))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for {:?}, got {status} with {body}",
            refused.case, refused.destination
        );
        assert_eq!(
            body["__type"],
            json!("WAFInvalidParameterException"),
            "case {:?}: expected WAFInvalidParameterException, got {body}",
            refused.case
        );
        assert_eq!(
            body["Field"],
            json!(FIELD_LOG_DESTINATION),
            "case {:?}: the refusal must name the field it is about, got {body}",
            refused.case
        );
        assert_eq!(
            body["Parameter"],
            json!(refused.destination),
            "case {:?}: the refusal must echo the destination it refused, got {body}",
            refused.case
        );
        assert_eq!(
            body["Reason"],
            json!(ARN_NOT_VALID),
            "case {:?}: expected the ARN reason, got {body}",
            refused.case
        );
        assert_eq!(
            body["message"],
            json!(format!(
                "Error reason: {ARN_NOT_VALID}, field: {FIELD_LOG_DESTINATION}, parameter: {}",
                refused.destination
            )),
            "case {:?}: the message is composed from Reason, Field and Parameter, got {body}",
            refused.case
        );

        // The configuration already bound is untouched by the refusal.
        let read_again = emulator
            .waf_ok("GetLoggingConfiguration", json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            read_again, stored,
            "case {:?}: a refused put must leave the stored configuration alone, got \
             {read_again}",
            refused.case
        );

        // No group the refused destination named exists: a put WAF would never have
        // accepted provisions nothing.
        if let Some(never_created) = refused.group.as_deref() {
            let delivered = emulator.delivered(never_created).await;
            assert!(
                delivered.is_none(),
                "case {:?}: a refused put must create no group {never_created:?}, got \
                 {delivered:?}",
                refused.case
            );
        }

        // And the stage keeps enforcing and keeps logging to the destination that WAS
        // named: one more request, one more record there.
        let (status, text) = emulator.invoke(&api, stage).await;
        assert_eq!(
            status, 403,
            "case {:?}: the stage must keep answering 403 after the refusal, got {status} \
             with {text:?}",
            refused.case
        );
        let delivered = emulator
            .delivered(&group)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the destination group is gone", refused.case));
        assert_eq!(
            delivered.len(),
            index + 1,
            "case {:?}: the records already delivered must stay and the new one join them, \
             got {delivered:?}",
            refused.case
        );
        redaction_held(refused.case, &delivered);
    }
}
