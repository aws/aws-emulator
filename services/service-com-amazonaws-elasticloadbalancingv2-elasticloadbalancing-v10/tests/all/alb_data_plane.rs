// SPDX-License-Identifier: CC0-1.0

//! The Application Load Balancer DATA plane, end to end over the wire: what a client that
//! sends traffic to the name a load balancer publishes gets back.
//!
//! The rule under test: **a request a client addresses to an Application Load Balancer is
//! answered out of the listener it reaches — the actions of the first rule, in ascending
//! priority order, whose conditions are ALL met, and the listener's default actions when
//! no rule's are.** A `fixed-response` action answers its own status code, content type
//! and message body; a `redirect` answers its status with the `Location` its components
//! build; a `forward` names targets this emulator has no server behind, so it answers the
//! status that condition earns rather than a body invented for a backend that never ran.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, carrying no AWS credential, exactly as a client of the
//! published `DNSName` sends it — and the configuration it is answered from is written
//! through the control plane's own operations (`CreateLoadBalancer`, `CreateTargetGroup`,
//! `CreateListener`, `CreateRule`, `RegisterTargets`), so nothing here is served from a
//! record a test invented.
//!
//! The rows span what the rule generalises over: each documented condition field (given
//! directly and through its own configuration shape), a matching and a non-matching value
//! for each, two rules whose patterns overlap at different priorities, a request no rule
//! matches, each terminating action type, an action this emulator cannot perform, the four
//! addressings a client can reach a load balancer by, and a load balancer the clock still
//! owes its move out of `provisioning`.
//!
//! The clock-owed moves (a load balancer's to `active`, a registration's to `healthy`) are
//! placed on either side of now in the store rather than slept for, exactly as
//! `provisioning_loop::seed_trust_store` places a trust store's.
//!
//! One condition is about the CONNECTION rather than the request, so it has a table of its
//! own at the end of this suite: **a `source-ip` condition is met exactly when the address
//! the connection came from lies in one of the CIDR blocks it names, and never because of an
//! `X-Forwarded-For` the client supplied** — `SourceIpConditionConfig$Values` is "the source
//! IP addresses, in CIDR format … the condition is satisfied if the source IP address of the
//! request matches one of the CIDR blocks. This condition is not satisfied by the addresses
//! in the X-Forwarded-For header". Those rows are driven over a REAL connection to the
//! published address, because the address a request comes from exists only on one, and the
//! blocks each row is judged against are written through `ModifyRule` and read back through
//! `DescribeRules` first. A second table pins which VALUES name a block at all.
//!
//! The last table of this suite is about the ADDRESS the client sends to rather than the
//! request it sends: **the value a load balancer publishes as its `DNSName` is an address of
//! THIS emulator — the `host:port` of the socket it opened for that load balancer — reported
//! identically by the create and by every later read, held by one load balancer alone, and
//! answered out of that load balancer's own stored configuration for as long as it exists.**
//! `LoadBalancer$DNSName` is "the public DNS name of the load balancer", the name a client
//! resolves and connects to in order to reach a listener, so a value naming a host no client
//! of this machine resolves announces an address the service does not answer on. Those rows
//! DIAL the published value over a real connection, as a client that reads it does, and they
//! are driven against two load balancers at once so what decides the answer is the address
//! dialled and not the path asked for.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::alb_desync_mitigation_and_forwarded_for::{raw_exchange, CONNECTION_ADDRESS};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use crate::target_group_attributes_and_modification::unescaped;

/// The host a client addresses the emulator's own endpoint by, which is the only address a
/// client on the machine this emulator runs on can reach — and so the host the three
/// non-published addressings are read on.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The body the listener's DEFAULT action answers with, which is what a request no rule
/// matches gets.
const DEFAULT_BODY: &str = "no-match";

/// One answered request: the status line, the headers asserted on, and the body verbatim.
pub(crate) struct Answered {
    pub(crate) status: u16,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: String,
}

impl Answered {
    /// One response header, lower-cased as HTTP names compare.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Issue one data-plane request: no credential, no body, `headers` as the client sent
/// them.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{target}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("the data-plane request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Answered {
        status,
        headers,
        body,
    }
}

/// Create a load balancer and answer its ARN, its name and the DNS name it published.
pub(crate) async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create load balancer {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let arn = reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body));
    let dns_name = reply
        .text("DNSName")
        .unwrap_or_else(|| panic!("case create {name}: no DNSName in {}", reply.body));
    (arn, dns_name)
}

/// Create the HTTP:80 listener whose default action is the fixed response above, and
/// answer its ARN.
pub(crate) async fn create_listener(router: &MultiServiceRouter, lb_arn: &str) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        "404",
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.MessageBody",
        DEFAULT_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create listener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener: no ListenerArn in {}", reply.body))
}

/// Add one rule at `priority` carrying `fields`, which must be accepted. Answers the ARN
/// the create minted, which is what a later `ModifyRule` of that rule names.
pub(crate) async fn add_rule(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
    priority: i32,
    fields: &[(String, String)],
) -> String {
    let mut form = action("CreateRule");
    form.push(p("ListenerArn", listener_arn));
    form.push(p("Priority", &priority.to_string()));
    form.extend(fields.iter().cloned());
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case rule {case}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("RuleArn")
        .unwrap_or_else(|| panic!("case rule {case}: no RuleArn in {}", reply.body))
}

/// A `fixed-response` action's fields, under `prefix`.
pub(crate) fn fixed_response(
    status: &str,
    body: &str,
    content_type: Option<&str>,
) -> Vec<(String, String)> {
    let mut fields = vec![
        p("Actions.member.1.Type", "fixed-response"),
        p("Actions.member.1.FixedResponseConfig.StatusCode", status),
        p("Actions.member.1.FixedResponseConfig.MessageBody", body),
    ];
    if let Some(content_type) = content_type {
        fields.push(p(
            "Actions.member.1.FixedResponseConfig.ContentType",
            content_type,
        ));
    }
    fields
}

/// A `redirect` action's fields: the `RedirectActionConfig$StatusCode` it answers with, the host
/// it sends the client to, and the request's own path kept through that action's `#{path}`
/// placeholder.
///
/// `pub(crate)` beside [`fixed_response`]: one spelling of each terminating action's fields, so
/// two suites of this crate cannot write a rule's action two different ways.
pub(crate) fn redirect_to(status_code: &str, host: &str) -> Vec<(String, String)> {
    vec![
        p("Actions.member.1.Type", "redirect"),
        p("Actions.member.1.RedirectConfig.StatusCode", status_code),
        p("Actions.member.1.RedirectConfig.Host", host),
        p("Actions.member.1.RedirectConfig.Path", "/#{path}"),
    ]
}

/// One condition on a path pattern, which most rows carry so they match one path each.
pub(crate) fn path_pattern(pattern: &str) -> Vec<(String, String)> {
    vec![
        p("Conditions.member.1.Field", "path-pattern"),
        p("Conditions.member.1.Values.member.1", pattern),
    ]
}

/// Both fields of one request table row: what was sent, and what must come back.
struct Row {
    case: &'static str,
    method: &'static str,
    host: &'static str,
    target: &'static str,
    headers: &'static [(&'static str, &'static str)],
    status: u16,
    body: &'static str,
    /// The `Content-Type` the answer must carry, when the row asserts one.
    content_type: Option<&'static str>,
    /// The `Location` the answer must carry, for a redirect row.
    location: Option<&'static str>,
}

/// A row that asserts a status and a body alone.
fn answers(
    case: &'static str,
    method: &'static str,
    target: &'static str,
    status: u16,
    body: &'static str,
) -> Row {
    Row {
        case,
        method,
        host: "",
        target,
        headers: &[],
        status,
        body,
        content_type: None,
        location: None,
    }
}

/// Everything the fixture built, so each test drives the same deployment.
struct Deployment {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    lb_arn: String,
    dns_name: String,
    name: String,
}

/// The load balancer every row below is answered by: one HTTP:80 listener whose default
/// action is a fixed 404, and one rule per condition field and per action type.
async fn deployment() -> Deployment {
    let (router, engine) = emulator();
    let name = "dp-alb".to_string();
    let (lb_arn, dns_name) = create_load_balancer(&router, &name).await;
    let listener_arn = create_listener(&router, &lb_arn).await;
    let forwarded = create_http_target_group(&router, "dp-registered").await;
    let empty = create_http_target_group(&router, "dp-empty").await;

    // Two rules whose patterns overlap: the lower priority VALUE is evaluated first, so
    // `/a` is answered `a1` and the rule at 20 never runs.
    let mut fields = path_pattern("/a");
    fields.extend(fixed_response("200", "a1", None));
    add_rule(&router, "path /a at 10", &listener_arn, 10, &fields).await;
    let mut fields = path_pattern("/a");
    fields.extend(fixed_response("200", "b1", None));
    add_rule(&router, "path /a at 20", &listener_arn, 20, &fields).await;

    // A wildcard pattern, and the content type the action carries.
    let mut fields = path_pattern("/img/*");
    fields.extend(fixed_response(
        "200",
        "{\"ok\":true}",
        Some("application/json"),
    ));
    add_rule(&router, "path /img/*", &listener_arn, 30, &fields).await;

    // One rule per remaining condition field, each given through its own configuration
    // shape where it has one, so both spellings are exercised.
    let mut fields = vec![
        p("Conditions.member.1.Field", "host-header"),
        p(
            "Conditions.member.1.HostHeaderConfig.Values.member.1",
            "shop.example.com",
        ),
    ];
    fields.extend(fixed_response("200", "shop", None));
    add_rule(&router, "host-header", &listener_arn, 40, &fields).await;

    let mut fields = path_pattern("/m");
    fields.push(p("Conditions.member.2.Field", "http-request-method"));
    fields.push(p(
        "Conditions.member.2.HttpRequestMethodConfig.Values.member.1",
        "POST",
    ));
    fields.extend(fixed_response("201", "created", None));
    add_rule(&router, "http-request-method", &listener_arn, 50, &fields).await;

    let mut fields = vec![
        p("Conditions.member.1.Field", "http-header"),
        p(
            "Conditions.member.1.HttpHeaderConfig.HttpHeaderName",
            "X-Channel",
        ),
        p("Conditions.member.1.HttpHeaderConfig.Values.member.1", "mob*"),
    ];
    fields.extend(fixed_response("200", "mobile", None));
    add_rule(&router, "http-header", &listener_arn, 60, &fields).await;

    let mut fields = vec![
        p("Conditions.member.1.Field", "query-string"),
        p(
            "Conditions.member.1.QueryStringConfig.Values.member.1.Key",
            "ver",
        ),
        p(
            "Conditions.member.1.QueryStringConfig.Values.member.1.Value",
            "2",
        ),
    ];
    fields.extend(fixed_response("200", "v2", None));
    add_rule(&router, "query-string", &listener_arn, 70, &fields).await;

    // A `source-ip` condition in front of an action this plane can perform: it is about the
    // CONNECTION's address, so a rule whose block does not hold the address this request
    // came from must be passed over however the request itself claims to come from it.
    let mut fields = path_pattern("/ip");
    fields.push(p("Conditions.member.2.Field", "source-ip"));
    fields.push(p(
        "Conditions.member.2.SourceIpConfig.Values.member.1",
        "10.0.0.0/8",
    ));
    fields.extend(fixed_response("200", "internal", None));
    add_rule(&router, "source-ip", &listener_arn, 80, &fields).await;

    // And one that matches on a regular expression, which this plane compiles and evaluates
    // through the same reading the control plane accepted it by.
    let mut fields = vec![
        p("Conditions.member.1.Field", "path-pattern"),
        p(
            "Conditions.member.1.PathPatternConfig.RegexValues.member.1",
            "^/re/[0-9]+$",
        ),
    ];
    fields.extend(fixed_response("200", "regex", None));
    add_rule(&router, "regex values", &listener_arn, 85, &fields).await;

    // And one whose expression is matched against an HTTP HEADER, beside a wildcard path
    // pattern: a rule matches when every condition of it does, whichever language each
    // condition's values are written in.
    let mut fields = path_pattern("/hr");
    fields.push(p("Conditions.member.2.Field", "http-header"));
    fields.push(p(
        "Conditions.member.2.HttpHeaderConfig.HttpHeaderName",
        "x-api-version",
    ));
    fields.push(p(
        "Conditions.member.2.HttpHeaderConfig.RegexValues.member.1",
        "^v[0-9]+$",
    ));
    fields.extend(fixed_response("200", "header-regex", None));
    add_rule(&router, "header regex values", &listener_arn, 86, &fields).await;

    // A redirect that changes the host and keeps the request's own path.
    let mut fields = path_pattern("/old");
    fields.extend(redirect_to("HTTP_301", "new.example.com"));
    add_rule(&router, "redirect", &listener_arn, 90, &fields).await;

    // A forward to a group with a target, and one to a group with none.
    let mut fields = path_pattern("/tg");
    fields.push(p("Actions.member.1.Type", "forward"));
    fields.push(p("Actions.member.1.TargetGroupArn", &forwarded));
    add_rule(&router, "forward to a registered target", &listener_arn, 100, &fields).await;
    let mut fields = path_pattern("/empty");
    fields.push(p("Actions.member.1.Type", "forward"));
    fields.push(p("Actions.member.1.TargetGroupArn", &empty));
    add_rule(&router, "forward to an empty group", &listener_arn, 110, &fields).await;

    // An action this emulator cannot perform, in front of one it can: a `jwt-validation`
    // action has to FETCH the key set it judges a token against, which is an outbound
    // connection this emulator makes none of, so the request it guards must not be served by
    // falling through to the action behind it. (What a rule's AUTHENTICATION action answers
    // instead — the 401, the redirect to the provider's authorization endpoint, or the action
    // behind it — is pinned in `alb_authenticate_users`.)
    let mut fields = path_pattern("/auth");
    fields.push(p("Actions.member.1.Type", "jwt-validation"));
    fields.push(p("Actions.member.1.Order", "1"));
    for (member, value) in [
        (
            "JwksEndpoint",
            "https://idp.example.com/.well-known/jwks.json",
        ),
        ("Issuer", "https://idp.example.com"),
    ] {
        fields.push(p(
            &format!("Actions.member.1.JwtValidationConfig.{member}"),
            value,
        ));
    }
    fields.push(p("Actions.member.2.Type", "fixed-response"));
    fields.push(p("Actions.member.2.Order", "2"));
    fields.push(p(
        "Actions.member.2.FixedResponseConfig.StatusCode",
        "200",
    ));
    fields.push(p(
        "Actions.member.2.FixedResponseConfig.MessageBody",
        "behind-the-idp",
    ));
    add_rule(&router, "jwt-validation", &listener_arn, 120, &fields).await;

    // One target in the forwarded group, which a load balancer now forwards to.
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &forwarded));
    form.push(p("Targets.member.1.Id", "i-11111111111111111"));
    let registered = call(&router, &form).await;
    assert_eq!(
        registered.status, 200,
        "case register a target: expected 200, got {} body {}",
        registered.status, registered.body
    );

    // The clock-owed moves, placed rather than slept for: the load balancer is active and
    // the registration is healthy.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));
    Deployment {
        router,
        engine,
        lb_arn,
        dns_name,
        name,
    }
}

/// Place the instant the load balancer's move to `active` and every registration's move to
/// `healthy` are owed at, so a case reads either side of it without sleeping.
pub(crate) fn settle(engine: &Arc<engine::InMemoryEngine>, lb_arn: &str, at: SystemTime) {
    let state = engine.state::<ElbState>();
    let mut store = state.lock();
    if let Some(record) = store.load_balancers.get_mut(lb_arn) {
        record.active_at = at;
    }
    for group in store.target_groups.values_mut() {
        for target in group.targets.values_mut() {
            target.healthy_at = at;
        }
    }
}

/// Every rule of the listener, as `DescribeRules` reports them: the read a data-plane
/// request must leave untouched.
///
/// The rule MEMBERS rather than the whole envelope, because each response carries its own
/// freshly minted `RequestId` — what two reads must agree on is the rules they report.
async fn described_rules(router: &MultiServiceRouter, lb_arn: &str) -> Vec<String> {
    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", lb_arn));
    let listeners = call(router, &form).await;
    let listener_arn = listeners
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("no ListenerArn in {}", listeners.body));
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener_arn));
    let reply = call(router, &form).await;
    members(&reply.body, "Rules")
}

/// Every target of the group a rule forwards to, as `DescribeTargetHealth` reports them.
async fn described_health(router: &MultiServiceRouter, tg_arn: &str) -> Vec<String> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", tg_arn));
    let reply = call(router, &form).await;
    members(&reply.body, "TargetHealthDescriptions")
}

#[tokio::test]
async fn a_listener_answers_a_request_from_the_rule_its_conditions_match() {
    let deployed = deployment().await;
    let dns_name: &str = &deployed.dns_name;

    let mut rows = vec![
        // The overlapping patterns: the lower priority value wins, and the rule at 20 is
        // never reached.
        answers("the first matching rule by priority", "GET", "/a", 200, "a1"),
        // No rule's conditions are met, so the listener's default action answers.
        answers("no rule matches", "GET", "/b", 404, DEFAULT_BODY),
        answers("no rule matches a deeper path", "GET", "/a/b", 404, DEFAULT_BODY),
        // `*` matches zero or more characters.
        Row {
            content_type: Some("application/json"),
            ..answers("a wildcard pattern", "GET", "/img/cat.png", 200, "{\"ok\":true}")
        },
        Row {
            content_type: Some("application/json"),
            ..answers("a wildcard matching nothing", "GET", "/img/", 200, "{\"ok\":true}")
        },
        // The method condition, both ways.
        answers("the method the rule names", "POST", "/m", 201, "created"),
        answers("another method", "GET", "/m", 404, DEFAULT_BODY),
        // The default content type of a fixed response: "If the value is not specified,
        // the default value is text/plain."
        Row {
            content_type: Some("text/plain"),
            ..answers("the default content type", "GET", "/a", 200, "a1")
        },
        // A redirect's status and the Location its components build; the port is the
        // listener's 80, which the scheme implies and so is left off.
        Row {
            location: Some("http://new.example.com/old"),
            ..answers("a redirect", "GET", "/old", 301, "")
        },
        // A forward names targets with no server of this emulator's behind them.
        answers("a forward to a healthy target", "GET", "/tg", 502, ""),
        answers("a forward to a group with no target", "GET", "/empty", 503, ""),
        // An action this emulator cannot perform is not bypassed.
        answers("an action this emulator cannot perform", "GET", "/auth", 500, ""),
    ];
    // The conditions read off a header, the query string and the client address, each
    // with a matching and a non-matching value.
    rows.extend([
        Row {
            headers: &[("x-channel", "mobile")],
            ..answers("a header value the rule matches", "GET", "/h", 200, "mobile")
        },
        Row {
            headers: &[("x-channel", "web")],
            ..answers("a header value it does not", "GET", "/h", 404, DEFAULT_BODY)
        },
        Row {
            headers: &[],
            ..answers("the header absent", "GET", "/h", 404, DEFAULT_BODY)
        },
        answers("a query parameter the rule matches", "GET", "/q?ver=2", 200, "v2"),
        answers("a query parameter it does not", "GET", "/q?ver=3", 404, DEFAULT_BODY),
        // A `source-ip` condition is matched against the address the CONNECTION came from,
        // which AWS documents the forwarded-for header does NOT satisfy, so a block holding
        // no address of this machine leaves its rule unmatched however the request claims to
        // come from it.
        Row {
            headers: &[("x-forwarded-for", "10.1.2.3")],
            ..answers("a source-ip rule and a forwarded client address", "GET", "/ip", 404, DEFAULT_BODY)
        },
        Row {
            headers: &[],
            ..answers("a source-ip rule and no forwarded address", "GET", "/ip", 404, DEFAULT_BODY)
        },
        // A regular-expression condition, and the near miss beside it: the request the
        // expression matches is answered by that rule's action, and one it does not falls
        // through to the listener's default.
        answers("a rule matching on a regular expression", "GET", "/re/7", 200, "regex"),
        answers("a near miss of that expression", "GET", "/re/x", 404, DEFAULT_BODY),
        // The same, for an expression matched against a header value.
        Row {
            headers: &[("x-api-version", "v12")],
            ..answers("a header value the expression matches", "GET", "/hr", 200, "header-regex")
        },
        Row {
            headers: &[("x-api-version", "vX")],
            ..answers("a header value it does not", "GET", "/hr", 404, DEFAULT_BODY)
        },
        Row {
            headers: &[],
            ..answers("the header absent", "GET", "/hr", 404, DEFAULT_BODY)
        },
    ]);

    for row in &rows {
        let host = if row.host.is_empty() {
            dns_name
        } else {
            row.host
        };
        let answered = send(&deployed.router, row.method, host, row.target, row.headers).await;
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
        if let Some(content_type) = row.content_type {
            assert_eq!(
                answered.header("content-type"),
                Some(content_type),
                "case {:?}: expected content type {content_type:?}, got {:?}",
                row.case,
                answered.header("content-type")
            );
        }
        if let Some(location) = row.location {
            assert_eq!(
                answered.header("location"),
                Some(location),
                "case {:?}: expected Location {location:?}, got {:?}",
                row.case,
                answered.header("location")
            );
        }
    }

    // The host-header condition is about the host the CLIENT addressed, which on AWS is
    // an alias of the load balancer's own name. A client reaching this emulator carries
    // that host and names the load balancer in the path, so the condition is driven that
    // way — and the same request under the published DNS name matches no host-header rule.
    let prefixed = format!("/_aws/elasticloadbalancing/{dns_name}/z");
    let answered = send(
        &deployed.router,
        "GET",
        "shop.example.com",
        &prefixed,
        &[],
    )
    .await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, "shop"),
        "case the host the rule names: expected the host-header rule's answer, got {} {:?}",
        answered.status,
        answered.body
    );
    let answered = send(&deployed.router, "GET", dns_name, "/z", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (404, DEFAULT_BODY),
        "case another host: the host-header rule does not match it, got {} {:?}",
        answered.status,
        answered.body
    );
}

#[tokio::test]
async fn a_load_balancer_is_reached_by_every_addressing_a_client_can_use() {
    let deployed = deployment().await;
    // Each addressing resolves the SAME load balancer, so each answers the rule at
    // priority 10 for `/a`.
    let prefixed = format!("/_aws/elasticloadbalancing/{}/a", deployed.dns_name);
    let by_name = format!("/_aws/elasticloadbalancing/{}/a", deployed.name);
    let path_style = format!("/{}/a", deployed.name);
    let rows: Vec<(&str, &str, &str)> = vec![
        ("the published DNS name", deployed.dns_name.as_str(), "/a"),
        ("the reserved prefix and the DNS name", EMULATOR_HOST, prefixed.as_str()),
        ("the reserved prefix and the name", EMULATOR_HOST, by_name.as_str()),
        ("the name as the first path segment", EMULATOR_HOST, path_style.as_str()),
        ("the request path alone", EMULATOR_HOST, "/a"),
    ];
    for (case, host, target) in &rows {
        let answered = send(&deployed.router, "GET", host, target, &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (200, "a1"),
            "case {case:?}: expected the rule's own answer, got {} {:?}",
            answered.status,
            answered.body
        );
    }

    // A request that addresses ANOTHER service's endpoint or another data plane's reserved
    // namespace, presents the credentials of an AWS API call (signed or presigned), or
    // carries a path no rule of this load balancer is written for, is not this load
    // balancer's traffic and is not answered as such — which is what leaves one claimant
    // per request when another plane reads a host-independent addressing of its own.
    struct Unclaimed {
        case: &'static str,
        host: &'static str,
        target: &'static str,
        headers: &'static [(&'static str, &'static str)],
    }
    let unclaimed: &[Unclaimed] = &[
        Unclaimed {
            case: "another service's endpoint",
            host: "s3.us-west-2.amazonaws.com",
            target: "/a",
            headers: &[],
        },
        Unclaimed {
            case: "another data plane's reserved namespace",
            host: EMULATOR_HOST,
            target: "/_aws/execute-api/abc123/prod/a",
            headers: &[],
        },
        Unclaimed {
            case: "a path no rule of this load balancer is written for",
            host: EMULATOR_HOST,
            target: "/prod/orders",
            headers: &[],
        },
        Unclaimed {
            case: "a signed API call on the emulator's endpoint",
            host: EMULATOR_HOST,
            target: "/a",
            headers: &[(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-west-2/s3/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            )],
        },
        Unclaimed {
            case: "an unsigned API call carrying only the signing date",
            host: EMULATOR_HOST,
            target: "/a",
            headers: &[("x-amz-date", "20260101T000000Z")],
        },
    ];    for row in unclaimed {
        let answered = send(&deployed.router, "GET", row.host, row.target, row.headers).await;
        assert_ne!(
            answered.body, "a1",
            "case {:?}: the request is not this load balancer's traffic, got {} {:?}",
            row.case, answered.status, answered.body
        );
    }

    // A presigned call is an AWS API call too, however little else it presents.
    let answered = send(
        &deployed.router,
        "GET",
        EMULATOR_HOST,
        "/a?X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-west-2%2Fs3%2Faws4_request",
        &[],
    )
    .await;
    assert_ne!(
        answered.body, "a1",
        "case a presigned API call: the request is not this load balancer's traffic, got {} {:?}",
        answered.status, answered.body
    );

    // An `Authorization` that is NOT SigV4 is ordinary traffic through a load balancer —
    // the credential an application behind it reads — so it is still answered by the rule.
    for credential in ["Basic dXNlcjpwYXNz", "Bearer a-token"] {
        let answered = send(
            &deployed.router,
            "GET",
            EMULATOR_HOST,
            "/a",
            &[("authorization", credential)],
        )
        .await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (200, "a1"),
            "case {credential:?}: traffic carrying a non-SigV4 credential is the load \
             balancer's to answer, got {} {:?}",
            answered.status,
            answered.body
        );
    }
}

#[tokio::test]
async fn a_load_balancer_that_is_still_provisioning_serves_no_traffic_and_reads_change_nothing() {
    let deployed = deployment().await;

    // Before: the rules a read reports, and the health a read reports.
    let target_group = target_group_arn(&deployed);
    let rules_before = described_rules(&deployed.router, &deployed.lb_arn).await;
    let health_before = described_health(&deployed.router, &target_group).await;

    // A burst of data-plane requests, each answered with what its own rule says, so the
    // burst cannot pass on a wrong answer.
    let burst: &[(&str, u16, &str)] = &[
        ("/a", 200, "a1"),
        ("/b", 404, DEFAULT_BODY),
        ("/img/x", 200, "{\"ok\":true}"),
        ("/old", 301, ""),
        ("/tg", 502, ""),
        ("/empty", 503, ""),
        ("/auth", 500, ""),
    ];
    for (target, status, body) in burst {
        let answered = send(&deployed.router, "GET", &deployed.dns_name, target, &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (*status, *body),
            "case {target:?}: expected HTTP {status} {body:?}, got {} {:?}",
            answered.status,
            answered.body
        );
    }

    // Serving traffic moves no status, consumes no quota and bumps no counter: both reads
    // report exactly what they did before.
    let rules_after = described_rules(&deployed.router, &deployed.lb_arn).await;
    assert_eq!(
        rules_after, rules_before,
        "case rules after serving: a read must report what it did before, got {rules_after:?}"
    );
    let health_after = described_health(&deployed.router, &target_group).await;
    assert_eq!(
        health_after, health_before,
        "case target health after serving: a read must report what it did before, got \
         {health_after:?}"
    );

    // A load balancer the clock still owes its move out of `provisioning` is not serving
    // traffic on the name it published yet.
    settle(
        &deployed.engine,
        &deployed.lb_arn,
        SystemTime::now() + Duration::from_secs(3600),
    );
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/a", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (503, ""),
        "case provisioning: a load balancer still coming up serves nothing, got {} {:?}",
        answered.status,
        answered.body
    );

    // And once the move is due, the same request is answered from the same records.
    settle(
        &deployed.engine,
        &deployed.lb_arn,
        SystemTime::now() - Duration::from_secs(1),
    );
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/a", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, "a1"),
        "case active: the rule answers once the load balancer is active, got {} {:?}",
        answered.status,
        answered.body
    );
}

/// The ARN of the target group a rule forwards to, read out of the store the control plane
/// wrote it to.
fn target_group_arn(deployed: &Deployment) -> String {
    let state = deployed.engine.state::<ElbState>();
    let store = state.lock();
    store
        .name_to_tg_arn
        .get("dp-registered")
        .cloned()
        .expect("the fixture created the group")
}

/// The path the two source-ip rules below are both written for, and the body each of them
/// answers with: the one the `source-ip` rule serves, and the one the fall-through behind it
/// serves.
const SOURCE_IP_PATH: &str = "/src";
const BY_SOURCE_IP: &str = "by-source-ip";
const FELL_THROUGH: &str = "fell-through";

/// A load balancer whose HTTP:80 listener carries TWO rules for one path: a `source-ip` rule
/// at the lower priority value, and a condition-free fall-through behind it. Which of the two
/// answers is decided by the `source-ip` condition alone.
struct SourceIpDeployment {
    router: MultiServiceRouter,
    /// The address the load balancer published, which every case below DIALS.
    dns_name: String,
    listener_arn: String,
    /// The ARN of the `source-ip` rule, which each case re-points through `ModifyRule`.
    rule_arn: String,
}

/// Build that load balancer, with the condition pointed at the address a client on this
/// machine connects from. Every resource is written through the control plane's own
/// operations.
async fn source_ip_deployment(name: &str) -> SourceIpDeployment {
    let (router, engine) = emulator();
    let (lb_arn, dns_name) = create_load_balancer(&router, name).await;
    let listener_arn = create_listener(&router, &lb_arn).await;

    let mut fields = path_pattern(SOURCE_IP_PATH);
    fields.push(p("Conditions.member.2.Field", "source-ip"));
    fields.push(p(
        "Conditions.member.2.SourceIpConfig.Values.member.1",
        &format!("{CONNECTION_ADDRESS}/32"),
    ));
    fields.extend(fixed_response("200", BY_SOURCE_IP, None));
    let rule_arn = add_rule(&router, "source-ip at 20", &listener_arn, 20, &fields).await;

    let mut fields = path_pattern(SOURCE_IP_PATH);
    fields.extend(fixed_response("200", FELL_THROUGH, None));
    add_rule(&router, "the fall-through at 30", &listener_arn, 30, &fields).await;

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    SourceIpDeployment {
        router,
        dns_name,
        listener_arn,
        rule_arn,
    }
}

/// The source-ip condition's `Values`, as the fields of a `ModifyRule` request that leaves
/// the rule's path condition and its action exactly as they were.
fn source_ip_condition(values: &[&str]) -> Vec<(String, String)> {
    let mut fields = path_pattern(SOURCE_IP_PATH);
    fields.push(p("Conditions.member.2.Field", "source-ip"));
    for (index, value) in values.iter().enumerate() {
        fields.push(p(
            &format!(
                "Conditions.member.2.SourceIpConfig.Values.member.{}",
                index + 1
            ),
            value,
        ));
    }
    fields
}

/// Point the source-ip rule's condition at `values`, and answer what the call replied, so a
/// case can assert either the acceptance or the refusal.
async fn point_the_condition_at(deployed: &SourceIpDeployment, values: &[&str]) -> Reply {
    let mut form = action("ModifyRule");
    form.push(p("RuleArn", &deployed.rule_arn));
    form.extend(source_ip_condition(values));
    call(&deployed.router, &form).await
}

/// The source-ip rule as `DescribeRules` reports it, which is what a case reads a stored
/// condition back out of.
async fn described_source_ip_rule(deployed: &SourceIpDeployment) -> String {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &deployed.listener_arn));
    let reply = call(&deployed.router, &form).await;
    assert_eq!(
        reply.status, 200,
        "expected DescribeRules to answer 200, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "Rules")
        .into_iter()
        .find(|rule| rule.contains(&deployed.rule_arn))
        .unwrap_or_else(|| {
            panic!(
                "no rule {} in {}",
                deployed.rule_arn, reply.body
            )
        })
}

/// Send one request for the shared path over a REAL connection to the published address,
/// carrying one `X-Forwarded-For` field per entry of `forwarded`, and answer the body the
/// load balancer served.
async fn answered_on_the_connection(
    deployed: &SourceIpDeployment,
    case: &str,
    forwarded: &[&str],
) -> String {
    let mut request = format!(
        "GET {SOURCE_IP_PATH} HTTP/1.1\r\nHost: {}\r\n",
        deployed.dns_name
    );
    for claimed in forwarded {
        request.push_str(&format!("X-Forwarded-For: {claimed}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    let (status, raw) = raw_exchange(case, &deployed.dns_name, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected one of the two rules to answer HTTP 200, got {status} in \
         {raw:?}"
    );
    let served: Vec<&str> = [BY_SOURCE_IP, FELL_THROUGH]
        .into_iter()
        .filter(|body| raw.contains(body))
        .collect();
    assert_eq!(
        served.len(),
        1,
        "case {case:?}: expected exactly one of the two rules' bodies, got {served:?} in \
         {raw:?}"
    );
    served[0].to_string()
}

/// One row of the source-ip table: the blocks the condition is pointed at, what the client
/// claims about its own address, and which of the two rules must then answer.
struct SourceIp {
    case: &'static str,
    values: &'static [&'static str],
    forwarded: &'static [&'static str],
    body: &'static str,
}

/// The rows, spanning what the rule generalises over: a block holding exactly the address the
/// connection comes from and blocks holding it with room to spare, a block of several that
/// holds it, blocks that do not hold it, a block of the OTHER address family, and — against
/// both a matching and a non-matching block — every form the client's own claim takes (an
/// address inside the block, its own address, a chain of them, and none at all).
fn source_ip_rows() -> Vec<SourceIp> {
    // RFC 5737 reserves 192.0.2.0/24 and 198.51.100.0/24 for documentation, so no client of
    // this emulator connects from either.
    vec![
        SourceIp {
            case: "the connecting address as a /32",
            values: &["127.0.0.1/32"],
            forwarded: &[],
            body: BY_SOURCE_IP,
        },
        SourceIp {
            case: "a block that holds the connecting address",
            values: &["127.0.0.0/8"],
            forwarded: &[],
            body: BY_SOURCE_IP,
        },
        SourceIp {
            case: "the whole IPv4 space",
            values: &["0.0.0.0/0"],
            forwarded: &[],
            body: BY_SOURCE_IP,
        },
        SourceIp {
            case: "one block of several holds it",
            values: &["192.0.2.0/24", "198.51.100.0/24", "127.0.0.1/32"],
            forwarded: &[],
            body: BY_SOURCE_IP,
        },
        SourceIp {
            case: "a matching block, with the client claiming another address",
            values: &["127.0.0.1/32"],
            forwarded: &["192.0.2.7"],
            body: BY_SOURCE_IP,
        },
        SourceIp {
            case: "a block that does not hold the connecting address",
            values: &["192.0.2.0/24"],
            forwarded: &[],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "the client claims an address inside that block",
            values: &["192.0.2.0/24"],
            forwarded: &["192.0.2.7"],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "the client claims the address it connects from",
            values: &["192.0.2.0/24"],
            forwarded: &[CONNECTION_ADDRESS],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "the client claims a whole chain inside that block",
            values: &["192.0.2.0/24"],
            forwarded: &["192.0.2.7, 198.51.100.4", "192.0.2.9"],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "a block of several, none of which holds it",
            values: &["192.0.2.0/24", "198.51.100.0/24"],
            forwarded: &[],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "the whole IPv6 space, against an IPv4 connection",
            values: &["::/0"],
            forwarded: &[],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "the IPv6 loopback claimed by the client",
            values: &["::1/128"],
            forwarded: &["::1"],
            body: FELL_THROUGH,
        },
        SourceIp {
            case: "the /32 restored after all of them",
            values: &["127.0.0.1/32"],
            forwarded: &[],
            body: BY_SOURCE_IP,
        },
    ]
}

/// RULE: a `source-ip` condition is met exactly when the address the CONNECTION came from
/// lies in one of the CIDR blocks its `SourceIpConfig.Values` names — never because of an
/// `X-Forwarded-For` the client supplied — and the rule carrying it wins over a rule at a
/// higher priority value only while it is met.
#[tokio::test]
async fn a_source_ip_condition_is_met_by_the_connecting_address_and_never_by_x_forwarded_for() {
    let deployed = source_ip_deployment("source-ip-alb").await;

    for row in source_ip_rows() {
        let reply = point_the_condition_at(&deployed, row.values).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected ModifyRule to answer 200, got {} body {}",
            row.case, reply.status, reply.body
        );
        // The blocks a case is driven against are the ones the control plane REPORTS, so no
        // row rests on a condition a test believes it wrote.
        let described = described_source_ip_rule(&deployed).await;
        for value in row.values {
            assert!(
                described.contains(&format!("<member>{value}</member>")),
                "case {:?}: expected DescribeRules to echo the block {value:?}, got \
                 {described:?}",
                row.case
            );
        }
        let served = answered_on_the_connection(&deployed, row.case, row.forwarded).await;
        assert_eq!(
            served, row.body,
            "case {:?}: expected the body {:?}, got {served:?}",
            row.case, row.body
        );
    }
}

/// One row of the condition-value table: the value, and whether it names an address block at
/// all.
struct ConditionValue {
    case: &'static str,
    value: &'static str,
    accepted: bool,
}

/// The values, spanning what the rule generalises over: each address family, each published
/// prefix bound and one past it, a value carrying no prefix, a wildcard ("wildcards are not
/// supported"), text that is no address, and the all-ones IPv4 address.
fn condition_values() -> Vec<ConditionValue> {
    vec![
        ConditionValue {
            case: "an IPv4 block",
            value: "10.0.0.0/8",
            accepted: true,
        },
        ConditionValue {
            case: "the whole IPv4 space, the lowest prefix length",
            value: "0.0.0.0/0",
            accepted: true,
        },
        ConditionValue {
            case: "one IPv4 address, the highest prefix length",
            value: "203.0.113.4/32",
            accepted: true,
        },
        ConditionValue {
            case: "one past the highest IPv4 prefix length",
            value: "10.0.0.0/33",
            accepted: false,
        },
        ConditionValue {
            case: "an IPv6 block",
            value: "2001:db8::/32",
            accepted: true,
        },
        ConditionValue {
            case: "the whole IPv6 space, the lowest prefix length",
            value: "::/0",
            accepted: true,
        },
        ConditionValue {
            case: "one IPv6 address, the highest prefix length",
            value: "::1/128",
            accepted: true,
        },
        ConditionValue {
            case: "one past the highest IPv6 prefix length",
            value: "::1/129",
            accepted: false,
        },
        ConditionValue {
            case: "an address carrying no prefix length",
            value: "10.0.0.1",
            accepted: false,
        },
        ConditionValue {
            case: "a wildcard, which the member does not support",
            value: "10.0.*.*/16",
            accepted: false,
        },
        ConditionValue {
            case: "text that is no address at all",
            value: "everyone/0",
            accepted: false,
        },
        ConditionValue {
            case: "the all-ones address, which no connection comes from",
            value: "255.255.255.255/32",
            accepted: false,
        },
    ]
}

/// The refusal a value that is no address block earns, in Elastic Load Balancing's own
/// wording. Read from the service's own one statement of it, so the suite and the handler
/// cannot drift on the text.
fn invalid_block_refusal(value: &str) -> String {
    service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::invalid_cidr_block_message(value)
}

/// RULE: every value of a `source-ip` condition must name a CIDR block; one that does not is
/// refused HTTP 400 `ValidationError` naming the value, by every operation that SETS a rule's
/// conditions, and the stored rule is left exactly as it was.
#[tokio::test]
async fn a_source_ip_condition_value_that_is_no_cidr_block_is_refused_and_changes_nothing() {
    let deployed = source_ip_deployment("source-ip-values-alb").await;
    // The condition as the fixture stored it, which every refused call must leave in place.
    let stored = format!("{CONNECTION_ADDRESS}/32");

    for row in condition_values() {
        let before = described_source_ip_rule(&deployed).await;
        let reply = point_the_condition_at(&deployed, &[row.value]).await;
        if row.accepted {
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected ModifyRule to accept {:?}, got {} body {}",
                row.case, row.value, reply.status, reply.body
            );
            let described = described_source_ip_rule(&deployed).await;
            assert!(
                described.contains(&format!("<member>{}</member>", row.value)),
                "case {:?}: expected the stored condition to carry {:?}, got {described:?}",
                row.case,
                row.value
            );
            // Back to the condition the next row is judged against, so one accepted row
            // cannot decide the next one's "unchanged".
            let restored = point_the_condition_at(&deployed, &[stored.as_str()]).await;
            assert_eq!(
                restored.status, 200,
                "case {:?}: expected the fixture's own block to be accepted again, got {} \
                 body {}",
                row.case, restored.status, restored.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected ModifyRule to refuse {:?} with HTTP 400, got {} body {}",
            row.case, row.value, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationError",
            "case {:?}: expected the ValidationError code, got {:?} in {}",
            row.case,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            unescaped(&reply.text("Message").unwrap_or_default()),
            invalid_block_refusal(row.value),
            "case {:?}: expected the refusal to name the value, got {:?}",
            row.case,
            reply.text("Message")
        );
        // The refused call changed nothing: the rule reads back exactly as it did, and the
        // listener still answers by the block it was already carrying.
        let after = described_source_ip_rule(&deployed).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused call must leave the rule as it was, got {after:?}",
            row.case
        );
        let served = answered_on_the_connection(&deployed, row.case, &[]).await;
        assert_eq!(
            served, BY_SOURCE_IP,
            "case {:?}: expected the stored condition to still answer, got {served:?}",
            row.case
        );
    }

    // The same clause on the operation that FIRST stores a rule's conditions: the rule is
    // not created, so the priority it named is still free afterwards.
    let mut fields = source_ip_condition(&["255.255.255.255/32"]);
    fields.extend(fixed_response("200", BY_SOURCE_IP, None));
    let mut form = action("CreateRule");
    form.push(p("ListenerArn", &deployed.listener_arn));
    form.push(p("Priority", "40"));
    form.extend(fields);
    let reply = call(&deployed.router, &form).await;
    assert_eq!(
        reply.status, 400,
        "case CreateRule with a value that is no block: expected HTTP 400, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ValidationError",
        "case CreateRule with a value that is no block: expected the ValidationError code, \
         got {:?} in {}",
        reply.error_code(),
        reply.body
    );
    assert_eq!(
        unescaped(&reply.text("Message").unwrap_or_default()),
        invalid_block_refusal("255.255.255.255/32"),
        "case CreateRule with a value that is no block: expected the refusal to name the \
         value, got {:?}",
        reply.text("Message")
    );
    let mut fields = path_pattern("/free");
    fields.extend(fixed_response("200", FELL_THROUGH, None));
    add_rule(
        &deployed.router,
        "the priority the refused create named",
        &deployed.listener_arn,
        40,
        &fields,
    )
    .await;
}

/// The paths the published-address table below writes rules for, and the body each of the two
/// load balancers answers the shared one with — different bodies for one path, so a row that
/// reached the other load balancer fails rather than passing.
const PUBLISHED_PATH: &str = "/status";
const GONE_PATH: &str = "/withdrawn";
const FIRST_BODY: &str = "the-first-load-balancer";
const SECOND_BODY: &str = "the-second-load-balancer";
const GONE_BODY: &str = "withdrawn-by-the-first-load-balancer";

/// The two paths whose rules FORWARD: one to a group holding no registration at all, one to a
/// group holding the address a network interface of the account holds.
const FORWARD_TO_NO_TARGET_PATH: &str = "/to-an-empty-group";
const FORWARD_TO_A_REGISTRATION_PATH: &str = "/to-a-registered-address";

/// The status a load balancer answers a `forward` whose group has no target it may send the
/// request to: "if a target group contains no registered targets, or if a target group
/// contains only unhealthy targets, the load balancer returns an HTTP 503".
const NO_HEALTHY_TARGET: u16 = 503;

/// The status it answers a request whose chosen target cannot be connected to — which is what
/// a registered ADDRESS is in this emulator, because nothing of this emulator listens on one.
/// What the row using it asserts is that the answer came from the load balancer at the address
/// it published, not what a server behind the address would have returned.
const TARGET_UNREACHABLE: u16 = 502;

/// The network the registered address belongs to, and the address itself: the shape an
/// `awsvpc` workload's own address has — a private address inside one subnet of one VPC, held
/// by a network interface of the account.
pub(crate) const WORKLOAD_VPC: &str = "vpc-0aa1b2c3d4e5f6071";
const WORKLOAD_SUBNET: &str = "subnet-0aa1b2c3d4e5f6071";
const WORKLOAD_ZONE: &str = "us-west-2a";
pub(crate) const WORKLOAD_ADDRESS: &str = "10.80.1.10";

/// The cadence the groups below are health-checked at: the shortest interval
/// `HealthCheckIntervalSeconds` admits ("the range is 5-300"), so the verdict a row waits for
/// is reached at the service's own pace rather than after a wait of this suite's choosing.
const HEALTH_CHECK_INTERVAL_SECONDS: i32 = 5;

/// How long a row waits for a verdict or for a withdrawn address to stop answering, and how
/// often it looks again. Generous on purpose — many times the cadence above — because the
/// deadline is only here so a failure is a failure rather than a hang.
const SETTLING_DEADLINE: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How long one dial of a published address is given, for the probe that asks whether the
/// address still answers at all.
const DIAL_TIMEOUT: Duration = Duration::from_secs(60);

/// Seed the account's EC2 inventory with one VPC, one subnet of it and one network interface
/// holding [`WORKLOAD_ADDRESS`], so a registration of that address is one the emulator holds
/// something behind and the load balancer's own health checks report it `healthy`.
///
/// Written through the inventory's own writer seam, so the interface is the record EC2's own
/// readers answer with rather than a value this suite made up.
pub(crate) fn seed_workload_interface(engine: &Arc<engine::InMemoryEngine>) {
    let state = ec2::service_state(&**engine).expect("the shared engine holds an EC2 inventory");
    let mut store = state.lock();
    store.vpcs.insert(
        WORKLOAD_VPC.to_string(),
        ec2_types::Vpc {
            vpc_id: Some(WORKLOAD_VPC.to_string()),
            cidr_block: Some("10.80.0.0/16".to_string()),
            ..ec2_types::Vpc::default()
        },
    );
    store.subnets.insert(
        WORKLOAD_SUBNET.to_string(),
        ec2_types::Subnet {
            subnet_id: Some(WORKLOAD_SUBNET.to_string()),
            vpc_id: Some(WORKLOAD_VPC.to_string()),
            cidr_block: Some("10.80.1.0/24".to_string()),
            availability_zone: Some(WORKLOAD_ZONE.to_string()),
            ..ec2_types::Subnet::default()
        },
    );
    store
        .insert_network_interface(ec2::NetworkInterfaceSpec {
            subnet_id: WORKLOAD_SUBNET.to_string(),
            vpc_id: WORKLOAD_VPC.to_string(),
            availability_zone: WORKLOAD_ZONE.to_string(),
            region: crate::provisioning_loop::REGION.to_string(),
            description: "a workload of the account".to_string(),
            interface_type: ec2::NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
            private_ip_addresses: vec![WORKLOAD_ADDRESS.to_string()],
            ..ec2::NetworkInterfaceSpec::default()
        })
        .expect("the inventory holds the interface");
}

/// Create an `ip` target group in `vpc` — the target type a workload addressed by its own
/// private address is registered under — and answer its ARN.
pub(crate) async fn ip_target_group(
    router: &MultiServiceRouter,
    name: &str,
    vpc: &str,
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", vpc));
    form.push(p(
        "HealthCheckIntervalSeconds",
        &HEALTH_CHECK_INTERVAL_SECONDS.to_string(),
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200 from CreateTargetGroup, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case create {name}: no TargetGroupArn in {}", reply.body))
}

/// Register `address` with the target group `arn` names.
pub(crate) async fn register_address(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    address: &str,
) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", address));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// A `forward` action's fields, naming the one target group it forwards to.
pub(crate) fn forward_to(target_group_arn: &str) -> Vec<(String, String)> {
    vec![
        p("Actions.member.1.Type", "forward"),
        p("Actions.member.1.TargetGroupArn", target_group_arn),
    ]
}

/// The `DNSName` a later read reports for the load balancer `arn` names.
async fn described_dns_name(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DescribeLoadBalancers, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("DNSName")
        .unwrap_or_else(|| panic!("case {case:?}: no DNSName in {}", reply.body))
}

/// Wait until every registration of the group `arn` names reports `wanted`, looking again
/// rather than sleeping for the span the checks take, and answer the states it reported.
pub(crate) async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> Vec<String> {
    let deadline = Instant::now() + SETTLING_DEADLINE;
    loop {
        let reported: Vec<String> = described_health(router, arn)
            .await
            .iter()
            .map(|description| first_text(description, "State").unwrap_or_default())
            .collect();
        let settled = !reported.is_empty() && reported.iter().all(|state| state == wanted);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every registration to reach {wanted:?}, got {reported:?}"
            );
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Whether `published` still ANSWERS: a connection it accepts, and a request on it that comes
/// back with bytes. A closed port, a connection nothing reads and an empty answer are all an
/// address this emulator no longer serves.
async fn still_answers(published: &str) -> bool {
    let Some(port) = published
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
    else {
        return false;
    };
    let dialed = tokio::time::timeout(
        DIAL_TIMEOUT,
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await;
    let Ok(Ok(mut stream)) = dialed else {
        return false;
    };
    let request =
        format!("GET / HTTP/1.1\r\nHost: {published}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut answer = Vec::new();
    match tokio::time::timeout(DIAL_TIMEOUT, stream.read_to_end(&mut answer)).await {
        Ok(Ok(_read)) => !answer.is_empty(),
        _ => false,
    }
}

/// The body of one answer read off a connection: everything after the blank line that ends
/// its head.
fn body_of(case: &str, raw: &str) -> String {
    raw.split_once("\r\n\r\n")
        .map(|(_head, body)| body.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: no head and body in {raw:?}"))
}

/// Which of the two load balancers' published addresses a row dials.
#[derive(Clone, Copy)]
enum Dialed {
    First,
    Second,
}

/// One row of the published-address table: the address it dials, the request line it writes,
/// and the answer that address must give it.
struct Published {
    case: &'static str,
    dialed: Dialed,
    method: &'static str,
    target: &'static str,
    status: u16,
    body: &'static str,
}

/// The rows, spanning what the rule generalises over: which of the two addresses is dialled,
/// each terminating action a listener holds (a rule's fixed response, the listener's default
/// action, a `forward`), the method and query a client writes beside the path, a path the
/// address dialled has a rule for and one it has none for, and the same path on both
/// addresses.
fn published_rows() -> Vec<Published> {
    vec![
        Published {
            case: "the rule the dialled load balancer holds for the path",
            dialed: Dialed::First,
            method: "GET",
            target: PUBLISHED_PATH,
            status: 200,
            body: FIRST_BODY,
        },
        Published {
            case: "the same path asked for with another method",
            dialed: Dialed::First,
            method: "POST",
            target: PUBLISHED_PATH,
            status: 200,
            body: FIRST_BODY,
        },
        Published {
            case: "the same path carrying a query string",
            dialed: Dialed::First,
            method: "GET",
            target: "/status?probe=1",
            status: 200,
            body: FIRST_BODY,
        },
        Published {
            case: "another rule of the same listener, answering another status",
            dialed: Dialed::First,
            method: "GET",
            target: GONE_PATH,
            status: 410,
            body: GONE_BODY,
        },
        Published {
            case: "a path no rule of the dialled load balancer matches",
            dialed: Dialed::First,
            method: "GET",
            target: "/nothing-here",
            status: 404,
            body: DEFAULT_BODY,
        },
        Published {
            case: "a forward to a group holding no registration",
            dialed: Dialed::First,
            method: "GET",
            target: FORWARD_TO_NO_TARGET_PATH,
            status: NO_HEALTHY_TARGET,
            body: "",
        },
        Published {
            case: "a forward to the registered address of a healthy workload",
            dialed: Dialed::First,
            method: "GET",
            target: FORWARD_TO_A_REGISTRATION_PATH,
            status: TARGET_UNREACHABLE,
            body: "",
        },
        Published {
            case: "the other load balancer's own rule for the shared path",
            dialed: Dialed::Second,
            method: "GET",
            target: PUBLISHED_PATH,
            status: 200,
            body: SECOND_BODY,
        },
        Published {
            case: "a path only the first load balancer has a rule for",
            dialed: Dialed::Second,
            method: "GET",
            target: FORWARD_TO_NO_TARGET_PATH,
            status: 404,
            body: DEFAULT_BODY,
        },
    ]
}

/// Two load balancers, each with an HTTP:80 listener of its own, and the groups the first
/// one's rules forward to.
struct PublishedDeployment {
    router: MultiServiceRouter,
    first_arn: String,
    /// The address the first load balancer published, which the rows above dial.
    first: String,
    second_arn: String,
    /// The address the second one published.
    second: String,
    /// The group holding the workload's own address, whose health a row waits for.
    registered_group: String,
}

/// Build them, every resource through the control plane's own operations.
async fn published_deployment() -> PublishedDeployment {
    let (router, engine) = emulator();
    seed_workload_interface(&engine);

    let (first_arn, first) = create_load_balancer(&router, "published-one").await;
    let listener_arn = create_listener(&router, &first_arn).await;
    let empty_group = ip_target_group(&router, "published-empty", WORKLOAD_VPC).await;
    let registered_group = ip_target_group(&router, "published-live", WORKLOAD_VPC).await;
    register_address(
        &router,
        "the workload's own address",
        &registered_group,
        WORKLOAD_ADDRESS,
    )
    .await;

    let mut fields = path_pattern(PUBLISHED_PATH);
    fields.extend(fixed_response("200", FIRST_BODY, None));
    add_rule(&router, "the first load balancer's path", &listener_arn, 10, &fields).await;
    let mut fields = path_pattern(GONE_PATH);
    fields.extend(fixed_response("410", GONE_BODY, None));
    add_rule(&router, "the withdrawn path", &listener_arn, 20, &fields).await;
    let mut fields = path_pattern(FORWARD_TO_NO_TARGET_PATH);
    fields.extend(forward_to(&empty_group));
    add_rule(&router, "the forward to an empty group", &listener_arn, 30, &fields).await;
    let mut fields = path_pattern(FORWARD_TO_A_REGISTRATION_PATH);
    fields.extend(forward_to(&registered_group));
    add_rule(&router, "the forward to a registration", &listener_arn, 40, &fields).await;

    // A SECOND load balancer, with an address of its own and its own answer for the path the
    // first one answers: the address dialled is what decides which of them serves a request.
    let (second_arn, second) = create_load_balancer(&router, "published-two").await;
    let other_listener_arn = create_listener(&router, &second_arn).await;
    let mut fields = path_pattern(PUBLISHED_PATH);
    fields.extend(fixed_response("200", SECOND_BODY, None));
    add_rule(
        &router,
        "the second load balancer's path",
        &other_listener_arn,
        10,
        &fields,
    )
    .await;

    // The moves to `active` the clock owes both of them, placed rather than slept for.
    let past = SystemTime::now() - Duration::from_secs(1);
    settle(&engine, &first_arn, past);
    settle(&engine, &second_arn, past);

    PublishedDeployment {
        router,
        first_arn,
        first,
        second_arn,
        second,
        registered_group,
    }
}

/// RULE: the value a load balancer publishes as its `DNSName` is an address of THIS emulator —
/// one a client on this machine dials — reported identically by the create and by every later
/// read, held by one load balancer alone, answered out of that load balancer's own stored
/// configuration, and withdrawn when the load balancer is deleted.
#[tokio::test]
async fn the_address_a_load_balancer_publishes_is_an_address_of_this_emulator_that_answers_for_it()
{
    let deployed = published_deployment().await;

    // Each published value is an address a client dials — a host and a port, on an interface
    // of this machine — and the read reports the very value the create answered with.
    for (case, arn, published) in [
        (
            "the first load balancer",
            &deployed.first_arn,
            &deployed.first,
        ),
        (
            "the second load balancer",
            &deployed.second_arn,
            &deployed.second,
        ),
    ] {
        let dialable: std::net::SocketAddr = published.parse().unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected an address of this emulator a client can dial, got \
                 {published:?} ({error})"
            )
        });
        assert!(
            dialable.ip().is_loopback(),
            "case {case:?}: expected an address on an interface of this machine, got \
             {published:?}"
        );
        assert_ne!(
            dialable.port(), 0,
            "case {case:?}: expected the port the socket answers on, got {published:?}"
        );
        let reported = described_dns_name(&deployed.router, case, arn).await;
        assert_eq!(
            reported, *published,
            "case {case:?}: a read must report the address the create published, got \
             {reported:?}"
        );
    }
    assert_ne!(
        deployed.first, deployed.second,
        "expected each load balancer to publish an address of its own, {:?} repeated",
        deployed.first
    );

    // The registration of an address a network interface of the account holds is one the load
    // balancer sends requests to, so the forward row below is a forward to a HEALTHY target
    // rather than to a group with nothing in it.
    health_reaching(
        &deployed.router,
        "the workload's own address",
        &deployed.registered_group,
        "healthy",
    )
    .await;

    for row in published_rows() {
        let published = match row.dialed {
            Dialed::First => &deployed.first,
            Dialed::Second => &deployed.second,
        };
        let request = format!(
            "{} {} HTTP/1.1\r\nHost: {published}\r\nConnection: close\r\n\r\n",
            row.method, row.target
        );
        let (status, raw) = raw_exchange(row.case, published, &request).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected the address it published to answer HTTP {}, got {status} in \
             {raw:?}",
            row.case, row.status
        );
        let body = body_of(row.case, &raw);
        assert_eq!(
            body, row.body,
            "case {:?}: expected the body {:?}, got {body:?}",
            row.case, row.body
        );
    }

    // The address belongs to the load balancer: deleting one withdraws its address, and the
    // load balancer beside it keeps serving on its own.
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &deployed.second_arn));
    let deleted = call(&deployed.router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case delete the second load balancer: expected 200, got {} body {}",
        deleted.status, deleted.body
    );
    let deadline = Instant::now() + SETTLING_DEADLINE;
    while still_answers(&deployed.second).await {
        assert!(
            Instant::now() < deadline,
            "case the deleted load balancer's address: expected {} to stop answering",
            deployed.second
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    let case = "the load balancer beside the deleted one";
    let request = format!(
        "GET {PUBLISHED_PATH} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        deployed.first
    );
    let (status, raw) = raw_exchange(case, &deployed.first, &request).await;
    assert_eq!(
        (status, body_of(case, &raw).as_str()),
        (200, FIRST_BODY),
        "case {case:?}: expected its own address to keep serving, got {status} in {raw:?}"
    );
}
