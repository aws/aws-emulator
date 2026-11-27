// SPDX-License-Identifier: CC0-1.0

//! What a reported FORWARD ACTION and a reported LISTENER carry, driven over the wire
//! exactly as a client drives them.
//!
//! Two rules are asserted here, each over a SET of inputs rather than over the one call a
//! recording happens to carry:
//!
//! * A forward action's distribution has TWO SPELLINGS — `Action$TargetGroupArn` for a
//!   single target group and `ForwardActionConfig$TargetGroups` for any number of them —
//!   so every reply that serializes such an action reports it under both, with each target
//!   group carried at the weight the request gave it or at the one the service distributes
//!   an unweighted group at, and with the stickiness the request configured or the one a
//!   forward is carried at until a request turns it on. A forward among SEVERAL groups has
//!   no single-group spelling, so that member stays absent for it.
//! * A listener's `MutualAuthentication` belongs to the listeners the model brackets the
//!   member to ("[HTTPS and TLS listeners] The mutual authentication configuration
//!   information"), so a read of such a listener reports the client certificate handling it
//!   is effectively under — the documented default mode for a listener nothing configured
//!   mTLS on — and a listener outside that bracket reports no such block at all.
//!
//! The rows differ along what those rules generalise over: which spelling the request used
//! (one, the other, or both), how many target groups a forward distributes among, whether
//! each carries a weight and which weight (both published bounds of the documented range
//! included), whether stickiness is configured and at which duration, the listener
//! protocol, the mutual authentication mode, and which operation reports the action (the
//! create, the modification, a priority move, a read by ARN and a read of the whole
//! listener — on a listener's default actions, on a rule a caller added, and on the rule
//! the listener mints with itself).

use crate::listener_rules::{
    create_load_balancer, forward_to, host_header, Form,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use crate::rule_deletion_and_condition_value_spellings::describe_rule;
use smithy_server_runtime::MultiServiceRouter;

/// The weight the service distributes a forward's target group at when the request named
/// none. `TargetGroupTuple$Weight`
/// (`smithy_query(query="shape",shape="TargetGroupTuple")`) is "[t]he weight. The range is
/// 0 to 999", and the weights of a forward's groups are relative to one another, so a
/// single group carried at the default takes every request the action matches.
const DEFAULT_WEIGHT: i32 = 1;

/// The two published bounds of that documented weight range, which two rows below carry so
/// a weight the request named is reported as named rather than as the default. The model
/// types the member as a plain integer with no range trait, so a value outside the range is
/// stored as given rather than refused — there is no refusal of a single weight to assert
/// here. The lowest bound is carried BESIDE a positive weight, because the weights of a
/// forward are relative to one another: an action whose weights total zero selects no target
/// group at all and is refused wherever it is set, which
/// `all/weighted_forward_and_target_group_stickiness.rs` pins.
const LOWEST_WEIGHT: i32 = 0;
const HIGHEST_WEIGHT: i32 = 999;

/// The two published bounds of `TargetGroupStickinessConfig$DurationSeconds` — "[t]he time
/// period, in seconds, during which requests from a client should be routed to the same
/// target group. The range is 1-604800 seconds (7 days)" — which two rows below carry.
const LOWEST_DURATION: i32 = 1;
const HIGHEST_DURATION: i32 = 604_800;

/// The client certificate handling method a listener nothing configured mTLS on is under,
/// from `MutualAuthenticationAttributes$Mode`'s own documentation
/// (`smithy_query(query="shape",shape="MutualAuthenticationAttributes")`): "The client
/// certificate handling method. The default value is `off`."
const DEFAULT_MODE: &str = "off";

/// Every mode that member documents for the HTTPS listener of an Application Load Balancer:
/// "[Application Load Balancers] The possible values are `off`, `passthrough`, and
/// `verify`."
const MODES: &[&str] = &["off", "passthrough", "verify"];

/// And every mode it documents for the TLS listener of a Network Load Balancer: "[Network
/// Load Balancers] The possible values are `off` and `verify`."
const NETWORK_MODES: &[&str] = &["off", "verify"];

/// The listeners the member is bracketed to, from its own documentation on both writes that
/// set it (`CreateListenerInput$MutualAuthentication` and
/// `ModifyListenerInput$MutualAuthentication`): "[HTTPS and TLS listeners]".
const MUTUAL_AUTHENTICATION_PROTOCOLS: &[&str] = &["HTTPS", "TLS"];

/// The error code this service answers an input its own documented brackets refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery error code of `DuplicateListenerException`, which `CreateListener` models
/// (`smithy_query(query="operation",operation="CreateListener")`: `awsQueryError` code
/// `DuplicateListener`).
const DUPLICATE_LISTENER: &str = "DuplicateListener";

/// A certificate ARN an HTTPS listener's default certificate resolves to, so a row that
/// needs an HTTPS listener reaches the mutual authentication assertion rather than the
/// certificate refusal pinned in `all/listener_tls_certificates.rs`.
const CERTIFICATE: &str = "arn:aws:acm:us-west-2:000000000000:certificate/mtls-listener";

// ---------------------------------------------------------------------------
// The forward table.
// ---------------------------------------------------------------------------

/// One row of the forward table: how the request SPELLS the forward action, and what every
/// reply reporting that action must then carry.
///
/// Target groups are named by their index into the suite's own target groups, so one table
/// serves a listener's default actions and a rule's actions alike.
struct ForwardRow {
    case: &'static str,
    /// The target group the request names in `Action$TargetGroupArn`, if any.
    shorthand: Option<usize>,
    /// The `(target group, Weight)` of each `ForwardConfig$TargetGroups` member the request
    /// carries, in order; empty when the request configures no target groups at all.
    configured: &'static [(usize, Option<i32>)],
    /// The `(Enabled, DurationSeconds)` of the `TargetGroupStickinessConfig` the request
    /// carries, if any.
    stickiness: Option<(bool, Option<i32>)>,
    /// The `(target group, Weight)` every reported `ForwardConfig$TargetGroups` member must
    /// carry, in order.
    reports: &'static [(usize, i32)],
    /// The target group a reported action must name in `Action$TargetGroupArn`, or `None`
    /// for a forward that distributes among several and so has no such spelling.
    reports_shorthand: Option<usize>,
    /// The `Enabled` the reported `TargetGroupStickinessConfig` must carry.
    reports_enabled: bool,
    /// The `DurationSeconds` it must carry, or `None` when the request configured none.
    reports_duration: Option<i32>,
}

/// The table every assertion below is made over.
const FORWARD_ROWS: &[ForwardRow] = &[
    ForwardRow {
        case: "a forward naming only its target group",
        shorthand: Some(0),
        configured: &[],
        stickiness: None,
        reports: &[(0, DEFAULT_WEIGHT)],
        reports_shorthand: Some(0),
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward configuring one target group with no weight",
        shorthand: None,
        configured: &[(1, None)],
        stickiness: None,
        reports: &[(1, DEFAULT_WEIGHT)],
        reports_shorthand: Some(1),
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward configuring the lowest published weight beside the default one",
        shorthand: None,
        configured: &[(2, Some(LOWEST_WEIGHT)), (1, Some(DEFAULT_WEIGHT))],
        stickiness: None,
        reports: &[(2, LOWEST_WEIGHT), (1, DEFAULT_WEIGHT)],
        reports_shorthand: None,
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward configuring one target group at the highest published weight",
        shorthand: None,
        configured: &[(0, Some(HIGHEST_WEIGHT))],
        stickiness: None,
        reports: &[(0, HIGHEST_WEIGHT)],
        reports_shorthand: Some(0),
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward naming its one target group under both spellings",
        shorthand: Some(1),
        configured: &[(1, Some(DEFAULT_WEIGHT))],
        stickiness: None,
        reports: &[(1, DEFAULT_WEIGHT)],
        reports_shorthand: Some(1),
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward distributing between two weighted target groups",
        shorthand: None,
        configured: &[(0, Some(30)), (1, Some(70))],
        stickiness: None,
        reports: &[(0, 30), (1, 70)],
        reports_shorthand: None,
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward among three target groups, the middle one carrying no weight",
        shorthand: None,
        configured: &[(0, Some(5)), (1, None), (2, Some(HIGHEST_WEIGHT))],
        stickiness: None,
        reports: &[(0, 5), (1, DEFAULT_WEIGHT), (2, HIGHEST_WEIGHT)],
        reports_shorthand: None,
        reports_enabled: false,
        reports_duration: None,
    },
    ForwardRow {
        case: "a forward with stickiness enabled at the lowest published duration",
        shorthand: Some(2),
        configured: &[],
        stickiness: Some((true, Some(LOWEST_DURATION))),
        reports: &[(2, DEFAULT_WEIGHT)],
        reports_shorthand: Some(2),
        reports_enabled: true,
        reports_duration: Some(LOWEST_DURATION),
    },
    ForwardRow {
        case: "a forward with stickiness enabled at the highest published duration",
        shorthand: None,
        configured: &[(0, Some(10)), (2, Some(20))],
        stickiness: Some((true, Some(HIGHEST_DURATION))),
        reports: &[(0, 10), (2, 20)],
        reports_shorthand: None,
        reports_enabled: true,
        reports_duration: Some(HIGHEST_DURATION),
    },
    ForwardRow {
        case: "a forward with stickiness configured off",
        shorthand: Some(0),
        configured: &[],
        stickiness: Some((false, None)),
        reports: &[(0, DEFAULT_WEIGHT)],
        reports_shorthand: Some(0),
        reports_enabled: false,
        reports_duration: None,
    },
];

/// The request fields of one row's forward action, under `prefix` — `DefaultActions.member.1.`
/// for a listener's default action and `Actions.member.1.` for a rule's.
fn forward_fields(row: &ForwardRow, prefix: &str, target_groups: &[String]) -> Form {
    let mut form = vec![p(&format!("{prefix}Type"), "forward")];
    if let Some(index) = row.shorthand {
        form.push(p(&format!("{prefix}TargetGroupArn"), &target_groups[index]));
    }
    for (member, (index, weight)) in row.configured.iter().enumerate() {
        let at = member + 1;
        form.push(p(
            &format!("{prefix}ForwardConfig.TargetGroups.member.{at}.TargetGroupArn"),
            &target_groups[*index],
        ));
        if let Some(weight) = weight {
            form.push(p(
                &format!("{prefix}ForwardConfig.TargetGroups.member.{at}.Weight"),
                &weight.to_string(),
            ));
        }
    }
    if let Some((enabled, duration)) = row.stickiness {
        form.push(p(
            &format!("{prefix}ForwardConfig.TargetGroupStickinessConfig.Enabled"),
            if enabled { "true" } else { "false" },
        ));
        if let Some(duration) = duration {
            form.push(p(
                &format!("{prefix}ForwardConfig.TargetGroupStickinessConfig.DurationSeconds"),
                &duration.to_string(),
            ));
        }
    }
    form
}

/// One reported action with its `ForwardConfig` element removed, which is what lets
/// `Action$TargetGroupArn` be read apart from the ARN each configured target group carries.
fn without_forward_config(action: &str) -> String {
    const CLOSE: &str = "</ForwardConfig>";
    match (action.find("<ForwardConfig>"), action.find(CLOSE)) {
        (Some(start), Some(end)) => {
            let mut outside = action[..start].to_string();
            outside.push_str(&action[end + CLOSE.len()..]);
            outside
        }
        _ => action.to_string(),
    }
}

/// The `(TargetGroupArn, Weight)` of every `ForwardConfig$TargetGroups` member of one
/// reported action, in the order it reported them.
fn reported_groups(action: &str) -> Vec<(String, String)> {
    members(action, "TargetGroups")
        .iter()
        .map(|tuple| {
            (
                first_text(tuple, "TargetGroupArn").unwrap_or_default(),
                first_text(tuple, "Weight").unwrap_or_default(),
            )
        })
        .collect()
}

/// Assert what one reported forward action carries, whichever operation reported it.
fn assert_forward(case: &str, where_from: &str, action: &str, row: &ForwardRow, tgs: &[String]) {
    assert!(
        action.contains("<ForwardConfig>"),
        "case {case}: {where_from} must report the ForwardConfig of a forward action, got {action}"
    );
    let expected: Vec<(String, String)> = row
        .reports
        .iter()
        .map(|(index, weight)| (tgs[*index].clone(), weight.to_string()))
        .collect();
    assert_eq!(
        reported_groups(action),
        expected,
        "case {case}: {where_from} expected the target groups {expected:?}, got {action}"
    );
    let expected_shorthand = row.reports_shorthand.map(|index| tgs[index].clone());
    assert_eq!(
        first_text(&without_forward_config(action), "TargetGroupArn"),
        expected_shorthand,
        "case {case}: {where_from} expected Action.TargetGroupArn {expected_shorthand:?}, \
         got {action}"
    );
    assert_eq!(
        first_text(action, "Enabled").as_deref(),
        Some(if row.reports_enabled { "true" } else { "false" }),
        "case {case}: {where_from} expected stickiness enabled {}, got {action}",
        row.reports_enabled
    );
    assert_eq!(
        first_text(action, "DurationSeconds"),
        row.reports_duration.map(|seconds| seconds.to_string()),
        "case {case}: {where_from} expected stickiness duration {:?}, got {action}",
        row.reports_duration
    );
}

/// The one forward action of a reported listener's default actions.
fn only_default_action(case: &str, where_from: &str, body: &str) -> String {
    let listeners = members(body, "Listeners");
    assert_eq!(
        listeners.len(),
        1,
        "case {case}: {where_from} must report the one listener, got {body}"
    );
    let actions = members(&listeners[0], "DefaultActions");
    assert_eq!(
        actions.len(),
        1,
        "case {case}: {where_from} must report the one default action, got {body}"
    );
    actions[0].clone()
}

/// The one forward action of a reported rule.
fn only_rule_action(case: &str, where_from: &str, rule: &str) -> String {
    let actions = members(rule, "Actions");
    assert_eq!(
        actions.len(),
        1,
        "case {case}: {where_from} must report the one action of the rule, got {rule}"
    );
    actions[0].clone()
}

/// Read one listener back.
///
/// `pub(crate)` because `all/listener_target_group_protocol_pairing.rs` reads a listener the
/// same way: ONE reader for these two suites, so they cannot drift on how a listener is
/// read back.
pub(crate) async fn describe_listener(router: &MultiServiceRouter, listener_arn: &str) -> Reply {
    let mut form = action("DescribeListeners");
    form.push(p("ListenerArns.member.1", listener_arn));
    call(router, &form).await
}

/// The three target groups the forward rows distribute among.
async fn forward_target_groups(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let mut groups = Vec::new();
    for index in 1..=3 {
        groups.push(create_http_target_group(router, &format!("{prefix}-tg-{index}")).await);
    }
    groups
}

// ---------------------------------------------------------------------------
// A listener's DEFAULT actions.
// ---------------------------------------------------------------------------

/// Every reply that serializes a LISTENER's default forward action reports its normalized
/// `ForwardConfig`: the create's own answer, a read of the listener, the modification that
/// put the action there, and the default rule `DescribeRules` answers with.
#[tokio::test]
async fn a_listener_default_forward_action_reports_its_normalized_forward_config() {
    let (router, _engine) = emulator();
    let target_groups = forward_target_groups(&router, "lb-forward").await;
    let lb = create_load_balancer(&router, "lb-forward").await;

    for (index, row) in FORWARD_ROWS.iter().enumerate() {
        let port = 10_000 + index as i32;
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", "HTTP"));
        form.push(p("Port", &port.to_string()));
        form.extend(forward_fields(row, "DefaultActions.member.1.", &target_groups));
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200 from CreateListener, got {} {}",
            row.case, created.status, created.body
        );
        assert_forward(
            row.case,
            "CreateListener",
            &only_default_action(row.case, "CreateListener", &created.body),
            row,
            &target_groups,
        );
        let listener = first_text(&created.body, "ListenerArn")
            .unwrap_or_else(|| panic!("case {}: no ListenerArn in {}", row.case, created.body));

        // What the write answered is what the store holds: the read is the assertion the
        // write's answer is only checked to agree with.
        let read = describe_listener(&router, &listener).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected 200 from DescribeListeners, got {} {}",
            row.case, read.status, read.body
        );
        assert_forward(
            row.case,
            "DescribeListeners",
            &only_default_action(row.case, "DescribeListeners", &read.body),
            row,
            &target_groups,
        );

        // The rule the listener mints with itself carries the listener's default actions,
        // and reports them through the same normalisation.
        let mut form = action("DescribeRules");
        form.push(p("ListenerArn", &listener));
        let rules = call(&router, &form).await;
        let default_rule = members(&rules.body, "Rules")
            .into_iter()
            .find(|rule| first_text(rule, "IsDefault").as_deref() == Some("true"))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: every listener reports a default rule, got {}",
                    row.case, rules.body
                )
            });
        assert_forward(
            row.case,
            "the default rule of DescribeRules",
            &only_rule_action(row.case, "the default rule", &default_rule),
            row,
            &target_groups,
        );
    }

    // A MODIFICATION reports the listener it leaves behind the same way: each row replaces
    // the default actions of a listener created with the plainest forward there is.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &target_groups[0]));
    let created = call(&router, &form).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the modified listener: no ARN in {}", created.body));
    for row in FORWARD_ROWS {
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &listener));
        form.extend(forward_fields(row, "DefaultActions.member.1.", &target_groups));
        let modified = call(&router, &form).await;
        assert_eq!(
            modified.status, 200,
            "case {}: expected 200 from ModifyListener, got {} {}",
            row.case, modified.status, modified.body
        );
        assert_forward(
            row.case,
            "ModifyListener",
            &only_default_action(row.case, "ModifyListener", &modified.body),
            row,
            &target_groups,
        );
        let read = describe_listener(&router, &listener).await;
        assert_forward(
            row.case,
            "DescribeListeners after the modification",
            &only_default_action(row.case, "DescribeListeners", &read.body),
            row,
            &target_groups,
        );
    }
}

// ---------------------------------------------------------------------------
// A RULE's actions.
// ---------------------------------------------------------------------------

/// Every reply that serializes a RULE's forward action reports its normalized
/// `ForwardConfig`: the create, a read by ARN, a read of the whole listener, the
/// modification that put the action there and the priority move that reports the rule.
#[tokio::test]
async fn a_rule_forward_action_reports_its_normalized_forward_config() {
    let (router, _engine) = emulator();
    let target_groups = forward_target_groups(&router, "rule-forward").await;
    let lb = create_load_balancer(&router, "rule-forward").await;
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &target_groups[0]));
    let created = call(&router, &form).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the rule listener: no ARN in {}", created.body));

    let mut arns = Vec::new();
    for (index, row) in FORWARD_ROWS.iter().enumerate() {
        let priority = 1 + index as i32;
        let mut form = action("CreateRule");
        form.push(p("ListenerArn", &listener));
        form.push(p("Priority", &priority.to_string()));
        form.extend(host_header(&format!("row-{index}.example.com")));
        form.extend(forward_fields(row, "Actions.member.1.", &target_groups));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200 from CreateRule, got {} {}",
            row.case, reply.status, reply.body
        );
        let rules = members(&reply.body, "Rules");
        assert_eq!(
            rules.len(),
            1,
            "case {}: a create answers with the one rule it stored, got {}",
            row.case,
            reply.body
        );
        assert_forward(
            row.case,
            "CreateRule",
            &only_rule_action(row.case, "CreateRule", &rules[0]),
            row,
            &target_groups,
        );
        let arn = first_text(&rules[0], "RuleArn")
            .unwrap_or_else(|| panic!("case {}: no RuleArn in {}", row.case, reply.body));

        let read = describe_rule(&router, &arn).await;
        let stored = members(&read.body, "Rules");
        assert_eq!(
            stored.len(),
            1,
            "case {}: a read by ARN answers with the one rule it names, got {}",
            row.case,
            read.body
        );
        assert_forward(
            row.case,
            "DescribeRules by ARN",
            &only_rule_action(row.case, "DescribeRules by ARN", &stored[0]),
            row,
            &target_groups,
        );
        arns.push(arn);
    }

    // A read of the WHOLE listener reports every one of those rules the same way.
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener));
    let reply = call(&router, &form).await;
    for (row, arn) in FORWARD_ROWS.iter().zip(&arns) {
        let rule = members(&reply.body, "Rules")
            .into_iter()
            .find(|rule| first_text(rule, "RuleArn").as_deref() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the listener must report the rule it holds, got {}",
                    row.case, reply.body
                )
            });
        assert_forward(
            row.case,
            "DescribeRules by listener",
            &only_rule_action(row.case, "DescribeRules by listener", &rule),
            row,
            &target_groups,
        );
    }

    // A PRIORITY MOVE reports each rule it moved, actions and all: every rule above is
    // moved down by the count of them, in one call.
    let mut form = action("SetRulePriorities");
    for (index, arn) in arns.iter().enumerate() {
        let member = index + 1;
        form.push(p(&format!("RulePriorities.member.{member}.RuleArn"), arn));
        form.push(p(
            &format!("RulePriorities.member.{member}.Priority"),
            &(100 + index as i32).to_string(),
        ));
    }
    let moved = call(&router, &form).await;
    assert_eq!(
        moved.status, 200,
        "case the priority move: expected 200, got {} {}",
        moved.status, moved.body
    );
    for (row, arn) in FORWARD_ROWS.iter().zip(&arns) {
        let rule = members(&moved.body, "Rules")
            .into_iter()
            .find(|rule| first_text(rule, "RuleArn").as_deref() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: SetRulePriorities must report every rule it moved, got {}",
                    row.case, moved.body
                )
            });
        assert_forward(
            row.case,
            "SetRulePriorities",
            &only_rule_action(row.case, "SetRulePriorities", &rule),
            row,
            &target_groups,
        );
    }

    // A MODIFICATION reports the rule it leaves behind the same way: each row replaces the
    // actions of a rule created with the plainest forward there is.
    for (index, row) in FORWARD_ROWS.iter().enumerate() {
        let priority = 500 + index as i32;
        let mut form = action("CreateRule");
        form.push(p("ListenerArn", &listener));
        form.push(p("Priority", &priority.to_string()));
        form.extend(host_header(&format!("modified-{index}.example.com")));
        form.extend(forward_to(&target_groups[0]));
        let reply = call(&router, &form).await;
        let arn = first_text(&reply.body, "RuleArn")
            .unwrap_or_else(|| panic!("case {}: no RuleArn in {}", row.case, reply.body));

        let mut form = action("ModifyRule");
        form.push(p("RuleArn", &arn));
        form.extend(forward_fields(row, "Actions.member.1.", &target_groups));
        let modified = call(&router, &form).await;
        assert_eq!(
            modified.status, 200,
            "case {}: expected 200 from ModifyRule, got {} {}",
            row.case, modified.status, modified.body
        );
        let rules = members(&modified.body, "Rules");
        assert_eq!(
            rules.len(),
            1,
            "case {}: a modification answers with the one rule it changed, got {}",
            row.case,
            modified.body
        );
        assert_forward(
            row.case,
            "ModifyRule",
            &only_rule_action(row.case, "ModifyRule", &rules[0]),
            row,
            &target_groups,
        );
        let read = describe_rule(&router, &arn).await;
        let stored = members(&read.body, "Rules");
        assert_forward(
            row.case,
            "DescribeRules after the modification",
            &only_rule_action(row.case, "DescribeRules after the modification", &stored[0]),
            row,
            &target_groups,
        );
    }
}

// ---------------------------------------------------------------------------
// A listener's MUTUAL AUTHENTICATION.
// ---------------------------------------------------------------------------

/// One row of the listener table: the protocol the listener is created on, the mutual
/// authentication mode the request configures (if any), and the `Mode` a read of the
/// listener must then report — `None` when the listener reports no such block at all.
struct ListenerRow {
    case: &'static str,
    protocol: &'static str,
    configured: Option<&'static str>,
    reports: Option<&'static str>,
}

/// The `Mode` of the `MutualAuthentication` block of the one listener in a reply, and
/// whether that block is there at all.
fn reported_mode(body: &str) -> (bool, Option<String>) {
    let listeners = members(body, "Listeners");
    let listener = listeners.first().cloned().unwrap_or_default();
    (
        listener.contains("<MutualAuthentication>"),
        first_text(&listener, "Mode"),
    )
}

/// The table the mutual authentication assertions are made over: each protocol the model
/// gives the member a bracket for and one outside it that still terminates TLS, and each
/// documented mode.
fn listener_rows() -> Vec<ListenerRow> {
    let mut rows = vec![
        ListenerRow {
            case: "an HTTPS listener created without mTLS",
            protocol: "HTTPS",
            configured: None,
            reports: Some(DEFAULT_MODE),
        },
        ListenerRow {
            case: "an HTTP listener",
            protocol: "HTTP",
            configured: None,
            reports: None,
        },
        ListenerRow {
            case: "a TLS listener created without mTLS",
            protocol: "TLS",
            configured: None,
            reports: Some(DEFAULT_MODE),
        },
    ];
    for mode in MODES {
        rows.push(ListenerRow {
            case: "an HTTPS listener created with a mode of its own",
            protocol: "HTTPS",
            configured: Some(mode),
            reports: Some(mode),
        });
    }
    for mode in NETWORK_MODES {
        rows.push(ListenerRow {
            case: "a TLS listener created with a mode of its own",
            protocol: "TLS",
            configured: Some(mode),
            reports: Some(mode),
        });
    }
    rows
}

/// A read of a listener reports the mutual authentication it is effectively under: the
/// documented default mode for an HTTPS listener nothing configured mTLS on, the mode a
/// request configured, and no block at all for a listener the member is not bracketed to.
#[tokio::test]
async fn a_read_listener_reports_the_mutual_authentication_it_is_under() {
    let (router, _engine) = emulator();
    let target_group = create_http_target_group(&router, "mtls-tg").await;
    let lb = create_load_balancer(&router, "mtls-lb").await;

    for (index, row) in listener_rows().iter().enumerate() {
        let port = 20_000 + index as i32;
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", row.protocol));
        form.push(p("Port", &port.to_string()));
        if row.protocol == "HTTPS" || row.protocol == "TLS" {
            form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
        }
        if let Some(mode) = row.configured {
            form.push(p("MutualAuthentication.Mode", mode));
        }
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200 from CreateListener, got {} {}",
            row.case, created.status, created.body
        );
        // A write ECHOES the configuration the request carried: the block is there when the
        // request configured one for a listener it belongs to, and absent otherwise.
        let (echoed, echoed_mode) = reported_mode(&created.body);
        let expected_echo = row
            .configured
            .filter(|_| MUTUAL_AUTHENTICATION_PROTOCOLS.contains(&row.protocol))
            .map(str::to_string);
        assert_eq!(
            (echoed, echoed_mode.clone()),
            (expected_echo.is_some(), expected_echo.clone()),
            "case {}: CreateListener expected the echoed mode {expected_echo:?}, got {}",
            row.case,
            created.body
        );

        let listener = first_text(&created.body, "ListenerArn")
            .unwrap_or_else(|| panic!("case {}: no ListenerArn in {}", row.case, created.body));
        let read = describe_listener(&router, &listener).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected 200 from DescribeListeners, got {} {}",
            row.case, read.status, read.body
        );
        let (present, mode) = reported_mode(&read.body);
        assert_eq!(
            (present, mode.as_deref()),
            (row.reports.is_some(), row.reports),
            "case {}: DescribeListeners expected the mode {:?}, got {}",
            row.case,
            row.reports,
            read.body
        );
    }
}

/// A listener MOVED onto and off the protocol the member is bracketed to reports its
/// mutual authentication accordingly: the mode a modification configures, and no block at
/// all once the listener is no longer an HTTPS one.
#[tokio::test]
async fn a_modified_listener_reports_the_mutual_authentication_it_is_left_under() {
    let (router, _engine) = emulator();
    let target_group = create_http_target_group(&router, "mtls-modify-tg").await;
    let lb = create_load_balancer(&router, "mtls-modify-lb").await;

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTPS"));
    form.push(p("Port", "443"));
    form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
    let created = call(&router, &form).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the modified listener: no ARN in {}", created.body));

    // Each mode the member documents, set one after the other on the one listener: the
    // configuration a request carries replaces the stored one.
    for mode in MODES {
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &listener));
        form.push(p("MutualAuthentication.Mode", mode));
        let modified = call(&router, &form).await;
        assert_eq!(
            modified.status, 200,
            "case the mode {mode}: expected 200 from ModifyListener, got {} {}",
            modified.status, modified.body
        );
        assert_eq!(
            reported_mode(&modified.body).1.as_deref(),
            Some(*mode),
            "case the mode {mode}: ModifyListener must echo the mode it was given, got {}",
            modified.body
        );
        let read = describe_listener(&router, &listener).await;
        assert_eq!(
            reported_mode(&read.body).1.as_deref(),
            Some(*mode),
            "case the mode {mode}: DescribeListeners must report the stored mode, got {}",
            read.body
        );
    }

    // "Changing the protocol from HTTPS to HTTP … removes the security policy and default
    // certificate properties" — and with them the client certificate handling, which
    // belongs to an HTTPS listener alone.
    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", &listener));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    let moved = call(&router, &form).await;
    assert_eq!(
        moved.status, 200,
        "case moved off HTTPS: expected 200 from ModifyListener, got {} {}",
        moved.status, moved.body
    );
    assert_eq!(
        reported_mode(&moved.body),
        (false, None),
        "case moved off HTTPS: ModifyListener must report no mutual authentication, got {}",
        moved.body
    );
    let read = describe_listener(&router, &listener).await;
    assert_eq!(
        reported_mode(&read.body),
        (false, None),
        "case moved off HTTPS: DescribeListeners must report no mutual authentication, got {}",
        read.body
    );

    // And the configuration is GONE, not merely unreported: a listener moved back onto
    // HTTPS is under the documented default, not the mode it carried before the move.
    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", &listener));
    form.push(p("Protocol", "HTTPS"));
    form.push(p("Port", "443"));
    form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
    let back = call(&router, &form).await;
    assert_eq!(
        back.status, 200,
        "case moved back onto HTTPS: expected 200 from ModifyListener, got {} {}",
        back.status, back.body
    );
    let read = describe_listener(&router, &listener).await;
    assert_eq!(
        reported_mode(&read.body),
        (true, Some(DEFAULT_MODE.to_string())),
        "case moved back onto HTTPS: expected the documented default mode, got {}",
        read.body
    );
}

/// `MutualAuthentication` belongs to the listeners the model brackets it to, so configuring
/// it on a listener of any other protocol is REFUSED rather than accepted with the
/// configuration dropped — on the create and on the modification alike.
#[tokio::test]
async fn configuring_mutual_authentication_outside_its_bracket_is_refused() {
    let (router, _engine) = emulator();
    let target_group = create_http_target_group(&router, "mtls-bracket-tg").await;
    let lb = create_load_balancer(&router, "mtls-bracket-lb").await;

    // Every value `ProtocolEnum` declares outside the bracket, plus a listener naming no
    // protocol at all (`smithy_query(query="shape",shape="ProtocolEnum")`: GENEVE, HTTP,
    // HTTPS, QUIC, TCP, TCP_QUIC, TCP_UDP, TLS, UDP) — HTTPS and TLS, the two the member is
    // bracketed to, are asserted accepted above.
    let outside: &[Option<&str>] = &[
        Some("HTTP"),
        Some("TCP"),
        Some("TCP_UDP"),
        Some("UDP"),
        Some("QUIC"),
        Some("TCP_QUIC"),
        Some("GENEVE"),
        None,
    ];

    for (index, protocol) in outside.iter().enumerate() {
        let case = format!("configuring mTLS on a {protocol:?} listener");
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        if let Some(protocol) = protocol {
            form.push(p("Protocol", protocol));
            form.push(p("Port", &(21_000 + index as i32).to_string()));
        }
        form.push(p("MutualAuthentication.Mode", "verify"));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, VALIDATION_ERROR),
            "case {case}: expected 400 {VALIDATION_ERROR}, got {} {}",
            reply.status,
            reply.body
        );
        let message = first_text(&reply.body, "Message").unwrap_or_default();
        assert!(
            message.contains("MutualAuthentication"),
            "case {case}: the refusal must name the member it judged, got {message}"
        );
    }

    // A MODIFICATION is judged by the same bracket, against the protocol it LEAVES the
    // listener on: setting the mode while moving the listener off HTTPS is refused, and the
    // listener keeps the mode it had.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTPS"));
    form.push(p("Port", "443"));
    form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
    form.push(p("MutualAuthentication.Mode", "passthrough"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
    let created = call(&router, &form).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the HTTPS listener: no ARN in {}", created.body));

    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", &listener));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "8080"));
    form.push(p("MutualAuthentication.Mode", "verify"));
    let refused = call(&router, &form).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, VALIDATION_ERROR),
        "case modifying onto HTTP while configuring mTLS: expected 400 {VALIDATION_ERROR}, \
         got {} {}",
        refused.status,
        refused.body
    );
    let read = describe_listener(&router, &listener).await;
    assert_eq!(
        reported_mode(&read.body),
        (true, Some("passthrough".to_string())),
        "case modifying onto HTTP while configuring mTLS: the refused call must have changed \
         nothing, got {}",
        read.body
    );
}

/// The client certificate handling is one of the settings `CreateListener` is idempotent
/// over: a repeat on the same port differing only in it is the modelled duplicate, and one
/// naming the mode the service defaults is the idempotent repeat of a create that named no
/// mode at all.
#[tokio::test]
async fn the_mutual_authentication_mode_is_one_of_the_settings_a_create_repeat_is_judged_by() {
    let (router, _engine) = emulator();
    let target_group = create_http_target_group(&router, "mtls-repeat-tg").await;
    let lb = create_load_balancer(&router, "mtls-repeat-lb").await;

    // The create every row below repeats: an HTTPS listener nothing configured mTLS on.
    let listener_form = |mode: Option<&str>| {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", "HTTPS"));
        form.push(p("Port", "443"));
        form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
        if let Some(mode) = mode {
            form.push(p("MutualAuthentication.Mode", mode));
        }
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &target_group));
        form
    };

    let created = call(&router, &listener_form(None)).await;
    let listener = first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("case the first create: no ARN in {}", created.body));

    // `(the mode the repeat names, whether it is the same settings)`: the mode the service
    // defaults is what the first create left the listener under, so naming it is the
    // documented idempotent repeat; every other documented mode is a different setting.
    let rows: &[(Option<&str>, bool)] = &[
        (None, true),
        (Some(DEFAULT_MODE), true),
        (Some("passthrough"), false),
        (Some("verify"), false),
    ];
    for (mode, same) in rows {
        let repeat = call(&router, &listener_form(*mode)).await;
        if *same {
            assert_eq!(
                (repeat.status, first_text(&repeat.body, "ListenerArn")),
                (200, Some(listener.clone())),
                "case a repeat naming the mode {mode:?}: expected the stored listener, got {} {}",
                repeat.status,
                repeat.body
            );
        } else {
            assert_eq!(
                (repeat.status, repeat.error_code().as_str()),
                (400, DUPLICATE_LISTENER),
                "case a repeat naming the mode {mode:?}: expected 400 {DUPLICATE_LISTENER}, \
                 got {} {}",
                repeat.status,
                repeat.body
            );
        }
    }
}
