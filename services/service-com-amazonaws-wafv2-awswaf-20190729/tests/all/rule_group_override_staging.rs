// SPDX-License-Identifier: CC0-1.0

//! Staging a rule group this account owns from BLOCK to COUNT, and WAF's own account
//! of what it did, end to end over the wire.
//!
//! The rule this suite pins: for a web ACL rule whose statement runs a rule group the
//! account owns, the action applied to a matching request is the action the group's own
//! rule carries UNLESS the web ACL put something else in its place, and the two levers
//! that can do so are independent of each other —
//! `RuleGroupReferenceStatement$RuleActionOverrides` is "action settings to use in the
//! place of the rule actions that are configured inside the rule group. You specify one
//! override for each rule whose action you want to change", so it replaces the action
//! of the rule it NAMES and of no other, while `OverrideAction$Count` "override[s] the
//! rule group evaluation result to count only" and "does not affect how the rules in
//! the rule group are evaluated", so it takes every one of the group's rules out of
//! force at once. Whichever lever left a matching request counted rather than refused,
//! the request reaches the origin, and the sample `GetSampledRequests` draws reports
//! `Action` as "the action that WAF applied to the request", `OverriddenAction` as "the
//! action that the rule group rule is configured for, and not the action that was
//! applied to the request" — absent where the group's own action was in force — and
//! `RuleNameWithinRuleGroup` in the form the member documents for a group this account
//! owns, `<rule group name>#<rule name>`.
//!
//! The table spans the class rather than the one reported staging sequence: one row per
//! way a web ACL can leave a group rule's action in force or put something else in its
//! place — the group's own actions, one named rule overridden to count, an override
//! naming a DIFFERENT rule of the same group, every rule overridden, a rule overridden
//! to a terminating action that is not the one it is configured for (so the lever is
//! exercised in both directions, not only downwards), a rule excluded by name, the
//! whole group counted by the web ACL rule's `OverrideAction`, both levers at once, and
//! a request none of the group's rules match — each crossed with which of the group's
//! three rules the request matches, so a row says both which rules acted and which did
//! not. Each row states the FULL set of samples the dimension must hold, which is what
//! catches an override that is stored and then ignored: such a web ACL answers with the
//! group's own action and samples it under that action.
//!
//! [`a_staged_rollout_follows_the_overrides_the_web_acl_holds_now`] then drives the same
//! group through the promotion sequence a rollout is staged in — the group's own block,
//! the inner rule overridden to count, the whole group counted, and both levers removed
//! again — on ONE web ACL and ONE protected resource, so each step observes the
//! configuration the control plane holds at the time of the request rather than the one
//! the web ACL was created with.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine the service is mounted on — this service models no data-plane endpoint
//! of its own, because the requests a web ACL decides arrive at the protected resource's
//! endpoint — and the web ACLs, the rule group and the samples all go over the wire, so
//! what the table observes is what an SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};

use crate::rule_group_lifecycle::create_rule_group;
use crate::sampled_rule_provenance::{samples_of, SCOPE};
use crate::web_acl_associations::web_acl_for_resource;
use crate::web_acl_lifecycle::{base64_of, visibility, Waf, TEST_REGION};

/// The separator `SampledHTTPRequest$RuleNameWithinRuleGroup` joins its segments with
/// ("for your own rule groups, the format for this name is `<rule group name>#<rule
/// name>`").
const SEPARATOR: char = '#';

/// The rule group this account owns that every row runs.
const GROUP: &str = "staged-rollout-guard";

/// The group's rules, in the order their priorities run them: two that BLOCK what they
/// match and one that only COUNTS it, so an override can be exercised in both
/// directions.
const BLOCK_PROBE: &str = "block-probe";
const BLOCK_AUDIT: &str = "block-audit";
const COUNT_TRACE: &str = "count-trace";

/// The header each of the group's rules inspects.
const PROBE_HEADER: &str = "x-waf-probe";
const AUDIT_HEADER: &str = "x-waf-audit";
const TRACE_HEADER: &str = "x-waf-trace";

/// The header value the group's rules match, and one they do not.
const MATCHING: &str = "stage-me";
const OTHER: &str = "nothing-to-see";

/// The web ACL rule that runs the group, and so the dimension every sample of it is
/// drawn under (`VisibilityConfig$MetricName`).
const GROUP_REF_RULE: &str = "groupref";

/// The `ActionValue` values this suite expects a sample to report
/// (`smithy_query(query="shape",shape="ActionValue")` lists `ALLOW`, `BLOCK`, `COUNT`,
/// `CAPTCHA`, `CHALLENGE`, `MONETIZE`, `EXCLUDED_AS_COUNT`).
const BLOCK: &str = "BLOCK";
const COUNT: &str = "COUNT";
const CHALLENGE: &str = "CHALLENGE";
const EXCLUDED_AS_COUNT: &str = "EXCLUDED_AS_COUNT";

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. Each row binds its own, so
/// no two web ACLs decide one request.
fn stage_arn(row: &str) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/f1a2b3c4d5/stages/{row}")
}

/// The qualification a sample of `rule` carries: the group's own name, then the rule's
/// group-local name.
fn within_group(rule: &str) -> String {
    format!("{GROUP}{SEPARATOR}{rule}")
}

/// A `ByteMatchStatement` matching `value` EXACTLY in the single header `name`.
fn single_header_statement(name: &str, value: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(value),
            "FieldToMatch": { "SingleHeader": { "Name": name } },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": "EXACTLY",
        }
    })
}

/// One rule of the group: a name, a priority, the header it inspects and its own
/// action.
fn group_rule(name: &str, priority: i64, header: &str, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": single_header_statement(header, MATCHING),
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A web ACL rule that runs the rule group `arn`, carrying `reference`'s extra members
/// (`ExcludedRules`, `RuleActionOverrides`) beside the statement's `ARN`, under
/// `override_action`.
fn group_reference_rule(arn: &str, reference: Value, override_action: Value) -> Value {
    let mut statement = json!({ "RuleGroupReferenceStatement": { "ARN": arn } });
    for (member, value) in reference.as_object().into_iter().flatten() {
        statement["RuleGroupReferenceStatement"][member] = value.clone();
    }
    json!({
        "Name": GROUP_REF_RULE,
        "Priority": 0,
        "Statement": statement,
        "OverrideAction": override_action,
        "VisibilityConfig": visibility(GROUP_REF_RULE),
    })
}

/// `RuleActionOverrides` putting `action_to_use` in the place of the own action of each
/// rule in `names`.
fn overriding(names: &[&str], action_to_use: Value) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| json!({ "Name": name, "ActionToUse": action_to_use }))
            .collect(),
    )
}

/// One request as the edge inspects it, carrying `MATCHING` under each header in
/// `matched` and `OTHER` under each header the group inspects but the row does not
/// match — so every row sends all three headers and only their VALUES differ.
fn request_matching(matched: &[&str]) -> InspectedRequest {
    let headers = [PROBE_HEADER, AUDIT_HEADER, TRACE_HEADER]
        .iter()
        .map(|name| {
            let value = if matched.contains(name) { MATCHING } else { OTHER };
            (name.to_string(), value.to_string())
        })
        .collect();
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: "/prod/override".to_string(),
        query_string: String::new(),
        headers,
        body: Vec::new(),
        client_ip: "198.51.100.7".to_string(),
    }
}

/// What one sample of the group's dimension reports: the action WAF APPLIED, the rule
/// within the group it applied it for, and the action that rule is CONFIGURED for where
/// the web ACL put something else in its place.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    action: String,
    within_rule_group: String,
    overridden_action: Option<String>,
}

/// [`Reported`] from the three values a row writes.
fn reports(action: &str, rule: &str, overridden: Option<&str>) -> Reported {
    Reported {
        action: action.to_string(),
        within_rule_group: within_group(rule),
        overridden_action: overridden.map(str::to_string),
    }
}

/// Every sample the `GROUP_REF_RULE` dimension of `web_acl_arn` holds, as the rule
/// reports them, in a stable order so a row compares a SET rather than an order (which
/// is `crate::web_acl_edge_reports`' own rule).
async fn reported_samples(waf: &Waf, web_acl_arn: &str, case: &str) -> Vec<Reported> {
    let mut reported: Vec<Reported> = samples_of(waf, web_acl_arn, GROUP_REF_RULE)
        .await
        .iter()
        .map(|sample| Reported {
            action: sample["Action"].as_str().unwrap_or_default().to_string(),
            within_rule_group: sample["RuleNameWithinRuleGroup"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every sample of a rule group's rule must name it \
                         within its group, got {sample}"
                    )
                })
                .to_string(),
            overridden_action: sample["OverriddenAction"].as_str().map(str::to_string),
        })
        .collect();
    reported.sort();
    reported
}

/// One way a web ACL can leave a referenced group's rule in force or put something else
/// in its place.
struct Case {
    case: &'static str,
    /// The stage this row's web ACL is put in front of, which is also the row's name.
    stage: &'static str,
    /// The extra members on the `RuleGroupReferenceStatement`.
    reference: Value,
    /// The web ACL rule's own `OverrideAction`.
    override_action: Value,
    /// The headers of the request that carry the value the group's rules match.
    matched: Vec<&'static str>,
    /// Whether the decision lets the request reach the origin.
    allowed: bool,
    /// The rule within the group the decision names, or `None` when no group rule
    /// terminated the evaluation and the `DefaultAction` decided.
    decided_by: Option<&'static str>,
    /// Every sample the group's dimension must hold, and nothing besides.
    samples: Vec<Reported>,
}

/// Create the rule group every row runs, answering its ARN.
async fn create_staging_group(waf: &Waf) -> String {
    let (_, arn, _) = create_rule_group(
        waf,
        SCOPE,
        GROUP,
        100,
        json!([
            group_rule(BLOCK_PROBE, 0, PROBE_HEADER, json!({ "Block": {} })),
            group_rule(BLOCK_AUDIT, 1, AUDIT_HEADER, json!({ "Block": {} })),
            group_rule(COUNT_TRACE, 2, TRACE_HEADER, json!({ "Count": {} })),
        ]),
    )
    .await;
    arn
}

/// The rule over the two override levers: the action applied to a request a referenced
/// group's rule matched is the rule's own unless the web ACL put something else in its
/// place, and WAF's sample reports the applied action, the configured one and the rule's
/// group-qualified name.
#[tokio::test]
async fn an_override_decides_the_action_a_referenced_groups_rule_applies() {
    let (waf, engine) = Waf::sharing_engine();
    let group_arn = create_staging_group(&waf).await;

    let cases = vec![
        Case {
            case: "the group's own actions in force",
            stage: "in-force",
            reference: json!({}),
            override_action: json!({ "None": {} }),
            matched: vec![PROBE_HEADER],
            allowed: false,
            decided_by: Some(BLOCK_PROBE),
            samples: vec![reports(BLOCK, BLOCK_PROBE, None)],
        },
        Case {
            case: "the matching rule overridden to count",
            stage: "overridden-count",
            reference: json!({
                "RuleActionOverrides": overriding(&[BLOCK_PROBE], json!({ "Count": {} }))
            }),
            override_action: json!({ "None": {} }),
            matched: vec![PROBE_HEADER],
            allowed: true,
            decided_by: None,
            samples: vec![reports(COUNT, BLOCK_PROBE, Some(BLOCK))],
        },
        Case {
            case: "an override naming another rule of the same group",
            stage: "overridden-elsewhere",
            reference: json!({
                "RuleActionOverrides": overriding(&[BLOCK_AUDIT], json!({ "Count": {} }))
            }),
            override_action: json!({ "None": {} }),
            matched: vec![PROBE_HEADER],
            allowed: false,
            decided_by: Some(BLOCK_PROBE),
            samples: vec![reports(BLOCK, BLOCK_PROBE, None)],
        },
        Case {
            case: "two rules matching, only the first overridden to count",
            stage: "overridden-first",
            reference: json!({
                "RuleActionOverrides": overriding(&[BLOCK_PROBE], json!({ "Count": {} }))
            }),
            override_action: json!({ "None": {} }),
            matched: vec![PROBE_HEADER, AUDIT_HEADER],
            allowed: false,
            decided_by: Some(BLOCK_AUDIT),
            samples: vec![
                reports(BLOCK, BLOCK_AUDIT, None),
                reports(COUNT, BLOCK_PROBE, Some(BLOCK)),
            ],
        },
        Case {
            case: "every blocking rule overridden to count",
            stage: "overridden-all",
            reference: json!({
                "RuleActionOverrides":
                    overriding(&[BLOCK_PROBE, BLOCK_AUDIT], json!({ "Count": {} }))
            }),
            override_action: json!({ "None": {} }),
            matched: vec![PROBE_HEADER, AUDIT_HEADER],
            allowed: true,
            decided_by: None,
            samples: vec![
                reports(COUNT, BLOCK_AUDIT, Some(BLOCK)),
                reports(COUNT, BLOCK_PROBE, Some(BLOCK)),
            ],
        },
        Case {
            case: "a counting rule overridden to a terminating action",
            stage: "overridden-upwards",
            reference: json!({
                "RuleActionOverrides": overriding(&[COUNT_TRACE], json!({ "Challenge": {} }))
            }),
            override_action: json!({ "None": {} }),
            matched: vec![TRACE_HEADER],
            allowed: false,
            decided_by: Some(COUNT_TRACE),
            samples: vec![reports(CHALLENGE, COUNT_TRACE, Some(COUNT))],
        },
        Case {
            case: "the group's own counting rule in force",
            stage: "counting-in-force",
            reference: json!({}),
            override_action: json!({ "None": {} }),
            matched: vec![TRACE_HEADER],
            allowed: true,
            decided_by: None,
            samples: vec![reports(COUNT, COUNT_TRACE, None)],
        },
        Case {
            case: "the matching rule excluded by name",
            stage: "excluded",
            reference: json!({ "ExcludedRules": [{ "Name": BLOCK_PROBE }] }),
            override_action: json!({ "None": {} }),
            matched: vec![PROBE_HEADER],
            allowed: true,
            decided_by: None,
            samples: vec![reports(EXCLUDED_AS_COUNT, BLOCK_PROBE, Some(BLOCK))],
        },
        Case {
            case: "the whole group counted by the web ACL rule's OverrideAction",
            stage: "group-counted",
            reference: json!({}),
            override_action: json!({ "Count": {} }),
            matched: vec![PROBE_HEADER, AUDIT_HEADER],
            allowed: true,
            decided_by: None,
            samples: vec![
                reports(COUNT, BLOCK_AUDIT, Some(BLOCK)),
                reports(COUNT, BLOCK_PROBE, Some(BLOCK)),
            ],
        },
        Case {
            case: "both levers at once",
            stage: "both-levers",
            reference: json!({
                "RuleActionOverrides": overriding(&[BLOCK_PROBE], json!({ "Count": {} }))
            }),
            override_action: json!({ "Count": {} }),
            matched: vec![PROBE_HEADER],
            allowed: true,
            decided_by: None,
            samples: vec![reports(COUNT, BLOCK_PROBE, Some(BLOCK))],
        },
        Case {
            case: "a request none of the group's rules match",
            stage: "unmatched",
            reference: json!({}),
            override_action: json!({ "None": {} }),
            matched: Vec::new(),
            allowed: true,
            decided_by: None,
            samples: Vec::new(),
        },
    ];

    for case in &cases {
        let web_acl_name = format!("staging-{}", case.stage);
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": &web_acl_name,
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&web_acl_name),
                    "Rules": [group_reference_rule(
                        &group_arn,
                        case.reference.clone(),
                        case.override_action.clone(),
                    )],
                }),
            )
            .await;
        let web_acl_arn = created["Summary"]["ARN"]
            .as_str()
            .expect("Summary.ARN")
            .to_string();
        let resource = stage_arn(case.stage);
        waf.ok(
            "AssociateWebACL",
            json!({ "WebACLArn": &web_acl_arn, "ResourceArn": &resource }),
        )
        .await;

        let request = request_matching(&case.matched);
        let decision = edge::decide(&*engine, &resource, &request).unwrap_or_else(|| {
            panic!(
                "case {:?}: the web ACL associated with {resource} must decide the request",
                case.case
            )
        });
        assert_eq!(
            decision.action.is_allow(),
            case.allowed,
            "case {:?}: expected allowed={}, got the action {:?}",
            case.case,
            case.allowed,
            decision.action
        );
        assert_eq!(
            decision.rule_name_within_rule_group,
            case.decided_by.map(within_group),
            "case {:?}: expected the decision to name {:?} within its group, got {:?}",
            case.case,
            case.decided_by,
            decision.rule_name_within_rule_group
        );

        let reported = reported_samples(&waf, &web_acl_arn, case.case).await;
        assert_eq!(
            reported, case.samples,
            "case {:?}: expected the {GROUP_REF_RULE:?} dimension to hold {:?}, got {reported:?}",
            case.case, case.samples
        );
    }
}

/// `UpdateWebACL` on `web_acl_name`/`web_acl_id` with the reference members and
/// `OverrideAction` of one step of the rollout, applied with the token the web ACL holds
/// now.
async fn restage(
    waf: &Waf,
    web_acl_name: &str,
    web_acl_id: &str,
    group_arn: &str,
    reference: Value,
    override_action: Value,
) {
    let held = waf
        .ok(
            "GetWebACL",
            json!({ "Name": web_acl_name, "Scope": SCOPE, "Id": web_acl_id }),
        )
        .await;
    let lock_token = held["LockToken"]
        .as_str()
        .expect("LockToken")
        .to_string();
    waf.ok(
        "UpdateWebACL",
        json!({
            "Name": web_acl_name,
            "Scope": SCOPE,
            "Id": web_acl_id,
            "LockToken": lock_token,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility(web_acl_name),
            "Rules": [group_reference_rule(group_arn, reference, override_action)],
        }),
    )
    .await;
}

/// One step of a staged rollout: what the web ACL holds, and what the edge then does
/// with the identical request.
struct Step {
    case: &'static str,
    reference: Value,
    override_action: Value,
    allowed: bool,
    samples: Vec<Reported>,
}

/// The rule over a STAGED rollout: every request is decided against the overrides the
/// web ACL holds at the time of the request, so promoting a group from its own block to
/// count and back changes what the edge does without the binding being written again —
/// and the web ACL reads back carrying the levers that are in force, with the protected
/// resource still reporting the same web ACL in front of it throughout.
#[tokio::test]
async fn a_staged_rollout_follows_the_overrides_the_web_acl_holds_now() {
    let (waf, engine) = Waf::sharing_engine();
    let group_arn = create_staging_group(&waf).await;

    let web_acl_name = "staged-rollout";
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": web_acl_name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(web_acl_name),
                "Rules": [group_reference_rule(
                    &group_arn,
                    json!({}),
                    json!({ "None": {} }),
                )],
            }),
        )
        .await;
    let web_acl_id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let web_acl_arn = created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    let resource = stage_arn("rollout");
    waf.ok(
        "AssociateWebACL",
        json!({ "WebACLArn": &web_acl_arn, "ResourceArn": &resource }),
    )
    .await;

    // The dimension accumulates a sample per step, so each step states the whole set.
    let counted_probe = || reports(COUNT, BLOCK_PROBE, Some(BLOCK));
    let steps = vec![
        Step {
            case: "the group's own block, as it was shipped",
            reference: json!({}),
            override_action: json!({ "None": {} }),
            allowed: false,
            samples: vec![reports(BLOCK, BLOCK_PROBE, None)],
        },
        Step {
            case: "the inner rule overridden to count",
            reference: json!({
                "RuleActionOverrides": overriding(&[BLOCK_PROBE], json!({ "Count": {} }))
            }),
            override_action: json!({ "None": {} }),
            allowed: true,
            samples: vec![reports(BLOCK, BLOCK_PROBE, None), counted_probe()],
        },
        Step {
            case: "no per-rule override, the whole group counted instead",
            reference: json!({}),
            override_action: json!({ "Count": {} }),
            allowed: true,
            samples: vec![
                reports(BLOCK, BLOCK_PROBE, None),
                counted_probe(),
                counted_probe(),
            ],
        },
        Step {
            case: "both levers removed again, promoting the group to block",
            reference: json!({}),
            override_action: json!({ "None": {} }),
            allowed: false,
            samples: vec![
                reports(BLOCK, BLOCK_PROBE, None),
                reports(BLOCK, BLOCK_PROBE, None),
                counted_probe(),
                counted_probe(),
            ],
        },
    ];

    for step in &steps {
        restage(
            &waf,
            web_acl_name,
            &web_acl_id,
            &group_arn,
            step.reference.clone(),
            step.override_action.clone(),
        )
        .await;

        // The web ACL reads back carrying exactly the levers this step put in force,
        // which is what a client diffs its own template against.
        let held = waf
            .ok(
                "GetWebACL",
                json!({ "Name": web_acl_name, "Scope": SCOPE, "Id": &web_acl_id }),
            )
            .await;
        let reference = &held["WebACL"]["Rules"][0]["Statement"]["RuleGroupReferenceStatement"];
        assert_eq!(
            reference["RuleActionOverrides"],
            step.reference
                .get("RuleActionOverrides")
                .cloned()
                .unwrap_or(Value::Null),
            "case {:?}: expected GetWebACL to round-trip the RuleActionOverrides this step \
             stored, got {reference}",
            step.case
        );
        assert_eq!(
            held["WebACL"]["Rules"][0]["OverrideAction"], step.override_action,
            "case {:?}: expected GetWebACL to round-trip the OverrideAction this step \
             stored, got {}",
            step.case, held["WebACL"]["Rules"][0]
        );

        // The identical request, decided against what the web ACL holds now.
        let decision = edge::decide(&*engine, &resource, &request_matching(&[PROBE_HEADER]))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the web ACL associated with {resource} must decide the request",
                    step.case
                )
            });
        assert_eq!(
            decision.action.is_allow(),
            step.allowed,
            "case {:?}: expected allowed={}, got the action {:?}",
            step.case,
            step.allowed,
            decision.action
        );

        let mut expected: Vec<&Reported> = step.samples.iter().collect();
        expected.sort();
        let reported = reported_samples(&waf, &web_acl_arn, step.case).await;
        let reported_refs: Vec<&Reported> = reported.iter().collect();
        assert_eq!(
            reported_refs, expected,
            "case {:?}: expected the {GROUP_REF_RULE:?} dimension to hold {expected:?}, got \
             {reported:?}",
            step.case
        );

        // The binding is untouched by any of it: the resource still reports this web ACL
        // in front of it.
        let in_front = web_acl_for_resource(&waf, &resource).await;
        assert_eq!(
            in_front
                .as_ref()
                .and_then(|web_acl| web_acl["ARN"].as_str()),
            Some(web_acl_arn.as_str()),
            "case {:?}: expected {resource} to still report the web ACL {web_acl_arn} in \
             front of it, got {in_front:?}",
            step.case
        );
    }
}
