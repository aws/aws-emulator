// SPDX-License-Identifier: CC0-1.0

//! Target group attributes, health-check modification, target deregistration and the
//! refusals each of those answers — over the wire, through the router.
//!
//! The rules these tables pin, and where each comes from:
//!
//! * An ELBv2 operation that names ONE target group and finds none refuses with its
//!   modelled `TargetGroupNotFound` (awsQuery code `TargetGroupNotFound`, HTTP 400 —
//!   `smithy_query(query="operation",operation="…")`), in an `<ErrorResponse>` that
//!   carries the `<RequestId>` AWS puts beside `<Error>`; never a 501, which would say
//!   nobody has written the operation, and never `InvalidAction`, which would say the
//!   API has no such action.
//! * A target group reports every attribute the model documents for a group of its
//!   shape, carrying the documented default until a write changes it
//!   (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")` is the only
//!   catalog of them, since the Smithy shape itself is an unconstrained key/value pair).
//! * A value outside an attribute's documented domain and a key the model does not
//!   document are each refused with `ValidationError`, a key another load balancer
//!   family's groups carry with the operation's own `InvalidConfigurationRequest`, and a
//!   refused write stores nothing.
//! * `ModifyTargetGroup` replaces the health-check members the request names and leaves
//!   every other one as it was.
//! * `CreateTargetGroup` refuses a `Name` outside the rule its own documentation states
//!   with `ValidationError` and a message naming the value.
//!
//! The wire harness (`router`, `call`, `action`, `p`, `members`, `first_text`) is the one
//! `provisioning_loop` already owns; nothing here restates it.

use crate::provisioning_loop::{
    account, action, call, create_http_target_group, emulator, first_text, members, p, REGION,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// One row of the not-found table: the case name, the request's form fields, and the
/// message the refusal must answer.
type NotFoundCase = (&'static str, Vec<(String, String)>, String);

/// A key this service defines no target group attribute under, which no row of the write
/// table's accepted half names.
const UNDOCUMENTED_ATTRIBUTE_KEY: &str = "not_a_real_attribute";

/// The awsQuery code of `InvalidConfigurationRequestException`
/// (`smithy_query(query="operation",operation="ModifyTargetGroupAttributes")`).
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The code Elastic Load Balancing answers an input value it cannot accept with, for a
/// rule its model documents in prose and declares no error shape for.
const VALIDATION_ERROR: &str = "ValidationError";

/// The keys the write table's refused rows name that this target group does not CARRY at
/// all — an attribute of another load balancer family, and one of a Lambda target — as
/// opposed to a key it carries written with a value outside that attribute's domain.
const KEYS_THE_GROUP_DOES_NOT_CARRY: &[&str] =
    &["proxy_protocol_v2.enabled", "lambda.multi_value_headers.enabled"];

/// The code a refused attribute write answers under: an attribute this target group does
/// not carry is a configuration it cannot hold, while a key the service defines no
/// attribute for and a value outside an attribute's documented domain are both inputs the
/// request has to fix.
fn refusal_code(key: &str) -> &'static str {
    if KEYS_THE_GROUP_DOES_NOT_CARRY.contains(&key) {
        INVALID_CONFIGURATION
    } else {
        VALIDATION_ERROR
    }
}

/// One row of the attribute-default table: the case name, the fields the target group is
/// created with, and the whole attribute set it must then report.
type AttributeDefaultCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    BTreeMap<String, String>,
);

/// One row of the health-check refusal table: the case name, the target group it is
/// asked of, the `ModifyTargetGroup` fields, and the expected awsQuery error code (empty
/// for a row that must be served).
type HealthCheckCase<'a> = (
    &'static str,
    &'a String,
    Vec<(&'static str, &'static str)>,
    &'static str,
);

/// One row of the deregistration table: the case name, the fields naming what to
/// deregister, and the registrations that must remain.
type DeregisterCase = (
    &'static str,
    Vec<(String, String)>,
    Vec<(String, String)>,
);

/// A target group ARN of the right shape that no target group in the store carries.
fn absent_target_group_arn() -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/not-here/0123456789abcdef",
        account()
    )
}

/// The text of an element with the five XML entities the serializer writes resolved, so
/// an assertion compares the VALUE rather than its encoding (the framework's XML codec
/// escapes `'` as `&apos;`).
///
/// `pub(crate)` because `all/target_validation_and_quotas.rs` and the listener suite
/// compare refusal messages the same way: ONE reader for the crate, so the suites cannot
/// drift on the encoding.
pub(crate) fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Create a target group named `name` with `fields` beside it, and answer its ARN.
///
/// `pub(crate)` because `all/target_group_attribute_writes.rs` creates target groups of
/// every load balancer family the same way: ONE creator for the crate, so the two suites
/// cannot drift on how a group is made.
pub(crate) async fn create_target_group(
    router: &MultiServiceRouter,
    name: &str,
    fields: &[(&str, &str)],
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    for (key, value) in fields {
        form.push(p(key, value));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case create {name}: no TargetGroupArn in {}", reply.body))
}

/// Every key/value pair in the first `Attributes` list of `body`.
fn attribute_pairs(body: &str) -> BTreeMap<String, String> {
    members(body, "Attributes")
        .iter()
        .map(|member| {
            (
                first_text(member, "Key").unwrap_or_default(),
                first_text(member, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// What `DescribeTargetGroupAttributes` reports for `arn`.
///
/// `pub(crate)` for the reason [`create_target_group`] gives: the sibling write suite
/// reads a group's whole attribute set back through the same reader.
pub(crate) async fn described_attributes(
    router: &MultiServiceRouter,
    arn: &str,
) -> BTreeMap<String, String> {
    let mut form = action("DescribeTargetGroupAttributes");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case describe attributes of {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    attribute_pairs(&reply.body)
}

/// Write one attribute and answer the reply, whatever it is.
async fn modify_attribute(
    router: &MultiServiceRouter,
    arn: &str,
    key: &str,
    value: &str,
) -> crate::provisioning_loop::Reply {
    let mut form = action("ModifyTargetGroupAttributes");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Attributes.member.1.Key", key));
    form.push(p("Attributes.member.1.Value", value));
    call(router, &form).await
}

// ---------------------------------------------------------------------------
// An absent target group, across every operation that names one.
// ---------------------------------------------------------------------------

/// Each operation that takes a target group and finds none answers the modelled
/// `TargetGroupNotFound`, with a request id in the envelope.
///
/// The rows span the shapes of such an operation: a list read, a read of one group's
/// attributes, two writes of one group, and the two registration calls. A `Describe`
/// over a LIST of identifiers names the identifiers it could not resolve, rendered as that
/// list; an operation naming ONE `TargetGroupArn` names that ARN without a list around it.
#[tokio::test]
async fn every_operation_naming_an_absent_target_group_answers_the_modelled_refusal() {
    let (router, _engine) = emulator();
    let absent = absent_target_group_arn();

    // (case, form fields, the message the refusal must answer)
    //
    // The wording follows the shape of the lookup, as the recorded answers do: a
    // `Describe` over a LIST of identifiers names what it could not resolve out of that
    // list, while an operation acting on ONE `TargetGroupArn` names that identifier with no
    // list around it.
    let selection = |identifier: &str| format!("Target groups '[{identifier}]' not found");
    let named = |identifier: &str| format!("Target groups '{identifier}' not found");
    let cases: Vec<NotFoundCase> = vec![
        (
            "DescribeTargetGroups by ARN",
            vec![p("TargetGroupArns.member.1", &absent)],
            selection(&absent),
        ),
        (
            "DescribeTargetGroups by name",
            vec![p("Names.member.1", "not-here")],
            selection("not-here"),
        ),
        (
            "DescribeTargetGroupAttributes",
            vec![p("TargetGroupArn", &absent)],
            named(&absent),
        ),
        (
            "ModifyTargetGroup",
            vec![p("TargetGroupArn", &absent), p("HealthCheckPath", "/up")],
            named(&absent),
        ),
        (
            "ModifyTargetGroupAttributes",
            vec![
                p("TargetGroupArn", &absent),
                p("Attributes.member.1.Key", "stickiness.enabled"),
                p("Attributes.member.1.Value", "true"),
            ],
            named(&absent),
        ),
        (
            "RegisterTargets",
            vec![
                p("TargetGroupArn", &absent),
                p("Targets.member.1.Id", "i-11111111111111111"),
            ],
            named(&absent),
        ),
        (
            "DeregisterTargets",
            vec![
                p("TargetGroupArn", &absent),
                p("Targets.member.1.Id", "i-11111111111111111"),
            ],
            named(&absent),
        ),
    ];

    for (case, fields, message) in cases {
        let operation = case.split(' ').next().expect("a case names its operation");
        let mut form = action(operation);
        form.extend(fields);
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "TargetGroupNotFound"),
            "case {case:?}: expected 400 TargetGroupNotFound, got {} body {}",
            reply.status,
            reply.body
        );
        let request_id = reply.text("RequestId");
        assert!(
            request_id.is_some_and(|id| id.split('-').map(str::len).eq([8, 4, 4, 4, 12])),
            "case {case:?}: an error envelope must carry a request id, got {}",
            reply.body
        );
        assert_eq!(
            reply.text("Message").as_deref().map(unescaped),
            Some(message),
            "case {case:?}: unexpected message in {}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// Attribute defaults, per load balancer family.
// ---------------------------------------------------------------------------

/// Every attribute a target group reports, and the default each carries, follow the
/// model's own `TargetGroupAttribute$Key` documentation — which groups them by the load
/// balancer family the target group's protocol belongs to.
///
/// The rows span every family that documentation names (Application, Network, Gateway)
/// plus the two target types that change the set (`lambda`, and an `ip` target group
/// whose protocol changes one default), so each branch of the catalog is exercised
/// rather than only the HTTP row the issue recorded.
#[tokio::test]
async fn attribute_defaults_follow_the_documented_catalog_of_the_load_balancer_family() {
    // Shared by all load balancers, plus the ones documented for "Application Load
    // Balancers and Network Load Balancers".
    let all_and_cross_zone: &[(&str, &str)] = &[
        ("load_balancing.cross_zone.enabled", "use_load_balancer_configuration"),
        ("stickiness.enabled", "false"),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.count",
            "1",
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            "off",
        ),
        (
            "target_group_health.unhealthy_state_routing.minimum_healthy_targets.count",
            "1",
        ),
        (
            "target_group_health.unhealthy_state_routing.minimum_healthy_targets.percentage",
            "off",
        ),
    ];
    // Documented "only if the load balancer is an Application Load Balancer and the
    // target is an instance or an IP address".
    let application_addressable: &[(&str, &str)] = &[
        ("load_balancing.algorithm.anomaly_mitigation", "off"),
        ("load_balancing.algorithm.type", "round_robin"),
        ("slow_start.duration_seconds", "0"),
        // A cookie name the documentation states no value for: the collection carries it
        // at the empty string.
        ("stickiness.app_cookie.cookie_name", ""),
        ("stickiness.app_cookie.duration_seconds", "86400"),
        ("stickiness.lb_cookie.duration_seconds", "86400"),
    ];
    // Documented "only by Network Load Balancers".
    let network_only: &[(&str, &str)] = &[
        ("proxy_protocol_v2.enabled", "false"),
        (
            "target_health_state.unhealthy.connection_termination.enabled",
            "true",
        ),
        (
            "target_health_state.unhealthy.draining_interval_seconds",
            "0",
        ),
    ];
    // Documented "only by Gateway Load Balancers".
    let gateway_only: &[(&str, &str)] = &[
        ("send_tcp_reset.on_deregistration.enabled", "false"),
        ("send_tcp_reset.on_unhealthy.enabled", "false"),
        ("target_failover.on_deregistration", "no_rebalance"),
        ("target_failover.on_unhealthy", "no_rebalance"),
    ];

    let join = |parts: &[&[(&str, &str)]]| -> BTreeMap<String, String> {
        parts
            .iter()
            .flat_map(|group| group.iter())
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };

    // (case, create fields, the whole attribute set the group must report)
    let cases: Vec<AttributeDefaultCase> = vec![
        (
            "an HTTP instance target group (Application)",
            vec![("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
            join(&[
                all_and_cross_zone,
                application_addressable,
                &[
                    ("deregistration_delay.timeout_seconds", "300"),
                    ("stickiness.type", "lb_cookie"),
                ],
            ]),
        ),
        (
            "a TCP instance target group (Network)",
            vec![("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")],
            join(&[
                all_and_cross_zone,
                network_only,
                &[
                    ("deregistration_delay.timeout_seconds", "300"),
                    ("stickiness.type", "source_ip"),
                    // "For new UDP/TCP_UDP target groups the default is true.
                    // Otherwise, the default is false."
                    ("deregistration_delay.connection_termination.enabled", "false"),
                    // "The default is disabled if the target group type is IP address
                    // and the target group protocol is TCP or TLS. Otherwise, the
                    // default is enabled."
                    ("preserve_client_ip.enabled", "true"),
                ],
            ]),
        ),
        (
            "a UDP ip target group (Network, the other branch of two defaults)",
            vec![
                ("Protocol", "UDP"),
                ("Port", "53"),
                ("VpcId", "vpc-1"),
                ("TargetType", "ip"),
            ],
            join(&[
                all_and_cross_zone,
                network_only,
                &[
                    ("deregistration_delay.timeout_seconds", "300"),
                    ("stickiness.type", "source_ip"),
                    ("deregistration_delay.connection_termination.enabled", "true"),
                    ("preserve_client_ip.enabled", "true"),
                ],
            ]),
        ),
        (
            "a TCP ip target group (Network, client IP preservation off)",
            vec![
                ("Protocol", "TCP"),
                ("Port", "443"),
                ("VpcId", "vpc-1"),
                ("TargetType", "ip"),
            ],
            join(&[
                all_and_cross_zone,
                network_only,
                &[
                    ("deregistration_delay.timeout_seconds", "300"),
                    ("stickiness.type", "source_ip"),
                    ("deregistration_delay.connection_termination.enabled", "false"),
                    ("preserve_client_ip.enabled", "false"),
                ],
            ]),
        ),
        (
            "a GENEVE target group (Gateway)",
            vec![("Protocol", "GENEVE"), ("Port", "6081"), ("VpcId", "vpc-1")],
            join(&[
                gateway_only,
                &[
                    ("deregistration_delay.timeout_seconds", "300"),
                    ("stickiness.enabled", "false"),
                    ("stickiness.type", "source_ip_dest_ip"),
                ],
            ]),
        ),
        (
            "a lambda target group",
            vec![("TargetType", "lambda")],
            join(&[&[
                // The model documents exactly ONE attribute for a Lambda target, and
                // nothing the load balancer CONNECTS to a target with applies — there
                // is no connection to drain, no cookie to pin and no zone to fail over
                // (see `TargetGroupRecord::carries_connection_attributes`).
                ("lambda.multi_value_headers.enabled", "false"),
            ]]),
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, expected)) in cases.into_iter().enumerate() {
        let name = format!("attr-default-{index}");
        let arn = create_target_group(&router, &name, &fields).await;
        let reported = described_attributes(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: unexpected attribute set for {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// Attribute writes: what is stored, what is refused.
// ---------------------------------------------------------------------------

/// A write inside an attribute's documented domain is stored and read back verbatim; a
/// write outside it and a key the model does not document are each refused with
/// `ValidationError`, a key that belongs to another load balancer family with
/// `InvalidConfigurationRequest`, and each of them stores nothing.
///
/// The accepted rows sit ON each published bound; the refused ones one step past it, so
/// the deregistration-delay value the issue recorded is one cell of the table.
#[tokio::test]
async fn an_attribute_write_is_accepted_on_its_documented_bounds_and_refused_past_them() {
    // (key, value, accepted) — every bound from the attribute's own documentation.
    let rows: &[(&str, &str, bool)] = &[
        // "The range is 0-3600 seconds. The default value is 300 seconds."
        ("deregistration_delay.timeout_seconds", "0", true),
        ("deregistration_delay.timeout_seconds", "3600", true),
        ("deregistration_delay.timeout_seconds", "60", true),
        ("deregistration_delay.timeout_seconds", "-1", false),
        ("deregistration_delay.timeout_seconds", "3601", false),
        ("deregistration_delay.timeout_seconds", "60.5", false),
        // "The range is 30-900 seconds (15 minutes). The default is 0 seconds
        // (disabled)."
        ("slow_start.duration_seconds", "0", true),
        ("slow_start.duration_seconds", "30", true),
        ("slow_start.duration_seconds", "900", true),
        ("slow_start.duration_seconds", "29", false),
        ("slow_start.duration_seconds", "901", false),
        // "The range is 1 second to 1 week (604800 seconds)."
        ("stickiness.lb_cookie.duration_seconds", "1", true),
        ("stickiness.lb_cookie.duration_seconds", "604800", true),
        ("stickiness.lb_cookie.duration_seconds", "0", false),
        ("stickiness.lb_cookie.duration_seconds", "604801", false),
        // "The value is true or false."
        ("stickiness.enabled", "true", true),
        ("stickiness.enabled", "false", true),
        ("stickiness.enabled", "yes", false),
        // "The value is round_robin, least_outstanding_requests, or weighted_random."
        ("load_balancing.algorithm.type", "round_robin", true),
        (
            "load_balancing.algorithm.type",
            "least_outstanding_requests",
            true,
        ),
        ("load_balancing.algorithm.type", "weighted_random", true),
        ("load_balancing.algorithm.type", "random", false),
        // "The possible values are off or an integer from 1 to 100."
        (
            "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            "off",
            true,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            "1",
            true,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            "100",
            true,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            "0",
            false,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.percentage",
            "101",
            false,
        ),
        // "The possible values are off or an integer from 1 to the maximum number of
        // targets", so the ceiling is the number of targets a target group holds at all
        // (`crate::state::TARGETS_PER_TARGET_GROUP`, the figure `DescribeAccountLimits`
        // reports as `targets-per-application-load-balancer`).
        (
            "target_group_health.dns_failover.minimum_healthy_targets.count",
            "off",
            true,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.count",
            "1",
            true,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.count",
            "1000",
            true,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.count",
            "0",
            false,
        ),
        (
            "target_group_health.dns_failover.minimum_healthy_targets.count",
            "1001",
            false,
        ),
        // "The value is an integer from 1 to the maximum number of targets" — the same
        // ceiling, and this one takes no `off`.
        (
            "target_group_health.unhealthy_state_routing.minimum_healthy_targets.count",
            "1",
            true,
        ),
        (
            "target_group_health.unhealthy_state_routing.minimum_healthy_targets.count",
            "1000",
            true,
        ),
        (
            "target_group_health.unhealthy_state_routing.minimum_healthy_targets.count",
            "1001",
            false,
        ),
        // "Names that start with the following prefixes are not allowed: AWSALB,
        // AWSALBAPP, and AWSALBTG."
        ("stickiness.app_cookie.cookie_name", "session", true),
        ("stickiness.app_cookie.cookie_name", "AWSALBAPP-1", false),
        // A key the model documents nowhere.
        (UNDOCUMENTED_ATTRIBUTE_KEY, "1", false),
        // A key documented "only by Network Load Balancers", on an Application Load
        // Balancer's target group.
        ("proxy_protocol_v2.enabled", "true", false),
        // A key documented only for a Lambda target, on an instance target group.
        ("lambda.multi_value_headers.enabled", "true", false),
    ];

    let (router, _engine) = emulator();

    for (index, (key, value, accepted)) in rows.iter().enumerate() {
        let case = format!("{key}={value}");
        // A fresh target group per row, so an accepted write never decides the
        // outcome of the next row.
        let name = format!("attr-write-{index}");
        let arn = create_target_group(
            &router,
            &name,
            &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        )
        .await;
        let before = described_attributes(&router, &arn).await;

        let reply = modify_attribute(&router, &arn, key, value).await;
        let after = described_attributes(&router, &arn).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a value on its documented bound must be stored, got {} body {}",
                reply.status, reply.body
            );
            // AWS answers with the group's whole collection, which is the set a read
            // reports: the modified attribute is in it, at the value just stored.
            assert_eq!(
                attribute_pairs(&reply.body),
                after,
                "case {case:?}: expected the whole collection answered, got {}",
                reply.body
            );
            assert_eq!(
                after.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected the stored value read back, got {after:?}"
            );
            // Nothing else moved.
            let mut expected = before.clone();
            expected.insert((*key).to_string(), (*value).to_string());
            assert_eq!(
                after, expected,
                "case {case:?}: a write must touch only the attribute it names"
            );
        } else {
            let code = refusal_code(key);
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, code),
                "case {case:?}: expected 400 {code}, got {} body {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused write must store nothing"
            );
        }
    }
}

/// One call writing several attributes writes all of them, and one carrying a single bad
/// attribute beside good ones writes NONE: the whole request is checked before the first
/// write.
#[tokio::test]
async fn an_attribute_write_is_all_or_nothing_across_the_request() {
    let (router, _engine) = emulator();
    let arn = create_http_target_group(&router, "attr-batch").await;
    let before = described_attributes(&router, &arn).await;

    let mut form = action("ModifyTargetGroupAttributes");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("Attributes.member.1.Key", "stickiness.enabled"));
    form.push(p("Attributes.member.1.Value", "true"));
    form.push(p("Attributes.member.2.Key", "slow_start.duration_seconds"));
    form.push(p("Attributes.member.2.Value", "120"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case two good attributes: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let stored = described_attributes(&router, &arn).await;
    assert_eq!(
        (
            stored.get("stickiness.enabled").map(String::as_str),
            stored.get("slow_start.duration_seconds").map(String::as_str)
        ),
        (Some("true"), Some("120")),
        "case two good attributes: expected both stored, got {stored:?}"
    );

    let mut form = action("ModifyTargetGroupAttributes");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("Attributes.member.1.Key", "stickiness.enabled"));
    form.push(p("Attributes.member.1.Value", "false"));
    form.push(p("Attributes.member.2.Key", "slow_start.duration_seconds"));
    form.push(p("Attributes.member.2.Value", "901"));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, VALIDATION_ERROR),
        "case one bad attribute beside a good one: expected the refusal, got {} body {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        described_attributes(&router, &arn).await,
        stored,
        "case one bad attribute beside a good one: expected nothing written"
    );
    assert_ne!(
        before, stored,
        "case setup: the first write must have changed something to make this meaningful"
    );
}

// ---------------------------------------------------------------------------
// ModifyTargetGroup.
// ---------------------------------------------------------------------------

/// `ModifyTargetGroup` replaces exactly the health-check members the request names, and
/// a later read reports the replacement beside every member that was left alone.
///
/// One row per member the operation declares
/// (`smithy_query(query="operation",operation="ModifyTargetGroup")`), each written on a
/// value inside its modelled range.
#[tokio::test]
async fn modify_target_group_replaces_only_the_health_check_members_it_names() {
    // (member, value) — every member `ModifyTargetGroup` declares beside the ARN.
    let members_to_write: &[(&str, &str)] = &[
        ("HealthCheckProtocol", "HTTPS"),
        ("HealthCheckPort", "8443"),
        ("HealthCheckPath", "/healthz"),
        ("HealthCheckIntervalSeconds", "17"),
        ("HealthCheckTimeoutSeconds", "11"),
        ("HealthyThresholdCount", "3"),
        ("UnhealthyThresholdCount", "4"),
        ("Matcher.HttpCode", "200-299"),
    ];

    let (router, _engine) = emulator();

    for (index, (member, value)) in members_to_write.iter().enumerate() {
        let name = format!("modify-tg-{index}");
        let arn = create_target_group(
            &router,
            &name,
            &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        )
        .await;
        // What the group reported before the write, member by member.
        let before = read_target_group(&router, &arn).await;

        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p(member, value));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {member}={value}: expected 200, got {} body {}",
            reply.status, reply.body
        );

        let after = read_target_group(&router, &arn).await;
        // The written member is the one the element name ends with (`Matcher.HttpCode`
        // is carried inside `<Matcher>`).
        let element = member.rsplit('.').next().expect("a member names an element");
        assert_eq!(
            first_text(&after, element).as_deref(),
            Some(*value),
            "case {member}={value}: expected the stored value read back, got {after}"
        );
        for (other, _) in members_to_write.iter().filter(|(other, _)| other != member) {
            let other_element = other.rsplit('.').next().expect("a member names an element");
            assert_eq!(
                first_text(&after, other_element),
                first_text(&before, other_element),
                "case {member}={value}: {other} must be left as it was, got {after}"
            );
        }
    }
}

/// A health check that cannot hold together is refused, and the stored health check is
/// left as it was: a `HealthCheckProtocol` value the member's documentation does not admit
/// — on its own or against the target group's protocol — is an input the request has to fix
/// (`ValidationError`), while a member that only an HTTP check carries, and health checks
/// turned off on a target type that always has them, are configurations the target group
/// cannot hold (the operation's own `InvalidConfigurationRequest`).
///
/// Each row is a sentence of the model's own documentation: the protocols a health check
/// may use, the one pairing of target group and health check protocol it rules out, the
/// members that exist only for an HTTP/HTTPS check, and the target types whose health
/// checks cannot be disabled.
#[tokio::test]
async fn a_health_check_that_cannot_hold_together_is_refused() {
    let (router, _engine) = emulator();
    let http_tg = create_http_target_group(&router, "modify-refuse-http").await;
    let tcp_tg = create_target_group(
        &router,
        "modify-refuse-tcp",
        &[("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")],
    )
    .await;
    let lambda_tg = create_target_group(
        &router,
        "modify-refuse-lambda",
        &[("TargetType", "lambda")],
    )
    .await;

    // (case, target group, fields, expected error code — empty for an accepted row)
    let cases: Vec<HealthCheckCase<'_>> = vec![
        (
            // "The GENEVE, TLS, UDP, TCP_UDP, QUIC, and TCP_QUIC protocols are not
            // supported for health checks."
            "a health check over UDP",
            &tcp_tg,
            vec![("HealthCheckProtocol", "UDP")],
            VALIDATION_ERROR,
        ),
        (
            "a health check over GENEVE",
            &tcp_tg,
            vec![("HealthCheckProtocol", "GENEVE")],
            VALIDATION_ERROR,
        ),
        (
            // "The TCP protocol is not supported for health checks if the protocol of
            // the target group is HTTP or HTTPS."
            "a TCP health check of an HTTP target group",
            &http_tg,
            vec![("HealthCheckProtocol", "TCP")],
            VALIDATION_ERROR,
        ),
        (
            // The same protocol IS supported for a TCP target group.
            "a TCP health check of a TCP target group",
            &tcp_tg,
            vec![("HealthCheckProtocol", "TCP")],
            "",
        ),
        (
            // "[HTTP/HTTPS health checks] The destination for health checks on the
            // targets."
            "a path on a TCP health check",
            &tcp_tg,
            vec![("HealthCheckPath", "/up")],
            "InvalidConfigurationRequest",
        ),
        (
            "a matcher on a TCP health check",
            &tcp_tg,
            vec![("Matcher.HttpCode", "200")],
            "InvalidConfigurationRequest",
        ),
        (
            // "If the target type is instance, ip, or alb, health checks are always
            // enabled and can't be disabled."
            "health checks disabled on an instance target group",
            &http_tg,
            vec![("HealthCheckEnabled", "false")],
            "InvalidConfigurationRequest",
        ),
        (
            // "If the target type is lambda, health checks are disabled by default but
            // can be enabled."
            "health checks enabled on a lambda target group",
            &lambda_tg,
            vec![("HealthCheckEnabled", "true")],
            "",
        ),
    ];

    for (case, arn, fields, code) in cases {
        let before = read_target_group(&router, arn).await;
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", arn));
        for (key, value) in &fields {
            form.push(p(key, value));
        }
        let reply = call(&router, &form).await;
        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, code),
                "case {case:?}: expected 400 {code}, got {} body {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                read_target_group(&router, arn).await,
                before,
                "case {case:?}: a refused modify must change nothing"
            );
        }
    }
}

/// Turning a health check from TCP into an HTTP one carries the members that exist only
/// for an HTTP check — named by the same request, because a modify fills no default for
/// either — and turning it back drops them, so a read never reports a path or a matcher
/// for a check that has neither.
#[tokio::test]
async fn the_http_only_health_check_members_follow_the_health_check_protocol() {
    let (router, _engine) = emulator();
    let arn = create_target_group(
        &router,
        "modify-tg-protocol",
        &[("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")],
    )
    .await;

    let tcp = read_target_group(&router, &arn).await;
    assert_eq!(
        (first_text(&tcp, "HealthCheckPath"), first_text(&tcp, "Matcher")),
        (None, None),
        "case a TCP health check: expected no path and no matcher, got {tcp}"
    );

    // The same move with neither member named would leave a layer-7 check with no
    // destination and no success criterion, so it is refused and stores nothing.
    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("HealthCheckProtocol", "HTTP"));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ValidationError"),
        "case becoming an HTTP health check with neither member: expected 400 \
         ValidationError, got {} body {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        read_target_group(&router, &arn).await,
        tcp,
        "case becoming an HTTP health check with neither member: expected nothing stored"
    );

    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("HealthCheckProtocol", "HTTP"));
    form.push(p("HealthCheckPath", "/up"));
    form.push(p("Matcher.HttpCode", "200-399"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case becoming an HTTP health check: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let http = read_target_group(&router, &arn).await;
    assert_eq!(
        (
            first_text(&http, "HealthCheckPath").as_deref(),
            first_text(&http, "HttpCode").as_deref()
        ),
        (Some("/up"), Some("200-399")),
        "case becoming an HTTP health check: expected the members the request named, \
         got {http}"
    );

    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("HealthCheckProtocol", "TCP"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case becoming a TCP health check again: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let back = read_target_group(&router, &arn).await;
    assert_eq!(
        (
            first_text(&back, "HealthCheckPath"),
            first_text(&back, "Matcher")
        ),
        (None, None),
        "case becoming a TCP health check again: expected both dropped, got {back}"
    );
}

/// The XML of the one target group `DescribeTargetGroups` reports for `arn`.
async fn read_target_group(router: &MultiServiceRouter, arn: &str) -> String {
    let mut form = action("DescribeTargetGroups");
    form.push(p("TargetGroupArns.member.1", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case read {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let found = members(&reply.body, "TargetGroups");
    assert_eq!(
        found.len(),
        1,
        "case read {arn}: expected exactly one target group, got {}",
        reply.body
    );
    found.into_iter().next().expect("one target group")
}

// ---------------------------------------------------------------------------
// Deregistration.
// ---------------------------------------------------------------------------

/// Deregistering removes exactly the registrations the request names — matched on the
/// target id AND its port, as the operation documents ("if you specified a port override
/// when you registered a target, you must specify both the target ID and the port when
/// you deregister it") — and a target the group does not hold is a success that removes
/// nothing.
#[tokio::test]
async fn deregistering_removes_the_named_registrations_and_is_idempotent() {
    let (router, _engine) = emulator();
    let arn = create_http_target_group(&router, "dereg-tg").await;

    // Three registrations: one on the group's port, and the same id plus another id on
    // overridden ports — "you can register each EC2 instance or IP address with the same
    // target group multiple times using different ports", so one request may ask for two
    // registrations of one id.
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("Targets.member.1.Id", "i-11111111111111111"));
    form.push(p("Targets.member.2.Id", "i-11111111111111111"));
    form.push(p("Targets.member.2.Port", "8080"));
    form.push(p("Targets.member.3.Id", "i-22222222222222222"));
    form.push(p("Targets.member.3.Port", "9090"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case register three: expected 200, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        registered_ports(&router, &arn).await,
        vec![
            ("i-11111111111111111".to_string(), "80".to_string()),
            ("i-11111111111111111".to_string(), "8080".to_string()),
            ("i-22222222222222222".to_string(), "9090".to_string()),
        ],
        "case register three: expected all three registrations"
    );

    // (case, the fields naming what to deregister, what must remain)
    let cases: Vec<DeregisterCase> = vec![
        (
            "a target the group does not hold at all",
            vec![p("Targets.member.1.Id", "i-99999999999999999")],
            vec![
                ("i-11111111111111111".to_string(), "80".to_string()),
                ("i-11111111111111111".to_string(), "8080".to_string()),
                ("i-22222222222222222".to_string(), "9090".to_string()),
            ],
        ),
        (
            "a registered id on a port it was never registered on",
            vec![
                p("Targets.member.1.Id", "i-22222222222222222"),
                p("Targets.member.1.Port", "7070"),
            ],
            vec![
                ("i-11111111111111111".to_string(), "80".to_string()),
                ("i-11111111111111111".to_string(), "8080".to_string()),
                ("i-22222222222222222".to_string(), "9090".to_string()),
            ],
        ),
        (
            "the registration on the group's own port, named without a port",
            vec![p("Targets.member.1.Id", "i-11111111111111111")],
            vec![
                ("i-11111111111111111".to_string(), "8080".to_string()),
                ("i-22222222222222222".to_string(), "9090".to_string()),
            ],
        ),
        (
            "two registrations at once, each named with its port",
            vec![
                p("Targets.member.1.Id", "i-11111111111111111"),
                p("Targets.member.1.Port", "8080"),
                p("Targets.member.2.Id", "i-22222222222222222"),
                p("Targets.member.2.Port", "9090"),
            ],
            Vec::new(),
        ),
        (
            "the same call again, with nothing left to remove",
            vec![
                p("Targets.member.1.Id", "i-11111111111111111"),
                p("Targets.member.1.Port", "8080"),
            ],
            Vec::new(),
        ),
    ];

    for (case, fields, remaining) in cases {
        let mut form = action("DeregisterTargets");
        form.push(p("TargetGroupArn", &arn));
        form.extend(fields);
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: deregistration is idempotent, so this must succeed; got {} body {}",
            reply.status, reply.body
        );
        assert_eq!(
            registered_ports(&router, &arn).await,
            remaining,
            "case {case:?}: unexpected registrations left"
        );
    }
}

/// Every registration `DescribeTargetHealth` reports for `arn`, as (id, health check
/// port) pairs in the order the service lists them.
async fn registered_ports(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case health of {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|member| {
            (
                first_text(member, "Id").unwrap_or_default(),
                first_text(member, "HealthCheckPort").unwrap_or_default(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The target group name.
// ---------------------------------------------------------------------------

/// A `Name` outside the rule `CreateTargetGroup`'s own documentation states is refused
/// with the awsQuery `ValidationError` code and a message naming the value, and nothing
/// is stored for it.
///
/// "This name must be unique per region per account, can have a maximum of 32
/// characters, must contain only alphanumeric characters or hyphens, and must not begin
/// or end with a hyphen." The rows sit on the published length and one past it, and on
/// each of the other clauses.
#[tokio::test]
async fn a_target_group_name_follows_the_documented_rule() {
    let thirty_two = "a".repeat(32);
    let thirty_three = "a".repeat(33);

    // (case, name, accepted)
    let cases: Vec<(&str, String, bool)> = vec![
        ("one character", "a".to_string(), true),
        ("the published maximum, 32 characters", thirty_two.clone(), true),
        ("alphanumeric with hyphens inside", "tg-1-ok".to_string(), true),
        ("one past the published maximum", thirty_three.clone(), false),
        ("an underscore", "tg_1".to_string(), false),
        ("a dot", "tg.1".to_string(), false),
        ("a leading hyphen", "-tg1".to_string(), false),
        ("a trailing hyphen", "tg1-".to_string(), false),
    ];

    let (router, _engine) = emulator();

    for (case, name, accepted) in cases {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", &name));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", "80"));
        form.push(p("VpcId", "vpc-1"));
        let reply = call(&router, &form).await;

        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: {name:?} satisfies the documented rule, got {} body {}",
                reply.status, reply.body
            );
            continue;
        }

        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected 400 ValidationError for {name:?}, got {} body {}",
            reply.status,
            reply.body
        );
        let message = unescaped(&reply.text("Message").unwrap_or_default());
        assert!(
            message.contains(&name),
            "case {case:?}: the refusal must name the value it rejected, got {message:?}"
        );
        // A refused create stores nothing: a read by that name reports no target group —
        // either because nothing holds the name (`TargetGroupNotFound`) or because the
        // read refuses the name on its own form, exactly as the create just did.
        let mut form = action("DescribeTargetGroups");
        form.push(p("Names.member.1", &name));
        let described = call(&router, &form).await;
        assert_eq!(
            described.status, 400,
            "case {case:?}: a refused create must store nothing, got {} body {}",
            described.status, described.body
        );
        assert!(
            !described.body.contains("<TargetGroups"),
            "case {case:?}: a refused create must store nothing, got {}",
            described.body
        );
    }

    // The length limit is the message's own: it states the value and the limit.
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", &thirty_three));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-1"));
    let reply = call(&router, &form).await;
    let message = unescaped(&reply.text("Message").unwrap_or_default());
    assert!(
        message.contains("32"),
        "case the length refusal: expected the limit stated, got {message:?}"
    );
}
