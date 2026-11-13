// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 web ACL in front of an Application Load Balancer, end to end over the wire.
//!
//! The rule under test: **a request a protected resource's own endpoint serves is decided
//! FIRST by the web ACL `AssociateWebACL` bound to that resource's ARN — a terminating
//! action other than `Allow` answers the request at the edge, with the action's own
//! `CustomResponse` where it configures one and otherwise under the status WAF serves that
//! action under, and none of the resource's own actions runs; a request the web ACL allows
//! is served by the resource, carrying the headers any `CustomRequestHandling` inserted.**
//!
//! Here that resource is a load balancer and its own answer is its listener's action, so
//! each row below sends the SAME kind of request a client sends to the published
//! `DNSName` and asserts what comes back: the listener's own fixed response for a request
//! the web ACL let through, and WAF's refusal for one it did not.
//!
//! The rows span what the rule generalises over: the request component the blocking rule
//! inspects (a single header, the URI path, the query string, the method), a matching and
//! a non-matching value for each, the terminating action (`Block`, `Captcha`,
//! `Challenge`), a block configured with its own custom response and one without, a rule
//! whose action is not terminating at all, the web ACL's `DefaultAction` deciding instead
//! of a rule, an `Allow` that inserts a header the listener's own rules then see, and no
//! web ACL at all. Two further tests pin that the binding is what enforces: taking it away
//! stops the enforcement, and a web ACL bound to ANOTHER resource decides nothing here.
//!
//! Every call goes through the router's front door — the control plane over awsQuery and
//! awsJson1_1, the traffic as a credential-less HTTP request — so what these tests observe
//! is what a client observes.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{create_load_balancer, send, settle, Answered};
use crate::provisioning_loop::{action, call, p, REGION};

/// The body the listener's DEFAULT action answers with: the request reached the load
/// balancer's own configuration.
const SERVED_BODY: &str = "ok";

/// The body the listener's one RULE answers with, which it does only for a request
/// carrying the header a web ACL's `CustomRequestHandling` inserts — so an answer of this
/// body is proof the inserted header reached the listener's own rule evaluation.
const TAGGED_BODY: &str = "tagged";

/// The name of the header the inserting web ACL configures, WITHOUT the
/// `x-amzn-waf-` prefix WAF puts on it ("for the header name `sample`, WAF inserts the
/// header `x-amzn-waf-sample`" — `CustomHTTPHeader$Name`).
pub(crate) const INSERTED_HEADER: &str = "tag";

/// The header name the listener's rule matches on: the inserted header as WAF spells it on
/// the request.
pub(crate) const INSERTED_HEADER_SENT: &str = "x-amzn-waf-tag";

/// The value the inserting web ACL gives that header.
pub(crate) const INSERTED_VALUE: &str = "edge";

/// A router carrying the load balancer service AND the WAFv2 service over ONE engine —
/// what the production router mounts, minus the other services. Both are needed: the
/// binding is WAFv2's record and the traffic is this service's.
fn emulator_with_waf() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// POST one awsJson1_1 WAFv2 call and answer its status and parsed body.
async fn waf_call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLEWAFV2/20260101/{REGION}/wafv2/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=00"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://wafv2.example.com/")
        .header("host", "wafv2.example.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(response.body()).into_owned())
        })
    };
    (status, parsed)
}

/// One WAFv2 call that must succeed.
pub(crate) async fn waf_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = waf_call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got status {status} and body {parsed}"
    );
    parsed
}

/// A minimal `VisibilityConfig`, with sampling on so the data plane's own sample is drawn.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// `ByteMatchStatement$SearchString` is a blob, which travels base64-encoded in an
/// awsJson body.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// A `ByteMatchStatement` over `field`, searching for `search` under `constraint`.
pub(crate) fn byte_match(field: Value, search: &str, constraint: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(search),
            "FieldToMatch": field,
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": constraint,
        }
    })
}

/// One rule at priority 0 carrying `action` over `statement`.
pub(crate) fn rule(name: &str, statement: Value, rule_action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": statement,
        "Action": rule_action,
        "VisibilityConfig": visibility(name),
    })
}

/// The web ACL one row puts in front of the load balancer.
pub(crate) struct WebAcl {
    /// `WebACL$DefaultAction`.
    pub(crate) default_action: Value,
    /// `WebACL$Rules`.
    pub(crate) rules: Value,
    /// `WebACL$CustomResponseBodies`, for a row whose block names one; left off the
    /// request where the row configures none (the member takes at least one entry).
    pub(crate) custom_response_bodies: Option<Value>,
}

impl WebAcl {
    /// A web ACL whose default action allows, carrying `rules`.
    pub(crate) fn allowing(rules: Value) -> Self {
        Self {
            default_action: json!({ "Allow": {} }),
            rules,
            custom_response_bodies: None,
        }
    }
}

/// Create the web ACL `spec` describes and answer its ARN.
pub(crate) async fn create_web_acl(router: &MultiServiceRouter, name: &str, spec: &WebAcl) -> String {
    let mut body = json!({
        "Name": name,
        "Scope": "REGIONAL",
        "DefaultAction": spec.default_action,
        "VisibilityConfig": visibility(name),
        "Rules": spec.rules,
    });
    if let Some(bodies) = &spec.custom_response_bodies {
        body["CustomResponseBodies"] = bodies.clone();
    }
    let created = waf_ok(router, "CreateWebACL", body).await;
    created["Summary"]["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebACL must answer Summary.ARN, got {created}"))
        .to_string()
}

/// The HTTP:80 listener every row is answered by when the web ACL lets the request
/// through: a default fixed response of 200 [`SERVED_BODY`], and one rule that answers
/// [`TAGGED_BODY`] for a request carrying the header a web ACL inserts.
async fn create_listener(router: &MultiServiceRouter, lb_arn: &str) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        "200",
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.MessageBody",
        SERVED_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create listener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let listener_arn = reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener: no ListenerArn in {}", reply.body));

    let mut form = action("CreateRule");
    form.push(p("ListenerArn", &listener_arn));
    form.push(p("Priority", "10"));
    form.push(p("Conditions.member.1.Field", "http-header"));
    form.push(p(
        "Conditions.member.1.HttpHeaderConfig.HttpHeaderName",
        INSERTED_HEADER_SENT,
    ));
    form.push(p(
        "Conditions.member.1.HttpHeaderConfig.Values.member.1",
        INSERTED_VALUE,
    ));
    form.push(p("Actions.member.1.Type", "fixed-response"));
    form.push(p(
        "Actions.member.1.FixedResponseConfig.StatusCode",
        "200",
    ));
    form.push(p(
        "Actions.member.1.FixedResponseConfig.MessageBody",
        TAGGED_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create rule: expected 200, got {} body {}",
        reply.status, reply.body
    );
    listener_arn
}

/// Everything a row drives: one load balancer, active, with that listener.
struct Deployment {
    router: MultiServiceRouter,
    lb_arn: String,
    dns_name: String,
}

/// One load balancer the clock already owes its move to `active`, with the listener above.
async fn deployment(name: &str) -> Deployment {
    let (router, engine) = emulator_with_waf();
    let (lb_arn, dns_name) = create_load_balancer(&router, name).await;
    create_listener(&router, &lb_arn).await;
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));
    Deployment {
        router,
        lb_arn,
        dns_name,
    }
}

/// One row: the web ACL in front of the load balancer (none at all for `web_acl: None`),
/// the request the client sends, and what must come back.
struct Row {
    case: &'static str,
    web_acl: Option<WebAcl>,
    method: &'static str,
    target: &'static str,
    headers: &'static [(&'static str, &'static str)],
    status: u16,
    body: &'static str,
    /// Response headers the answer must carry, for a row whose refusal configures some.
    response_headers: &'static [(&'static str, &'static str)],
}

/// A row whose request is a plain `GET /`, as the recorded client sends it.
fn row(case: &'static str, web_acl: Option<WebAcl>, status: u16, body: &'static str) -> Row {
    Row {
        case,
        web_acl,
        method: "GET",
        target: "/",
        headers: &[],
        status,
        body,
        response_headers: &[],
    }
}

/// The header the recorded blocking rule inspects.
pub(crate) const BLOCK_HEADER: &str = "x-waf-block";

/// A `Block` rule on the single header above, exactly as the recorded web ACL writes it.
fn header_block() -> Value {
    rule(
        "block-header",
        byte_match(
            json!({ "SingleHeader": { "Name": BLOCK_HEADER } }),
            "1",
            "EXACTLY",
        ),
        json!({ "Block": {} }),
    )
}

/// The statuses WAF serves its terminating actions under, which the table below asserts:
/// `403` for a `Block`, and for the two checks that serve an interstitial the statuses
/// their own documentation gives — "the HTTP status code 405 Method Not Allowed"
/// (`CaptchaAction`) and 202 for a `Challenge`.
pub(crate) const BLOCK_STATUS: u16 = 403;
pub(crate) const CAPTCHA_STATUS: u16 = 405;
pub(crate) const CHALLENGE_STATUS: u16 = 202;

/// Every row of the table.
fn rows() -> Vec<Row> {
    vec![
        // No web ACL at all: the listener answers, which is this plane's behaviour before
        // any binding exists.
        row("no web ACL in front of the load balancer", None, 200, SERVED_BODY),
        // A web ACL with nothing to match: its `DefaultAction` allows.
        row(
            "a web ACL with no rules and a default action that allows",
            Some(WebAcl::allowing(json!([]))),
            200,
            SERVED_BODY,
        ),
        // The recorded case, and the two requests it must NOT refuse.
        Row {
            headers: &[(BLOCK_HEADER, "1")],
            ..row(
                "a Block rule on a single header, the header matching",
                Some(WebAcl::allowing(json!([header_block()]))),
                BLOCK_STATUS,
                "",
            )
        },
        row(
            "a Block rule on a single header, the header absent",
            Some(WebAcl::allowing(json!([header_block()]))),
            200,
            SERVED_BODY,
        ),
        Row {
            headers: &[(BLOCK_HEADER, "2")],
            ..row(
                "a Block rule on a single header, the header carrying another value",
                Some(WebAcl::allowing(json!([header_block()]))),
                200,
                SERVED_BODY,
            )
        },
        // The same rule over every other request component a `FieldToMatch` names, each
        // with a matching and a non-matching request.
        Row {
            target: "/admin",
            ..row(
                "a Block rule on the URI path, the path matching",
                Some(WebAcl::allowing(json!([rule(
                    "block-path",
                    byte_match(json!({ "UriPath": {} }), "/admin", "EXACTLY"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
                "",
            )
        },
        Row {
            target: "/public",
            ..row(
                "a Block rule on the URI path, another path",
                Some(WebAcl::allowing(json!([rule(
                    "block-path",
                    byte_match(json!({ "UriPath": {} }), "/admin", "EXACTLY"),
                    json!({ "Block": {} }),
                )]))),
                200,
                SERVED_BODY,
            )
        },
        Row {
            target: "/?drop=all",
            ..row(
                "a Block rule on the query string, a query string containing the value",
                Some(WebAcl::allowing(json!([rule(
                    "block-query",
                    byte_match(json!({ "QueryString": {} }), "drop", "CONTAINS"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
                "",
            )
        },
        Row {
            target: "/?keep=all",
            ..row(
                "a Block rule on the query string, another query string",
                Some(WebAcl::allowing(json!([rule(
                    "block-query",
                    byte_match(json!({ "QueryString": {} }), "drop", "CONTAINS"),
                    json!({ "Block": {} }),
                )]))),
                200,
                SERVED_BODY,
            )
        },
        Row {
            method: "POST",
            ..row(
                "a Block rule on the method, the method matching",
                Some(WebAcl::allowing(json!([rule(
                    "block-method",
                    byte_match(json!({ "Method": {} }), "POST", "EXACTLY"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
                "",
            )
        },
        row(
            "a Block rule on the method, another method",
            Some(WebAcl::allowing(json!([rule(
                "block-method",
                byte_match(json!({ "Method": {} }), "POST", "EXACTLY"),
                json!({ "Block": {} }),
            )]))),
            200,
            SERVED_BODY,
        ),
        // The web ACL's own default action decides a request no rule matched.
        row(
            "a web ACL whose default action blocks",
            Some(WebAcl {
                default_action: json!({ "Block": {} }),
                rules: json!([]),
                custom_response_bodies: None,
            }),
            BLOCK_STATUS,
            "",
        ),
        // A `Count` action is not terminating: the request goes on to the default action,
        // which allows it, so the listener answers.
        row(
            "a Count rule that matches, under a default action that allows",
            Some(WebAcl::allowing(json!([rule(
                "count-path",
                byte_match(json!({ "UriPath": {} }), "/", "EXACTLY"),
                json!({ "Count": {} }),
            )]))),
            200,
            SERVED_BODY,
        ),
        // A block that configures its OWN response is served by WAF, with that response's
        // status, body, media type and headers — the listener's action is not consulted.
        Row {
            headers: &[(BLOCK_HEADER, "1")],
            response_headers: &[("x-refused-by", "waf"), ("content-type", "text/plain")],
            ..row(
                "a Block rule carrying its own custom response",
                Some(WebAcl {
                    default_action: json!({ "Allow": {} }),
                    rules: json!([rule(
                        "block-header",
                        byte_match(
                            json!({ "SingleHeader": { "Name": BLOCK_HEADER } }),
                            "1",
                            "EXACTLY",
                        ),
                        json!({
                            "Block": {
                                "CustomResponse": {
                                    "ResponseCode": 503,
                                    "CustomResponseBodyKey": "refused",
                                    "ResponseHeaders": [
                                        { "Name": "x-refused-by", "Value": "waf" }
                                    ],
                                }
                            }
                        }),
                    )]),
                    custom_response_bodies: Some(json!({
                        "refused": { "ContentType": "TEXT_PLAIN", "Content": "refused at the edge" }
                    })),
                }),
                503,
                "refused at the edge",
            )
        },
        // The other two terminating actions, each under the status WAF serves it with.
        Row {
            headers: &[(BLOCK_HEADER, "1")],
            ..row(
                "a Captcha rule that matches, with no token on the request",
                Some(WebAcl::allowing(json!([rule(
                    "captcha-header",
                    byte_match(
                        json!({ "SingleHeader": { "Name": BLOCK_HEADER } }),
                        "1",
                        "EXACTLY",
                    ),
                    json!({ "Captcha": {} }),
                )]))),
                CAPTCHA_STATUS,
                "",
            )
        },
        Row {
            headers: &[(BLOCK_HEADER, "1")],
            ..row(
                "a Challenge rule that matches, with no token on the request",
                Some(WebAcl::allowing(json!([rule(
                    "challenge-header",
                    byte_match(
                        json!({ "SingleHeader": { "Name": BLOCK_HEADER } }),
                        "1",
                        "EXACTLY",
                    ),
                    json!({ "Challenge": {} }),
                )]))),
                CHALLENGE_STATUS,
                "",
            )
        },
        // An `Allow` that inserts a header: the request reaches the listener, whose own
        // rule matches on the inserted header and answers its own body.
        row(
            "an Allow rule inserting a header the listener's rule matches",
            Some(WebAcl::allowing(json!([rule(
                "tag-everything",
                byte_match(json!({ "UriPath": {} }), "/", "EXACTLY"),
                json!({
                    "Allow": {
                        "CustomRequestHandling": {
                            "InsertHeaders": [
                                { "Name": INSERTED_HEADER, "Value": INSERTED_VALUE }
                            ]
                        }
                    }
                }),
            )]))),
            200,
            TAGGED_BODY,
        ),
        // The same request with no web ACL in front: the listener's rule does not match,
        // so the inserted header above is the only thing that made it match.
        row(
            "no web ACL, so nothing inserts that header",
            None,
            200,
            SERVED_BODY,
        ),
    ]
}

/// Assert one answered request against its row.
fn assert_row(row: &Row, answered: &Answered) {
    assert_eq!(
        answered.status, row.status,
        "case {:?}: expected HTTP {}, got {} body {:?}",
        row.case, row.status, answered.status, answered.body
    );
    assert_eq!(
        answered.body, row.body,
        "case {:?}: expected body {:?}, got {:?}",
        row.case, row.body, answered.body
    );
    for (name, value) in row.response_headers {
        assert_eq!(
            answered.header(name),
            Some(*value),
            "case {:?}: expected header {name}: {value:?}, got {:?}",
            row.case,
            answered.header(name)
        );
    }
}

#[tokio::test]
async fn the_web_acl_in_front_of_a_load_balancer_decides_every_request_to_it() {
    for (index, row) in rows().into_iter().enumerate() {
        let deployed = deployment(&format!("waf-alb-{index}")).await;
        if let Some(spec) = &row.web_acl {
            let web_acl_arn =
                create_web_acl(&deployed.router, &format!("waf-acl-{index}"), spec).await;
            waf_ok(
                &deployed.router,
                "AssociateWebACL",
                json!({ "ResourceArn": deployed.lb_arn, "WebACLArn": web_acl_arn }),
            )
            .await;
        }
        let answered = send(
            &deployed.router,
            row.method,
            &deployed.dns_name,
            row.target,
            row.headers,
        )
        .await;
        assert_row(&row, &answered);
    }
}

/// The BINDING is what enforces: the same request is refused while the web ACL is
/// associated and served by the listener once `DisassociateWebACL` takes the binding away,
/// and associating a SECOND web ACL over the first replaces it.
#[tokio::test]
async fn enforcement_follows_the_binding_the_association_writes() {
    let deployed = deployment("waf-alb-binding").await;
    let blocking =
        create_web_acl(&deployed.router, "binding-blocking", &WebAcl::allowing(json!([header_block()])))
            .await;
    let permissive =
        create_web_acl(&deployed.router, "binding-permissive", &WebAcl::allowing(json!([]))).await;
    let blocked: &[(&str, &str)] = &[(BLOCK_HEADER, "1")];

    // Before any binding: the listener answers both requests.
    for (case, headers) in [("no binding, the header set", blocked), ("no binding, no header", &[])] {
        let answered = send(&deployed.router, "GET", &deployed.dns_name, "/", headers).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (200, SERVED_BODY),
            "case {case:?}: expected the listener's own answer, got {} {:?}",
            answered.status,
            answered.body
        );
    }

    // The blocking web ACL, bound: the matching request is refused and the other is not.
    waf_ok(
        &deployed.router,
        "AssociateWebACL",
        json!({ "ResourceArn": deployed.lb_arn, "WebACLArn": blocking }),
    )
    .await;
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/", blocked).await;
    assert_eq!(
        answered.status, BLOCK_STATUS,
        "case the bound blocking web ACL: expected HTTP {BLOCK_STATUS}, got {} body {:?}",
        answered.status, answered.body
    );
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, SERVED_BODY),
        "case the bound blocking web ACL, no header: expected the listener's own answer, \
         got {} {:?}",
        answered.status,
        answered.body
    );

    // A second association REPLACES the first: the resource has at most one web ACL in
    // front of it, so the matching request is no longer refused.
    waf_ok(
        &deployed.router,
        "AssociateWebACL",
        json!({ "ResourceArn": deployed.lb_arn, "WebACLArn": permissive }),
    )
    .await;
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/", blocked).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, SERVED_BODY),
        "case the replacing web ACL: expected the listener's own answer, got {} {:?}",
        answered.status,
        answered.body
    );

    // And with the binding taken away, nothing decides the request but the listener.
    waf_ok(
        &deployed.router,
        "AssociateWebACL",
        json!({ "ResourceArn": deployed.lb_arn, "WebACLArn": blocking }),
    )
    .await;
    waf_ok(
        &deployed.router,
        "DisassociateWebACL",
        json!({ "ResourceArn": deployed.lb_arn }),
    )
    .await;
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/", blocked).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, SERVED_BODY),
        "case the binding taken away: expected the listener's own answer, got {} {:?}",
        answered.status,
        answered.body
    );
}

/// A web ACL bound to ANOTHER resource decides nothing about this load balancer's traffic:
/// the binding is keyed by the protected resource's own ARN.
#[tokio::test]
async fn a_web_acl_bound_to_another_resource_decides_nothing_here() {
    let deployed = deployment("waf-alb-unbound").await;
    let blocking = create_web_acl(
        &deployed.router,
        "unbound-blocking",
        &WebAcl::allowing(json!([header_block()])),
    )
    .await;

    // One row per OTHER protected resource the same web ACL is put in front of, in the
    // documented ARN forms of `AssociateWebACL`'s `ResourceArn`: a second load balancer of
    // this service's, and a REST API stage.
    let (other_lb_arn, _other_dns) = create_load_balancer(&deployed.router, "waf-alb-other").await;
    let other_resources: &[(&str, &str)] = &[
        ("another load balancer", other_lb_arn.as_str()),
        (
            "an API Gateway REST API stage",
            "arn:aws:apigateway:us-west-2::/restapis/a1b2c3d4e5/stages/prod",
        ),
    ];
    for (case, resource_arn) in other_resources {
        waf_ok(
            &deployed.router,
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": blocking }),
        )
        .await;
        let answered = send(
            &deployed.router,
            "GET",
            &deployed.dns_name,
            "/",
            &[(BLOCK_HEADER, "1")],
        )
        .await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (200, SERVED_BODY),
            "case {case:?}: a web ACL in front of it must not decide this load balancer's \
             traffic, got {} {:?}",
            answered.status,
            answered.body
        );
    }
}

/// The decision is WAF's OWN data plane, not a second reading of a rule tree: the request
/// WAF refused is in the sample `GetSampledRequests` draws from, for the dimension of the
/// rule that refused it, and the request it allowed is in the web ACL's own dimension.
#[tokio::test]
async fn a_decided_request_enters_the_web_acl_sample_the_report_draws_from() {
    let deployed = deployment("waf-alb-sampled").await;
    let web_acl_arn = create_web_acl(
        &deployed.router,
        "sampled-acl",
        &WebAcl::allowing(json!([header_block()])),
    )
    .await;
    waf_ok(
        &deployed.router,
        "AssociateWebACL",
        json!({ "ResourceArn": deployed.lb_arn, "WebACLArn": web_acl_arn }),
    )
    .await;

    let blocked = send(
        &deployed.router,
        "GET",
        &deployed.dns_name,
        "/pay?x=1",
        &[(BLOCK_HEADER, "1")],
    )
    .await;
    assert_eq!(
        blocked.status, BLOCK_STATUS,
        "the matching request must be refused, got {} body {:?}",
        blocked.status, blocked.body
    );
    let allowed = send(&deployed.router, "GET", &deployed.dns_name, "/pay?x=1", &[]).await;
    assert_eq!(
        allowed.status, 200,
        "the other request must reach the listener, got {} body {:?}",
        allowed.status, allowed.body
    );

    // (the dimension the sample is drawn for, the action it must report)
    let dimensions: &[(&str, &str)] = &[("block-header", "BLOCK"), ("sampled-acl", "ALLOW")];
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as f64;
    for (metric, expected_action) in dimensions {
        let report = waf_ok(
            &deployed.router,
            "GetSampledRequests",
            json!({
                "WebAclArn": web_acl_arn,
                "RuleMetricName": metric,
                "Scope": "REGIONAL",
                "TimeWindow": { "StartTime": now - 600.0, "EndTime": now + 60.0 },
                "MaxItems": 10,
            }),
        )
        .await;
        let samples = report["SampledRequests"]
            .as_array()
            .unwrap_or_else(|| panic!("case {metric:?}: SampledRequests must be a list, got {report}"))
            .clone();
        assert_eq!(
            samples.len(),
            1,
            "case {metric:?}: expected exactly the one request this dimension decided, got {report}"
        );
        assert_eq!(
            samples[0]["Action"].as_str(),
            Some(*expected_action),
            "case {metric:?}: expected the sample to report {expected_action}, got {report}"
        );
        assert_eq!(
            samples[0]["Request"]["URI"].as_str(),
            Some("/pay?x=1"),
            "case {metric:?}: expected the sample to carry the request line the client \
             sent, got {report}"
        );
    }
}
