// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ENGINE rule-group kinds the model added — the
//! `FIREWALL` and `IPS` rule groups a Next Generation policy's sections evaluate,
//! written through the `FirewallRuleGroup` and `IpsRuleGroup` documents.
//!
//! This service writes none of them: they are reported as not written yet rather
//! than stored under a document the service would have to invent, and nothing a
//! caller can read is changed by asking for one.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use serde_json::{json, Value};

use crate::harness::{error_code, string_at, Harness, NOT_FOUND_MESSAGE};

/// A minimal stateless rule-group document, the one every other suite writes.
fn stateless_document() -> Value {
    json!({
        "RulesSource": {
            "StatelessRulesAndCustomActions": {
                "StatelessRules": [{
                    "Priority": 1,
                    "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
                }],
            },
        },
    })
}

/// A firewall engine rule-group document.
fn firewall_document() -> Value {
    json!({ "RulesSource": { "RulesString": "pass tcp any any -> any any (sid:1;)" } })
}

/// An IPS engine rule-group document.
fn ips_document() -> Value {
    json!({ "RulesSource": { "RulesString": "alert tcp any any -> any any (sid:1;)" } })
}

/// RULE: a rule group of an ENGINE kind, or one submitted as an engine document,
/// is reported as not written yet (HTTP 501) rather than stored — on the create
/// and on the update alike — and the request leaves the rule-group store exactly
/// as it was.
///
/// The rows span both engine types, both engine documents on a type that is not
/// an engine one, and an engine type beside an engine document.
#[tokio::test]
async fn an_engine_rule_group_is_reported_as_not_written_yet() {
    // (case, the members the create adds beside the name and capacity)
    let rows: [(&str, Value); 5] = [
        (
            "the FIREWALL type",
            json!({ "Type": "FIREWALL", "RuleGroup": stateless_document() }),
        ),
        (
            "the IPS type",
            json!({ "Type": "IPS", "RuleGroup": stateless_document() }),
        ),
        (
            "a firewall engine document on a stateful rule group",
            json!({ "Type": "STATEFUL", "FirewallRuleGroup": firewall_document() }),
        ),
        (
            "an IPS engine document on a stateful rule group",
            json!({ "Type": "STATEFUL", "IpsRuleGroup": ips_document() }),
        ),
        (
            "the FIREWALL type beside its own document",
            json!({ "Type": "FIREWALL", "FirewallRuleGroup": firewall_document() }),
        ),
    ];

    let harness = Harness::new();
    for (case, members) in rows {
        let mut request = json!({ "RuleGroupName": "engine-group", "Capacity": 10 });
        for (member, value) in members.as_object().expect("the row's members") {
            request[member] = value.clone();
        }
        let (status, body) = harness.call("CreateRuleGroup", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {case}: expected the engine rule group reported as not written yet, got \
             {status} {body:?}"
        );

        // Nothing was stored under the name: the listing is still empty, and a
        // read of either half of the name space misses.
        let listed = harness.ok("ListRuleGroups", &json!({})).await;
        assert_eq!(
            listed["RuleGroups"],
            json!([]),
            "case {case}: expected no rule group stored, got {listed:?}"
        );
        for group_type in ["STATEFUL", "STATELESS"] {
            let (status, body) = harness
                .call(
                    "DescribeRuleGroup",
                    &json!({ "RuleGroupName": "engine-group", "Type": group_type }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: expected no rule group under {group_type}, got {status} {body:?}"
            );
        }
    }

    // The update path answers the same way, and leaves the rule group it names
    // untouched.
    harness
        .shared_rule_group("engine-update", "STATEFUL", 10)
        .await;
    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "engine-update", "Type": "STATEFUL" }),
        )
        .await;
    let token = string_at(&described, &["UpdateToken"]).to_string();
    for (case, members) in [
        (
            "a firewall engine document",
            json!({ "FirewallRuleGroup": firewall_document() }),
        ),
        (
            "an IPS engine document",
            json!({ "IpsRuleGroup": ips_document() }),
        ),
    ] {
        let mut request = json!({
            "RuleGroupName": "engine-update",
            "Type": "STATEFUL",
            "UpdateToken": token,
        });
        for (member, value) in members.as_object().expect("the row's members") {
            request[member] = value.clone();
        }
        let (status, body) = harness.call("UpdateRuleGroup", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {case} on the update: expected not written yet, got {status} {body:?}"
        );
        let after = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": "engine-update", "Type": "STATEFUL" }),
            )
            .await;
        assert_eq!(
            after, described,
            "case {case} on the update: expected the stored rule group untouched, got {after:?}"
        );
    }
}

/// RULE: a rule group of an ENGINE kind is a rule group of its own kind, so a
/// request that asks for that kind resolves to nothing — never to the rule group
/// of another kind that happens to carry the same name — and a read of a rule
/// group this service does hold reports no engine document.
#[tokio::test]
async fn an_engine_type_names_no_rule_group_of_another_kind() {
    let harness = Harness::new();
    for group_type in ["STATEFUL", "STATELESS"] {
        harness
            .shared_rule_group("shared-name", group_type, 10)
            .await;
    }

    for engine_type in ["FIREWALL", "IPS"] {
        for operation in ["DescribeRuleGroup", "DeleteRuleGroup"] {
            let (status, body) = harness
                .call(
                    operation,
                    &json!({ "RuleGroupName": "shared-name", "Type": engine_type }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str(), body["Message"].as_str()),
                (400, "ResourceNotFoundException", Some(NOT_FOUND_MESSAGE)),
                "case {engine_type} on {operation}: expected no rule group of that kind, got \
                 {status} {body:?}"
            );
        }
    }

    // The rule groups this service does hold carry no engine document, so a read
    // reports neither engine member — and the delete refused above removed
    // neither of them.
    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "shared-name", "Type": "STATEFUL" }),
        )
        .await;
    harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "shared-name", "Type": "STATELESS" }),
        )
        .await;
    for member in ["FirewallRuleGroup", "IpsRuleGroup"] {
        assert_eq!(
            described.get(member),
            None,
            "expected no {member} on a rule group written through RuleGroup, got {described:?}"
        );
    }
    assert!(
        described.get("RuleGroup").is_some(),
        "expected the stored RuleGroup document reported, got {described:?}"
    );
}
