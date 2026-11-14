// SPDX-License-Identifier: CC0-1.0

//! What a WAFv2 web ACL in front of a deployed REST API stage REPORTS to CloudWatch about
//! the requests it decided, end to end over the wire.
//!
//! The rule this suite pins, one level above the recorded case:
//!
//! > A request to a deployed stage with a web ACL associated is INSPECTED by that web ACL
//! > and is always answered with a bounded HTTP response — a terminating action settles it
//! > at the edge, a non-terminating `Count` leaves it to the origin — and every dimension
//! > that acted on it whose `VisibilityConfig$CloudWatchMetricsEnabled` is true counts it
//! > as ONE observation of 1, with no unit of measure, in the `AWS/WAFV2` namespace under
//! > the `WebACL`, `Rule` and `Region` dimensions TOGETHER, so the `Sum` a client reads
//! > over the window is exactly how many requests that dimension acted on; a dimension
//! > whose flag is false enforces identically and reports no datapoint at all.
//!
//! Only a cross-service suite can pin that: the api and its stage are API Gateway's, the
//! web ACL and the inspection are WAFv2's, and the series live in the CloudWatch
//! emulator's own metric store — the one `ListMetrics` and `GetMetricStatistics` read —
//! all three mounted on ONE engine, as `core/router/src/lib.rs` mounts them. Every
//! assertion reads the outcome back through CloudWatch's own readers, so a counter that
//! never reached that store cannot pass.
//!
//! The rows differ along what the rule generalises over: which action the matching rule
//! applies (`Block`, `Count`, `Allow`, `Captcha`, `Challenge`), whether that action
//! terminates the evaluation, whether the dimension's `CloudWatchMetricsEnabled` is on or
//! off for a terminating and for a non-terminating rule, how many requests each row drives
//! (a different number per row, so no row's count can stand in for another's), and a path
//! no rule matches at all, which the web ACL's `DefaultAction` decides under the web ACL's
//! own dimension.
//!
//! Every request is asserted to be ANSWERED within [`ANSWER_DEADLINE`], so a stage that
//! leaves a request open fails here instead of hanging the suite.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the invoke host, all three control planes and every ARN name.
const REGION: &str = "us-east-1";

/// A Region nothing here is created in, used to show that the `Region` dimension is part of
/// a series' identity rather than decoration.
const OTHER_REGION: &str = "eu-central-1";

/// The `Scope` an API Gateway stage's web ACL lives in: a stage is a REGIONAL resource.
const SCOPE: &str = "REGIONAL";

/// The stage every api below is deployed to.
const STAGE: &str = "prod";

/// CloudWatch's `X-Amz-Target` prefix and its signing name (`sigv4Name: monitoring`), which
/// is what routes a call to it.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The namespace WAF reports what a web ACL did with web requests in, and the three
/// dimensions one of its series is identified by. Recorded AWS behaviour: the WAF metric
/// pipeline is in no Smithy model, and `VisibilityConfig$MetricName` is the modelled switch
/// that names the `Rule` dimension's value.
const NAMESPACE: &str = "AWS/WAFV2";
const WEB_ACL_DIMENSION: &str = "WebACL";
const RULE_DIMENSION: &str = "Rule";
const REGION_DIMENSION: &str = "Region";

/// Every counter this namespace holds, one per action a dimension can apply to a request.
/// A row names the ONE its requests land in; every other must stay empty for that
/// dimension.
const ALLOWED_REQUESTS: &str = "AllowedRequests";
const BLOCKED_REQUESTS: &str = "BlockedRequests";
const COUNTED_REQUESTS: &str = "CountedRequests";
const CAPTCHA_REQUESTS: &str = "CaptchaRequests";
const CHALLENGE_REQUESTS: &str = "ChallengeRequests";
const EVERY_COUNTER: [&str; 5] = [
    ALLOWED_REQUESTS,
    BLOCKED_REQUESTS,
    COUNTED_REQUESTS,
    CAPTCHA_REQUESTS,
    CHALLENGE_REQUESTS,
];

/// The `Unit` a datapoint of these counters carries: a counter of requests measures no
/// quantity, which is the value `StandardUnit` declares for a measurement that has none.
const NO_UNIT: &str = "None";

/// The period the series are read at, in seconds, and the span either side of the traffic a
/// read covers — generous, because the only thing that decides which window a datapoint
/// falls in is the instant the request was evaluated at.
const PERIOD: i64 = 60;

/// How long ONE request to the deployed stage is given to be answered. A request still open
/// after this was never going to be answered at all — which is the symptom this suite pins,
/// so it is asserted rather than waited out.
const ANSWER_DEADLINE: Duration = Duration::from_secs(60);

/// What the stage's `MOCK` integration answers, so the body a request gets says whether it
/// reached the origin.
const ORIGIN_BODY: &str = r#"{"served":"origin"}"#;
const ORIGIN_MARK: &str = "origin";

/// The `VisibilityConfig$MetricName` the WEB ACL's own dimension carries, which is the one
/// the requests its `DefaultAction` decided are counted under.
const WEB_ACL_METRIC: &str = "webAclDefault";

/// One emulator with API Gateway, WAFv2 and CloudWatch mounted on a single shared engine.
struct Front {
    router: MultiServiceRouter,
}

impl Front {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises the services, not SigV4: the control-plane calls carry a
        // credential scope so the router can name the target service, and the invokes carry
        // no credential at all, as a client of a published invoke URL sends none.
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
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name` in `REGION`,
    /// which is what routes the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// Dispatch one request as it arrives on the wire, answering its status, its parsed
    /// body and that body verbatim.
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

    /// One WAFv2 call (`awsJson1_1`) that has to succeed.
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

    /// One CloudWatch call (`awsJson1_0`) that has to succeed.
    async fn cloudwatch(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, parsed, text) = self
            .dispatch(
                "POST",
                &format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.0"),
                    (
                        "authorization",
                        &Self::authorization(CLOUDWATCH_SIGNING_NAME),
                    ),
                    (
                        "x-amz-target",
                        &format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
                    ),
                ],
                Bytes::from(body.to_string()),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {text:?}"
        );
        parsed
    }

    /// One unsigned GET to a deployed stage's invoke URL — the api's own host, no
    /// credential, as a client of a published invoke URL sends it.
    ///
    /// The request must be ANSWERED: a stage that leaves it open fails the case here.
    async fn get(&self, case: &str, api: &str, path: &str) -> (u16, String) {
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let answered = tokio::time::timeout(
            ANSWER_DEADLINE,
            self.dispatch("GET", &host, path, &[], Bytes::new()),
        )
        .await;
        let (status, _, text) = answered.unwrap_or_else(|_| {
            panic!(
                "case {case:?}: GET {path} must be answered by the stage within \
                 {ANSWER_DEADLINE:?}, got no HTTP response at all"
            )
        });
        (status, text)
    }

    /// Read ONE series over the whole window and fold what it holds.
    async fn held(&self, case: &str, series: &Series<'_>, at: f64) -> Held {
        let answered = self
            .cloudwatch(
                case,
                "GetMetricStatistics",
                json!({
                    "Namespace": NAMESPACE,
                    "MetricName": series.metric,
                    "Dimensions": series.dimensions(),
                    "StartTime": at - 2.0 * PERIOD as f64,
                    "EndTime": at + 2.0 * PERIOD as f64,
                    "Period": PERIOD,
                    "Statistics": ["Sum", "SampleCount"],
                }),
            )
            .await;
        let mut folded = Held::default();
        for datapoint in answered["Datapoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            folded.datapoints += 1;
            folded.sum += datapoint["Sum"].as_f64().unwrap_or_default();
            folded.samples += datapoint["SampleCount"].as_f64().unwrap_or_default();
            if let Some(unit) = datapoint["Unit"].as_str() {
                folded.units.insert(unit.to_string());
            }
        }
        folded
    }
}

/// One series of this namespace, as a reader names it: the counter, and the three
/// dimensions that identify it together.
struct Series<'a> {
    web_acl: &'a str,
    rule_metric: &'a str,
    region: &'a str,
    metric: &'a str,
}

impl Series<'_> {
    fn dimensions(&self) -> Value {
        json!([
            { "Name": WEB_ACL_DIMENSION, "Value": self.web_acl },
            { "Name": RULE_DIMENSION, "Value": self.rule_metric },
            { "Name": REGION_DIMENSION, "Value": self.region },
        ])
    }
}

/// What one series holds over the window: how many datapoints a client reads, what they sum
/// to, how many observations they aggregate, and which units they carry.
#[derive(Debug, Default)]
struct Held {
    datapoints: usize,
    sum: f64,
    samples: f64,
    units: BTreeSet<String>,
}

/// One row: a path, the rule the web ACL carries for it, how many requests the row drives,
/// and what each of them must be answered with and counted into.
struct Row {
    case: &'static str,
    /// The path the row drives, and the suffix its rule matches on.
    path: &'static str,
    /// The rule's `Action` — the single member of `RuleAction` it names — or `None` for the
    /// row that matches no rule at all.
    action: Option<&'static str>,
    /// The rule's `VisibilityConfig$MetricName`, which is the `Rule` dimension's value.
    rule_metric: Option<&'static str>,
    /// The rule's `VisibilityConfig$CloudWatchMetricsEnabled`.
    metrics_enabled: bool,
    /// How many requests this row drives.
    requests: usize,
    /// The HTTP status each of them must be answered with.
    status: u16,
    /// The counter each of them must land in, or `None` when the dimension reports nothing.
    counter: Option<&'static str>,
    /// Whether the evaluation goes ON to the `DefaultAction` after this row's rule: a
    /// `Count` rule does not terminate the request, and a path no rule matches reaches the
    /// default action directly, so the web ACL's own dimension counts both.
    falls_through: bool,
}

/// The table: one row per action a rule can apply, the two `CloudWatchMetricsEnabled: false`
/// rows (one terminating, one not), and a path no rule matches. The statuses are the ones
/// each action settles a request with at the edge — 403 for a terminating `Block`, 200 from
/// the origin for a request no rule terminated, and the status WAF serves its CAPTCHA
/// (`405`) and silent challenge (`202`) interstitials under.
const ROWS: [Row; 8] = [
    Row {
        case: "a terminating Block rule with metrics on",
        path: "/blocked",
        action: Some("Block"),
        rule_metric: Some("blockedRule"),
        metrics_enabled: true,
        requests: 7,
        status: 403,
        counter: Some(BLOCKED_REQUESTS),
        falls_through: false,
    },
    Row {
        case: "a Count rule with metrics on",
        path: "/counted",
        action: Some("Count"),
        rule_metric: Some("countedRule"),
        metrics_enabled: true,
        requests: 3,
        status: 200,
        counter: Some(COUNTED_REQUESTS),
        falls_through: true,
    },
    Row {
        case: "a terminating Block rule with metrics off",
        path: "/quiet",
        action: Some("Block"),
        rule_metric: Some("quietRule"),
        metrics_enabled: false,
        requests: 5,
        status: 403,
        counter: None,
        falls_through: false,
    },
    Row {
        case: "a Count rule with metrics off",
        path: "/hushed",
        action: Some("Count"),
        rule_metric: Some("hushedRule"),
        metrics_enabled: false,
        requests: 6,
        status: 200,
        counter: None,
        falls_through: true,
    },
    Row {
        case: "a terminating Allow rule with metrics on",
        path: "/allowed",
        action: Some("Allow"),
        rule_metric: Some("allowedRule"),
        metrics_enabled: true,
        requests: 2,
        status: 200,
        counter: Some(ALLOWED_REQUESTS),
        falls_through: false,
    },
    Row {
        case: "a Captcha rule with metrics on",
        path: "/puzzle",
        action: Some("Captcha"),
        rule_metric: Some("puzzleRule"),
        metrics_enabled: true,
        requests: 4,
        status: 405,
        counter: Some(CAPTCHA_REQUESTS),
        falls_through: false,
    },
    Row {
        case: "a Challenge rule with metrics on",
        path: "/silent",
        action: Some("Challenge"),
        rule_metric: Some("silentRule"),
        metrics_enabled: true,
        requests: 1,
        status: 202,
        counter: Some(CHALLENGE_REQUESTS),
        falls_through: false,
    },
    Row {
        case: "a path no rule matches at all",
        path: "/elsewhere",
        action: None,
        rule_metric: None,
        metrics_enabled: false,
        requests: 8,
        status: 200,
        counter: None,
        falls_through: true,
    },
];

/// The ARN `AssociateWebACL` binds a web ACL to a REST API stage by, the one form
/// `AssociateWebACLRequest$ResourceArn` documents for an API Gateway REST API stage.
fn stage_arn(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// Wall-clock now as epoch seconds, the clock the series' windows are measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// A `VisibilityConfig` naming `metric`, with the CloudWatch switch as `enabled` says.
fn visibility(metric: &str, enabled: bool) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": enabled,
        "MetricName": metric,
    })
}

/// The rule one row asks the web ACL to carry: its action applied to a request whose URI
/// path — `/{stage}/{resource-path}`, the path AWS's edge inspects — ends in the row's own
/// path.
fn rule_of(row: &Row, priority: usize) -> Option<Value> {
    let (action, metric) = (row.action?, row.rule_metric?);
    Some(json!({
        "Name": metric,
        "Priority": priority,
        "Statement": {
            "ByteMatchStatement": {
                "SearchString": base64_of(row.path),
                "FieldToMatch": { "UriPath": {} },
                "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                "PositionalConstraint": "ENDS_WITH",
            }
        },
        "Action": { action: {} },
        "VisibilityConfig": visibility(metric, row.metrics_enabled),
    }))
}

/// An api with a `{proxy+}` catch-all on `ANY`, whose `MOCK` integration answers `200`
/// with [`ORIGIN_BODY`], deployed to [`STAGE`]. Answers the api's id.
async fn deployed_api(front: &Front, name: &str) -> String {
    let created = front
        .api_ok("POST", "/restapis", Some(json!({ "name": name })), 201)
        .await;
    let api = created["id"].as_str().expect("the api id").to_string();
    let root = created["rootResourceId"]
        .as_str()
        .expect("the root resource id")
        .to_string();
    let proxy = front
        .api_ok(
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": "{proxy+}" })),
            201,
        )
        .await;
    let proxy = proxy["id"].as_str().expect("the resource id").to_string();
    let method = format!("/restapis/{api}/resources/{proxy}/methods/ANY");
    front
        .api_ok(
            "PUT",
            &method,
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
    front
        .api_ok(
            "PUT",
            &format!("{method}/integration"),
            Some(json!({ "type": "MOCK" })),
            201,
        )
        .await;
    front
        .api_ok("PUT", &format!("{method}/responses/200"), Some(json!({})), 201)
        .await;
    front
        .api_ok(
            "PUT",
            &format!("{method}/integration/responses/200"),
            Some(json!({ "responseTemplates": { "application/json": ORIGIN_BODY } })),
            201,
        )
        .await;
    front
        .api_ok(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
            201,
        )
        .await;
    api
}

/// A web ACL named `name` carrying the rule of every row that has one, with its
/// `DefaultAction` allowing and its OWN dimension sending counters, put in front of `api`'s
/// stage.
async fn web_acl_in_front(front: &Front, name: &str, api: &str) {
    let rules: Vec<Value> = ROWS
        .iter()
        .enumerate()
        .filter_map(|(priority, row)| rule_of(row, priority))
        .collect();
    let created = front
        .waf_ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(WEB_ACL_METRIC, true),
                "Rules": rules,
            }),
        )
        .await;
    let arn = created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    front
        .waf_ok(
            "AssociateWebACL",
            json!({ "WebACLArn": arn, "ResourceArn": stage_arn(api, STAGE) }),
        )
        .await;
}

/// Drive `row`'s requests at `api`'s stage, asserting each one is answered with the status
/// its action settles on — and that a request no rule terminated reached the origin.
async fn drive(front: &Front, api: &str, row: &Row) {
    for ordinal in 1..=row.requests {
        let path = format!("/{STAGE}{}", row.path);
        let (status, text) = front.get(row.case, api, &path).await;
        assert_eq!(
            status, row.status,
            "case {:?} request {ordinal} of {}: expected HTTP {}, got {status} with {text:?}",
            row.case, row.requests, row.status
        );
        if row.status == 200 {
            assert!(
                text.contains(ORIGIN_MARK),
                "case {:?} request {ordinal} of {}: a request the web ACL did not terminate \
                 must be answered by the origin, got {text:?}",
                row.case,
                row.requests
            );
        }
    }
}

#[tokio::test]
async fn every_request_a_web_acl_decides_at_a_stage_is_answered_and_counted_into_its_own_series() {
    let front = Front::new();
    let web_acl = "stage-metrics-acl";
    let api = deployed_api(&front, "stage-metrics-api").await;
    web_acl_in_front(&front, web_acl, &api).await;

    let at = now_secs();
    for row in &ROWS {
        drive(&front, &api, row).await;
    }

    // Each row's OWN dimension: the counter its action lands in holds one observation of 1
    // per request that dimension acted on, with no unit of measure, and every other counter
    // of the namespace is empty for it — which is what a row with
    // `CloudWatchMetricsEnabled: false` reports for all five, its 403s or its 200s
    // notwithstanding.
    for row in &ROWS {
        let Some(rule_metric) = row.rule_metric else {
            continue;
        };
        for counter in EVERY_COUNTER {
            let series = Series {
                web_acl,
                rule_metric,
                region: REGION,
                metric: counter,
            };
            let case = format!("{} read as {counter}", row.case);
            let held = front.held(&case, &series, at).await;
            let expected = if row.counter == Some(counter) {
                row.requests as f64
            } else {
                0.0
            };
            if expected == 0.0 {
                assert_eq!(
                    held.datapoints, 0,
                    "case {case:?}: expected no datapoint at all, got {held:?}"
                );
                continue;
            }
            assert_eq!(
                held.sum, expected,
                "case {case:?}: expected Sum {expected} — one observation per request the \
                 rule acted on — got {held:?}"
            );
            assert_eq!(
                held.samples, expected,
                "case {case:?}: expected SampleCount {expected}, got {held:?}"
            );
            assert_eq!(
                held.units,
                BTreeSet::from([NO_UNIT.to_string()]),
                "case {case:?}: expected every datapoint to carry the unit {NO_UNIT:?}, got \
                 {held:?}"
            );
        }
    }

    // The WEB ACL's own dimension holds the requests its `DefaultAction` decided: every row
    // whose rule did not terminate the request, plus the row no rule matched.
    let decided_by_default: f64 = ROWS
        .iter()
        .filter(|row| row.falls_through)
        .map(|row| row.requests as f64)
        .sum();
    for counter in EVERY_COUNTER {
        let series = Series {
            web_acl,
            rule_metric: WEB_ACL_METRIC,
            region: REGION,
            metric: counter,
        };
        let case = format!("the web ACL's own dimension read as {counter}");
        let held = front.held(&case, &series, at).await;
        if counter != ALLOWED_REQUESTS {
            assert_eq!(
                held.datapoints, 0,
                "case {case:?}: the DefaultAction allows, so no other counter of this \
                 dimension holds anything, got {held:?}"
            );
            continue;
        }
        assert_eq!(
            held.sum, decided_by_default,
            "case {case:?}: expected Sum {decided_by_default} — every request no rule \
             terminated — got {held:?}"
        );
        assert_eq!(
            held.units,
            BTreeSet::from([NO_UNIT.to_string()]),
            "case {case:?}: expected every datapoint to carry the unit {NO_UNIT:?}, got \
             {held:?}"
        );
    }
}

#[tokio::test]
async fn a_series_is_identified_by_the_web_acl_the_rule_and_the_region_together() {
    let front = Front::new();
    let web_acl = "stage-identity-acl";
    let api = deployed_api(&front, "stage-identity-api").await;
    web_acl_in_front(&front, web_acl, &api).await;

    let at = now_secs();
    for row in &ROWS {
        drive(&front, &api, row).await;
    }

    // `ListMetrics` reports one series per dimension that acted, each carrying the web ACL,
    // the rule and the Region TOGETHER — which is what lets a reader filtering on the web
    // ACL find the per-rule series beneath it.
    let listed = front
        .cloudwatch(
            "listed for one web ACL",
            "ListMetrics",
            json!({
                "Namespace": NAMESPACE,
                "Dimensions": [{ "Name": WEB_ACL_DIMENSION, "Value": web_acl }],
            }),
        )
        .await;
    let reported = listed["Metrics"].as_array().cloned().unwrap_or_default();
    assert!(
        !reported.is_empty(),
        "case listed for one web ACL: expected the series its traffic produced, got {listed}"
    );
    let mut rules_reported: BTreeSet<String> = BTreeSet::new();
    for metric in &reported {
        let dimensions = metric["Dimensions"].as_array().cloned().unwrap_or_default();
        let names: BTreeSet<String> = dimensions
            .iter()
            .filter_map(|dimension| dimension["Name"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            names,
            BTreeSet::from([
                WEB_ACL_DIMENSION.to_string(),
                RULE_DIMENSION.to_string(),
                REGION_DIMENSION.to_string(),
            ]),
            "case listed for one web ACL: expected the three dimensions together, got {metric}"
        );
        let value_of = |name: &str| -> Option<String> {
            dimensions
                .iter()
                .find(|dimension| dimension["Name"].as_str() == Some(name))
                .and_then(|dimension| dimension["Value"].as_str())
                .map(str::to_string)
        };
        assert_eq!(
            value_of(WEB_ACL_DIMENSION).as_deref(),
            Some(web_acl),
            "case listed for one web ACL: the filter must report only this web ACL's series, \
             got {metric}"
        );
        assert_eq!(
            value_of(REGION_DIMENSION).as_deref(),
            Some(REGION),
            "case listed for one web ACL: a REGIONAL web ACL's series are reported for its \
             own Region, got {metric}"
        );
        if let Some(rule) = value_of(RULE_DIMENSION) {
            rules_reported.insert(rule);
        }
    }

    // Exactly the dimensions that acted AND asked for counters: every row with metrics on,
    // plus the web ACL's own dimension. A row with the flag off is in no series at all,
    // while its own enforcement is already proven above.
    let mut expected_rules: BTreeSet<String> = ROWS
        .iter()
        .filter(|row| row.metrics_enabled)
        .filter_map(|row| row.rule_metric.map(str::to_string))
        .collect();
    expected_rules.insert(WEB_ACL_METRIC.to_string());
    assert_eq!(
        rules_reported, expected_rules,
        "case listed for one web ACL: expected the series of the dimensions that acted with \
         metrics on, got {rules_reported:?}"
    );

    // The Region is part of a series' IDENTITY: the same web ACL and rule read for another
    // Region is another series, and it holds nothing.
    let elsewhere = Series {
        web_acl,
        rule_metric: "blockedRule",
        region: OTHER_REGION,
        metric: BLOCKED_REQUESTS,
    };
    let held = front.held("read for another Region", &elsewhere, at).await;
    assert_eq!(
        held.datapoints, 0,
        "case read for another Region: this web ACL's series must not be reported for \
         {OTHER_REGION}, got {held:?}"
    );
}

#[tokio::test]
async fn a_stage_with_no_web_acl_in_front_of_it_is_served_and_counts_nothing() {
    let front = Front::new();
    let api = deployed_api(&front, "stage-unprotected-api").await;

    // Every path the table drives is served by the origin, because nothing inspects these
    // requests...
    for row in &ROWS {
        let path = format!("/{STAGE}{}", row.path);
        let (status, text) = front.get(row.case, &api, &path).await;
        assert_eq!(
            status, 200,
            "case {:?} with no web ACL in front of the stage: expected HTTP 200 from the \
             origin, got {status} with {text:?}",
            row.case
        );
    }

    // ...and the namespace holds no series at all: these counters exist because a web ACL
    // decided traffic, not because a stage was invoked.
    let listed = front
        .cloudwatch(
            "no web ACL in front of the stage",
            "ListMetrics",
            json!({ "Namespace": NAMESPACE }),
        )
        .await;
    assert_eq!(
        listed["Metrics"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case no web ACL in front of the stage: expected no series in {NAMESPACE}, got \
         {listed}"
    );
}
