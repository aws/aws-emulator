// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ACTION each Traffic Mirror filter rule is reported with, driven
//! over the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rule under test: every operation that reports a Traffic Mirror filter projects each of
//! its rules from the ONE stored record, so a rule's `RuleAction` reads back as the action it
//! was created or last modified with — never a default, and never another rule's. The filter's
//! two rule sets are its own members (`TrafficMirrorFilter$IngressFilterRules` →
//! `ingressFilterRuleSet` and `$EgressFilterRules` → `egressFilterRuleSet`), and a rule's
//! action is `TrafficMirrorFilterRule$RuleAction` → `ruleAction`, targeting the modelled
//! `TrafficMirrorRuleAction` enum, so the readers covered here are every one that carries
//! those sets: `ModifyTrafficMirrorFilterNetworkServices` — which reports "the Traffic Mirror
//! filter that the network service is associated with" and must leave every rule's action
//! alone — and `DescribeTrafficMirrorFilters` beside it, plus the rule's own create and modify
//! answers.
//!
//! The rows differ along what the rule generalises over: which action the rule carries (every
//! member of the modelled enum), which direction's set holds it, whether the action was set by
//! the create or by a later modify, how many rules the filter holds, and whether the rules of
//! one filter carry the same action or different ones. The recorded unit this suite was filed
//! from — one ingress rule stored as `reject`, read back through
//! `ModifyTrafficMirrorFilterNetworkServices` — is ONE row of
//! [`every_reader_reports_the_action_each_rule_was_created_with`].

use ec2::state::{TRAFFIC_DIRECTION_EGRESS, TRAFFIC_DIRECTION_INGRESS};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. A Traffic Mirror filter's modelled shape names no
/// Region, so the Region only selects the envelope.
const REGION: &str = "us-west-2";

/// The actions a Traffic Mirror rule can carry, as the modelled `TrafficMirrorRuleAction` enum
/// declares them — the crate's published catalog
/// (`smithy_query(query="shape",shape="TrafficMirrorRuleAction")` is `accept | reject`), so the
/// table below covers every member the model carries rather than the one the recording used.
const RULE_ACTIONS: &[&str] = ec2::ops::types::TRAFFIC_MIRROR_RULE_ACTION_VALUES;

/// The directions a rule can filter in, as the modelled `TrafficDirection` enum declares them
/// (`crate::ops::types::TRAFFIC_DIRECTION_VALUES` is `egress | ingress`), so each row is
/// asserted on both of the filter's rule sets.
const TRAFFIC_DIRECTIONS: &[&str] = ec2::ops::types::TRAFFIC_DIRECTION_VALUES;

/// The network services a filter can be asked to mirror, as the modelled
/// `TrafficMirrorNetworkService` enum declares them: what the modification under test changes.
const NETWORK_SERVICES: &[&str] = ec2::ops::types::TRAFFIC_MIRROR_NETWORK_SERVICE_VALUES;

/// The destination block every rule here is created with.
const DESTINATION_CIDR: &str = "10.2.0.0/16";

/// The source block every rule here is created with.
const SOURCE_CIDR: &str = "10.1.0.0/16";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call of `action` that has to succeed, and the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The set member one direction's rules are reported under: the `xmlName` of the filter member
/// that direction selects (`TrafficMirrorFilter$IngressFilterRules` → `ingressFilterRuleSet`,
/// `$EgressFilterRules` → `egressFilterRuleSet`).
fn rule_set(traffic_direction: &str) -> &'static str {
    match traffic_direction {
        TRAFFIC_DIRECTION_INGRESS => "ingressFilterRuleSet",
        TRAFFIC_DIRECTION_EGRESS => "egressFilterRuleSet",
        other => panic!("{other:?} is no direction the modelled TrafficDirection enum carries"),
    }
}

/// The one Traffic Mirror filter `body` reports: the element that carries a filter id of its
/// own and no rule number — a RULE carries a filter id too, so the rule number is what tells
/// the filter from the rules nested inside it.
fn reported_filter<'a, 'i>(
    case: &str,
    document: &'a roxmltree::Document<'i>,
    body: &str,
) -> roxmltree::Node<'a, 'i> {
    document
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "trafficMirrorFilterId").is_some()
                && child(*node, "ruleNumber").is_none()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no reported Traffic Mirror filter in {body}"))
}

/// The `(ruleNumber, ruleAction)` of every rule the one filter in `body` reports in the
/// direction `traffic_direction`, in the order it reported them.
///
/// Read through the XML parser so a member is never taken out of a nested shape, and paired
/// with the rule number so a row holding several rules names which rule each action belongs to.
fn reported_actions(case: &str, body: &str, traffic_direction: &str) -> Vec<(String, String)> {
    let document = document(case, body);
    let filter = reported_filter(case, &document, body);
    set_items(filter, rule_set(traffic_direction))
        .into_iter()
        .map(|item| {
            (
                child_text(item, "ruleNumber").unwrap_or_default(),
                child_text(item, "ruleAction").unwrap_or_default(),
            )
        })
        .collect()
}

/// One Traffic Mirror filter, and the id it was minted with.
async fn created_filter(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(case, router, "CreateTrafficMirrorFilter", &[]).await;
    element(&body, "trafficMirrorFilterId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the filter's id: {body}"))
}

/// One rule on `filter_id`, created in `traffic_direction` at `rule_number` with `rule_action`.
/// Answers the rule's id and the action the create itself reported.
async fn created_rule(
    case: &str,
    router: &MultiServiceRouter,
    filter_id: &str,
    traffic_direction: &str,
    rule_number: i32,
    rule_action: &str,
) -> (String, Option<String>) {
    let body = ok(
        case,
        router,
        "CreateTrafficMirrorFilterRule",
        &[
            p("TrafficMirrorFilterId", filter_id),
            p("TrafficDirection", traffic_direction),
            p("RuleNumber", rule_number.to_string()),
            p("RuleAction", rule_action),
            p("DestinationCidrBlock", DESTINATION_CIDR),
            p("SourceCidrBlock", SOURCE_CIDR),
        ],
    )
    .await;
    let reported = document(case, &body);
    let rule = reported
        .descendants()
        .find(|node| node.is_element() && child(*node, "ruleNumber").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no reported rule in {body}"));
    let rule_id = child_text(rule, "trafficMirrorFilterRuleId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the rule's id: {body}"));
    (rule_id, child_text(rule, "ruleAction"))
}

/// One `ModifyTrafficMirrorFilterNetworkServices` call on `filter_id`, and the body it
/// answered.
async fn modified_network_services(
    case: &str,
    router: &MultiServiceRouter,
    filter_id: &str,
    add: &[&str],
    remove: &[&str],
) -> String {
    let params = [
        vec![p("TrafficMirrorFilterId", filter_id)],
        indexed("AddNetworkService", add),
        indexed("RemoveNetworkService", remove),
    ]
    .concat();
    ok(
        case,
        router,
        "ModifyTrafficMirrorFilterNetworkServices",
        &params,
    )
    .await
}

/// The body `DescribeTrafficMirrorFilters` answers for the one filter `filter_id` names.
async fn described(case: &str, router: &MultiServiceRouter, filter_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeTrafficMirrorFilters",
        &indexed("TrafficMirrorFilterId", &[filter_id]),
    )
    .await
}

/// Pin that every reader of the filter reports `expected` in `traffic_direction`, and `other`
/// in the direction's counterpart.
async fn every_reader_reports(
    case: &str,
    router: &MultiServiceRouter,
    filter_id: &str,
    traffic_direction: &str,
    expected: &[(String, String)],
    other: &[(String, String)],
) {
    let opposite = if traffic_direction == TRAFFIC_DIRECTION_INGRESS {
        TRAFFIC_DIRECTION_EGRESS
    } else {
        TRAFFIC_DIRECTION_INGRESS
    };
    // Both directions of the network-service change, so neither an add nor a remove can be the
    // call that disturbs a rule's action, with the describe beside each of them.
    let steps: Vec<(String, String)> = vec![
        (
            format!("{case}, the describe"),
            described(case, router, filter_id).await,
        ),
        (
            format!("{case}, the network-service add"),
            modified_network_services(case, router, filter_id, NETWORK_SERVICES, &[]).await,
        ),
        (
            format!("{case}, the describe after the add"),
            described(case, router, filter_id).await,
        ),
        (
            format!("{case}, the network-service remove"),
            modified_network_services(case, router, filter_id, &[], NETWORK_SERVICES).await,
        ),
        (
            format!("{case}, the describe after the remove"),
            described(case, router, filter_id).await,
        ),
    ];
    for (step, body) in steps {
        let actions = reported_actions(&step, &body, traffic_direction);
        assert_eq!(
            actions, expected,
            "case {step:?}: expected the {traffic_direction} rules to report {expected:?}, got \
             {actions:?} in {body}"
        );
        let actions = reported_actions(&step, &body, opposite);
        assert_eq!(
            actions, other,
            "case {step:?}: expected the {opposite} rules to report {other:?}, got {actions:?} \
             in {body}"
        );
    }
}

/// `(rule number, action)` rows as the wire reports them.
fn owned(rows: &[(i32, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(number, action)| (number.to_string(), (*action).to_string()))
        .collect()
}

/// Every reader of a filter reports the action each of its rules was CREATED with: one row per
/// member of the modelled action enum, in each of the two directions.
#[tokio::test]
async fn every_reader_reports_the_action_each_rule_was_created_with() {
    for traffic_direction in TRAFFIC_DIRECTIONS {
        for rule_action in RULE_ACTIONS {
            let case = format!("a {traffic_direction} rule created with {rule_action}");
            let router = emulator();
            let filter_id = created_filter(&case, &router).await;
            let (_, created) = created_rule(
                &case,
                &router,
                &filter_id,
                traffic_direction,
                100,
                rule_action,
            )
            .await;
            // The create's own answer carries the action too, so the stored record and the
            // create cannot disagree about it.
            assert_eq!(
                created.as_deref(),
                Some(*rule_action),
                "case {case:?}: the create must report {rule_action}, got {created:?}"
            );
            every_reader_reports(
                &case,
                &router,
                &filter_id,
                traffic_direction,
                &owned(&[(100, *rule_action)]),
                &[],
            )
            .await;
        }
    }
}

/// Every reader of a filter reports the action a rule was last MODIFIED with — every ordered
/// pair over the modelled enum, including the pair that assigns the action the rule already
/// held, in each of the two directions.
#[tokio::test]
async fn every_reader_reports_the_action_a_rule_was_last_modified_with() {
    for traffic_direction in TRAFFIC_DIRECTIONS {
        for created_action in RULE_ACTIONS {
            for assigned_action in RULE_ACTIONS {
                let case = format!(
                    "a {traffic_direction} rule created with {created_action} and modified to \
                     {assigned_action}"
                );
                let router = emulator();
                let filter_id = created_filter(&case, &router).await;
                let (rule_id, _) = created_rule(
                    &case,
                    &router,
                    &filter_id,
                    traffic_direction,
                    100,
                    created_action,
                )
                .await;
                let body = ok(
                    &case,
                    &router,
                    "ModifyTrafficMirrorFilterRule",
                    &[
                        p("TrafficMirrorFilterRuleId", rule_id),
                        p("RuleAction", *assigned_action),
                    ],
                )
                .await;
                let modified = element(&body, "ruleAction");
                assert_eq!(
                    modified.as_deref(),
                    Some(*assigned_action),
                    "case {case:?}: the modify must report {assigned_action}, got {modified:?}"
                );
                every_reader_reports(
                    &case,
                    &router,
                    &filter_id,
                    traffic_direction,
                    &owned(&[(100, *assigned_action)]),
                    &[],
                )
                .await;
            }
        }
    }
}

/// Each rule of one filter keeps its OWN action: a filter holding several rules in both
/// directions reports each of them with the action that rule carries, so no rule's action is
/// taken from another rule or from a default.
#[tokio::test]
async fn each_rule_of_a_filter_keeps_its_own_action() {
    // The rows differ along how the actions are spread over the filter's rules: all the same,
    // alternating, and the reverse of each — over three rules in each direction, so a reader
    // that reported one rule's action for every rule, or a fixed action, fails on at least one
    // row.
    let spreads: &[&[&str]] = &[
        &["accept", "accept", "accept"],
        &["reject", "reject", "reject"],
        &["accept", "reject", "accept"],
        &["reject", "accept", "reject"],
    ];
    for spread in spreads {
        let case = format!("rules carrying {spread:?}");
        for rule_action in *spread {
            assert!(
                RULE_ACTIONS.contains(rule_action),
                "case {case:?}: every row must name a member of {RULE_ACTIONS:?}"
            );
        }
        let router = emulator();
        let filter_id = created_filter(&case, &router).await;
        // Each direction gets the spread in the opposite order, so the two sets never hold the
        // same sequence of actions and one set cannot be reported for the other.
        let mut reversed: Vec<&str> = spread.to_vec();
        reversed.reverse();
        let mut expected: Vec<(&str, Vec<(String, String)>)> = Vec::new();
        for (traffic_direction, actions) in [
            (TRAFFIC_DIRECTION_INGRESS, spread.to_vec()),
            (TRAFFIC_DIRECTION_EGRESS, reversed),
        ] {
            let mut rows: Vec<(String, String)> = Vec::new();
            for (index, rule_action) in actions.iter().enumerate() {
                // Ascending rule numbers that are not consecutive, so a reader cannot get the
                // pairing right by counting instead of by reading each rule.
                let rule_number = 100 + 10 * i32::try_from(index).unwrap_or_default();
                created_rule(
                    &case,
                    &router,
                    &filter_id,
                    traffic_direction,
                    rule_number,
                    rule_action,
                )
                .await;
                rows.push((rule_number.to_string(), (*rule_action).to_string()));
            }
            expected.push((traffic_direction, rows));
        }
        for (traffic_direction, rows) in &expected {
            let other = expected
                .iter()
                .find(|(direction, _)| direction != traffic_direction)
                .map(|(_, rows)| rows.clone())
                .unwrap_or_default();
            every_reader_reports(&case, &router, &filter_id, traffic_direction, rows, &other).await;
        }
    }
}
