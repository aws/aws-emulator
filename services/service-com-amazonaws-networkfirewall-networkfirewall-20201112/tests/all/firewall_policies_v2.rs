// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Next Generation firewall policies: the
//! `CreateFirewallPolicyV2`, `DescribeFirewallPolicyV2`,
//! `UpdateFirewallPolicyV2`, `DeleteFirewallPolicyV2` and
//! `ListFirewallPoliciesV2` operations the model added.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{
    error_code, error_message, is_uuid_form, minimal_policy_document, string_at, with_tag_list,
    Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// The error code every bad-request refusal of these operations carries.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The error code a miss carries.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The ARN a firewall policy of `name` carries in the region these tests sign
/// for. Both generations of policy mint the same `firewall-policy/<name>` ARN,
/// since a firewall names the policy it uses through one `FirewallPolicyArn`.
fn policy_arn(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/{name}")
}

/// An embedded firewall engine rule group carrying `rules` as a
/// Suricata-compatible firewall-mode rule string.
fn inline_firewall_rules_string(rules: &str) -> Value {
    json!({ "RulesSource": { "RulesString": rules } })
}

/// An embedded firewall engine rule group carrying `count` structured
/// `FirewallRule` objects, each the minimal one the model requires (an action, an
/// action scope, a hook and a protocol).
fn inline_firewall_rules(count: usize) -> Value {
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "Action": "ACCEPT",
                "ActionScope": "FLOW",
                "Hook": "TCP_PRE_STREAM",
                "Protocol": "TCP",
                "RuleOptions": [{ "Keyword": "sid", "Settings": [format!("{}", index + 1)] }],
            })
        })
        .collect();
    json!({ "RulesSource": { "FirewallRules": rules } })
}

/// An embedded IPS engine rule group carrying `rules` as a Suricata-compatible
/// rule string.
fn inline_ips_rules_string(rules: &str) -> Value {
    json!({ "RulesSource": { "RulesString": rules } })
}

/// One Next Generation policy document, named for the class of document it
/// stands for.
struct PolicyCase {
    label: &'static str,
    name: &'static str,
    document: Value,
    /// The `(firewall, ips, ips-domain)` capacity units the document consumes.
    consumed: [i64; 3],
}

/// The documents the round-trip and the capacity rules are asserted over: they
/// vary along what those rules generalise over — which sections the document
/// carries, how an embedded rule group states its rules (a rules string or
/// structured rules), how many rules it carries, and which policy-wide settings
/// are set.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            label: "a document with no section at all",
            name: "ng-empty",
            document: json!({}),
            consumed: [0, 0, 0],
        },
        PolicyCase {
            label: "one embedded firewall rule group given as a rule string",
            name: "ng-firewall-string",
            document: json!({
                "PolicyMode": "ENFORCE",
                "FirewallDefaultAction": "DROP",
                "FirewallRuleGroupEntries": [{
                    "Priority": 1,
                    "InlineRuleGroup": inline_firewall_rules_string(
                        "pass tcp any any -> any any (sid:1;)\n# a comment\n\npass tcp any any -> any any (sid:2;)",
                    ),
                }],
            }),
            consumed: [2, 0, 0],
        },
        PolicyCase {
            label: "one embedded firewall rule group given as structured rules",
            name: "ng-firewall-structured",
            document: json!({
                "PolicyMode": "MONITOR",
                "FirewallDefaultAction": "ALERT",
                "FirewallRuleGroupEntries": [{
                    "Priority": 100,
                    "InlineRuleGroup": inline_firewall_rules(3),
                }],
            }),
            consumed: [3, 0, 0],
        },
        PolicyCase {
            label: "an IPS section only",
            name: "ng-ips-only",
            document: json!({
                "IpsRuleGroupEntries": [{
                    "InlineRuleGroup": inline_ips_rules_string(
                        "alert tcp any any -> any any (sid:1;)",
                    ),
                    "Override": { "Action": "DROP_TO_ALERT" },
                }],
            }),
            consumed: [0, 1, 0],
        },
        PolicyCase {
            label: "both sections, several entries each, and policy-wide settings",
            name: "ng-both-sections",
            document: json!({
                "PolicyMode": "ENFORCE",
                "FirewallDefaultAction": "REJECT",
                "StatefulEngineOptions": { "StreamExceptionPolicy": "DROP" },
                "PolicyVariables": { "RuleVariables": { "HOME_NET": { "Definition": ["10.0.0.0/16"] } } },
                "FirewallRuleGroupEntries": [
                    { "Priority": 1, "InlineRuleGroup": inline_firewall_rules(2) },
                    {
                        "Priority": 2,
                        "InlineRuleGroup": inline_firewall_rules_string(
                            "pass tcp any any -> any any (sid:7;)",
                        ),
                    },
                ],
                "IpsRuleGroupEntries": [
                    {
                        "InlineRuleGroup": inline_ips_rules_string(
                            "alert tcp any any -> any any (sid:1;)\nalert udp any any -> any any (sid:2;)",
                        ),
                    },
                ],
            }),
            consumed: [3, 2, 0],
        },
    ]
}

/// RULE: a Next Generation policy is a resource of this service: the create
/// stores the submitted document under a `firewall-policy/<name>` ARN with a
/// server-minted id and token, a read reports back exactly what was stored
/// together with the capacity its sections consume, and the listing carries its
/// name and ARN until a delete removes it.
///
/// The rows span the document classes [`policy_cases`] describes.
#[tokio::test]
async fn a_next_generation_policy_round_trips_through_its_own_operations() {
    let harness = Harness::new();
    for case in policy_cases() {
        let created = harness
            .ok(
                "CreateFirewallPolicyV2",
                &json!({
                    "FirewallPolicyV2Name": case.name,
                    "FirewallPolicyV2": case.document,
                    "Description": format!("{} policy", case.label),
                }),
            )
            .await;
        let response = &created["FirewallPolicyV2Response"];
        assert_eq!(
            response["FirewallPolicyV2Name"],
            json!(case.name),
            "case {}: expected the name the request gave, got {created:?}",
            case.label
        );
        assert_eq!(
            response["FirewallPolicyV2Arn"],
            json!(policy_arn(case.name)),
            "case {}: expected the policy ARN built from the request's region, got {created:?}",
            case.label
        );
        assert!(
            is_uuid_form(string_at(&created, &["FirewallPolicyV2Response", "FirewallPolicyV2Id"])),
            "case {}: expected a server-minted id, got {created:?}",
            case.label
        );
        assert!(
            is_uuid_form(string_at(&created, &["UpdateToken"])),
            "case {}: expected a server-minted update token, got {created:?}",
            case.label
        );
        assert_eq!(
            response["FirewallPolicyV2Status"],
            json!("ACTIVE"),
            "case {}: expected the policy ready to be used, got {created:?}",
            case.label
        );
        assert_eq!(
            response["NumberOfAssociations"],
            json!(0),
            "case {}: expected no firewall using the new policy, got {created:?}",
            case.label
        );
        let consumed = [
            ("ConsumedFirewallRuleCapacity", case.consumed[0]),
            ("ConsumedIpsRuleCapacity", case.consumed[1]),
            ("ConsumedIpsDomainCapacity", case.consumed[2]),
        ];
        for (member, units) in consumed {
            assert_eq!(
                response[member],
                json!(units),
                "case {}: expected {member} {units}, got {created:?}",
                case.label
            );
        }

        // The read reports the stored document and the same high-level
        // properties the create minted.
        let described = harness
            .ok(
                "DescribeFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": case.name }),
            )
            .await;
        assert_eq!(
            described["FirewallPolicyV2"], case.document,
            "case {}: expected the document as submitted, got {described:?}",
            case.label
        );
        assert_eq!(
            described["FirewallPolicyV2Response"],
            with_tag_list(response),
            "case {}: expected the stored response, got {described:?}",
            case.label
        );
        assert_eq!(
            described["UpdateToken"], created["UpdateToken"],
            "case {}: expected the token the create minted, got {described:?}",
            case.label
        );

        // The same policy is reachable by ARN alone.
        let by_arn = harness
            .ok(
                "DescribeFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Arn": policy_arn(case.name) }),
            )
            .await;
        assert_eq!(
            by_arn, described,
            "case {}: expected the ARN to name the same policy as the name, got {by_arn:?}",
            case.label
        );
    }

    // The listing carries every stored policy, in name order, with the ARN each
    // reports.
    let listed = harness.ok("ListFirewallPoliciesV2", &json!({})).await;
    let mut expected: Vec<Value> = policy_cases()
        .iter()
        .map(|case| json!({ "Name": case.name, "Arn": policy_arn(case.name) }))
        .collect();
    expected.sort_by_key(|entry| entry["Name"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        listed["FirewallPoliciesV2"],
        json!(expected),
        "expected every stored policy in name order, got {listed:?}"
    );

    // A delete reports the removed policy as DELETING, and the policy is then
    // gone from the read and from the listing.
    for case in policy_cases() {
        let deleted = harness
            .ok(
                "DeleteFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": case.name }),
            )
            .await;
        assert_eq!(
            deleted["FirewallPolicyV2Response"]["FirewallPolicyV2Status"],
            json!("DELETING"),
            "case {}: expected the terminal status of the removed policy, got {deleted:?}",
            case.label
        );
        let (status, body) = harness
            .call(
                "DescribeFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": case.name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body).as_str()),
            (400, NOT_FOUND, NOT_FOUND_MESSAGE),
            "case {}: expected the deleted policy to be gone, got {status} {body:?}",
            case.label
        );
    }
    let listed = harness.ok("ListFirewallPoliciesV2", &json!({})).await;
    assert_eq!(
        listed["FirewallPoliciesV2"],
        json!([]),
        "expected an empty listing once every policy is deleted, got {listed:?}"
    );
}

/// RULE: an update replaces the stored document under the caller's token and
/// re-mints that token, so a later read reports the new document and the stale
/// token no longer opens an update.
///
/// The rows span replacing a section with another, adding a section, emptying
/// the document, and changing only the policy-wide settings.
#[tokio::test]
async fn an_update_replaces_the_stored_document_under_the_current_token() {
    // (case, the document the update submits)
    let rows: [(&str, Value); 4] = [
        (
            "another embedded firewall rule group",
            json!({
                "FirewallRuleGroupEntries": [{
                    "Priority": 5,
                    "InlineRuleGroup": inline_firewall_rules(1),
                }],
            }),
        ),
        (
            "an IPS section added beside the firewall one",
            json!({
                "FirewallRuleGroupEntries": [{
                    "Priority": 1,
                    "InlineRuleGroup": inline_firewall_rules(1),
                }],
                "IpsRuleGroupEntries": [{
                    "InlineRuleGroup": inline_ips_rules_string(
                        "alert tcp any any -> any any (sid:1;)",
                    ),
                }],
            }),
        ),
        ("every section dropped", json!({})),
        (
            "only the policy-wide settings",
            json!({ "PolicyMode": "MONITOR", "FirewallDefaultAction": "ACCEPT" }),
        ),
    ];

    let harness = Harness::new();
    for (case, document) in rows {
        let name = format!("ng-update-{}", document.to_string().len());
        let created = harness
            .ok(
                "CreateFirewallPolicyV2",
                &json!({
                    "FirewallPolicyV2Name": name,
                    "FirewallPolicyV2": {
                        "FirewallRuleGroupEntries": [{
                            "Priority": 1,
                            "InlineRuleGroup": inline_firewall_rules_string(
                                "pass tcp any any -> any any (sid:1;)",
                            ),
                        }],
                    },
                }),
            )
            .await;
        let token = string_at(&created, &["UpdateToken"]).to_string();

        let updated = harness
            .ok(
                "UpdateFirewallPolicyV2",
                &json!({
                    "FirewallPolicyV2Name": name,
                    "UpdateToken": token,
                    "FirewallPolicyV2": document,
                    "Description": "updated",
                }),
            )
            .await;
        let new_token = string_at(&updated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&new_token) && new_token != token,
            "case {case}: expected a freshly minted token, got {updated:?}"
        );

        let described = harness
            .ok(
                "DescribeFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": name }),
            )
            .await;
        assert_eq!(
            described["FirewallPolicyV2"], document,
            "case {case}: expected the document the update submitted, got {described:?}"
        );
        assert_eq!(
            described["FirewallPolicyV2Response"]["Description"],
            json!("updated"),
            "case {case}: expected the description the update gave, got {described:?}"
        );
        assert_eq!(
            described["UpdateToken"],
            json!(new_token),
            "case {case}: expected the token the update minted, got {described:?}"
        );

        // The token the update consumed no longer opens one.
        let (status, body) = harness
            .call(
                "UpdateFirewallPolicyV2",
                &json!({
                    "FirewallPolicyV2Name": name,
                    "UpdateToken": token,
                    "FirewallPolicyV2": json!({}),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidTokenException"),
            "case {case}: expected the stale token refused, got {status} {body:?}"
        );
        let after = harness
            .ok(
                "DescribeFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": name }),
            )
            .await;
        assert_eq!(
            after, described,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }
}

/// RULE: `DryRun` computes the response the request would have produced and
/// stores nothing — on the create, where the policy is then unknown to every
/// reader, and on the update, where the stored document and token are left as
/// they were.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_stores_nothing() {
    let harness = Harness::new();

    let dry_created = harness
        .ok(
            "CreateFirewallPolicyV2",
            &json!({
                "FirewallPolicyV2Name": "ng-dry-run",
                "FirewallPolicyV2": { "PolicyMode": "ENFORCE" },
                "DryRun": true,
            }),
        )
        .await;
    assert_eq!(
        dry_created["FirewallPolicyV2Response"]["FirewallPolicyV2Arn"],
        json!(policy_arn("ng-dry-run")),
        "case the create: expected the ARN the policy would have had, got {dry_created:?}"
    );
    let (status, body) = harness
        .call(
            "DescribeFirewallPolicyV2",
            &json!({ "FirewallPolicyV2Name": "ng-dry-run" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, NOT_FOUND),
        "case the create: expected nothing stored, got {status} {body:?}"
    );

    let created = harness
        .ok(
            "CreateFirewallPolicyV2",
            &json!({
                "FirewallPolicyV2Name": "ng-dry-update",
                "FirewallPolicyV2": { "PolicyMode": "ENFORCE" },
            }),
        )
        .await;
    let token = string_at(&created, &["UpdateToken"]).to_string();
    let dry_updated = harness
        .ok(
            "UpdateFirewallPolicyV2",
            &json!({
                "FirewallPolicyV2Name": "ng-dry-update",
                "UpdateToken": token,
                "FirewallPolicyV2": { "PolicyMode": "MONITOR" },
                "DryRun": true,
            }),
        )
        .await;
    assert_ne!(
        dry_updated["UpdateToken"], created["UpdateToken"],
        "case the update: expected the token the update would have minted, got {dry_updated:?}"
    );
    let described = harness
        .ok(
            "DescribeFirewallPolicyV2",
            &json!({ "FirewallPolicyV2Name": "ng-dry-update" }),
        )
        .await;
    assert_eq!(
        described["FirewallPolicyV2"],
        json!({ "PolicyMode": "ENFORCE" }),
        "case the update: expected the stored document untouched, got {described:?}"
    );
    assert_eq!(
        described["UpdateToken"],
        json!(token),
        "case the update: expected the stored token untouched, got {described:?}"
    );
}

/// RULE: a firewall-policy NAME identifies one policy whichever generation
/// created it, because both generations mint the same `firewall-policy/<name>`
/// ARN — so a create under a name either generation already holds is refused
/// with this service's constant already-exists message, and nothing is stored.
///
/// The rows span both orders: the first-generation create first, then the Next
/// Generation one, and the reverse.
#[tokio::test]
async fn one_firewall_policy_name_space_spans_both_generations() {
    let harness = Harness::new();

    // (case, the operation that takes the name, the operation refused after it)
    harness.create_policy("ng-shared-v1").await;
    let (status, body) = harness
        .call(
            "CreateFirewallPolicyV2",
            &json!({
                "FirewallPolicyV2Name": "ng-shared-v1",
                "FirewallPolicyV2": json!({}),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body).as_str()),
        (
            400,
            INVALID_REQUEST,
            "A resource with the specified name already exists"
        ),
        "case the first-generation policy first: expected the name refused, got {status} {body:?}"
    );

    harness
        .ok(
            "CreateFirewallPolicyV2",
            &json!({
                "FirewallPolicyV2Name": "ng-shared-v2",
                "FirewallPolicyV2": json!({}),
            }),
        )
        .await;
    let (status, body) = harness
        .call(
            "CreateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "ng-shared-v2",
                "FirewallPolicy": minimal_policy_document(),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body).as_str()),
        (
            400,
            INVALID_REQUEST,
            "A resource with the specified name already exists"
        ),
        "case the Next Generation policy first: expected the name refused, got {status} {body:?}"
    );

    // Each generation still reports only its own policy, so one name space does
    // not merge the two stores.
    let (status, body) = harness
        .call(
            "DescribeFirewallPolicyV2",
            &json!({ "FirewallPolicyV2Name": "ng-shared-v1" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, NOT_FOUND),
        "expected a first-generation policy to be unknown to the V2 read, got {status} {body:?}"
    );
    let (status, body) = harness
        .call(
            "DescribeFirewallPolicy",
            &json!({ "FirewallPolicyName": "ng-shared-v2" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, NOT_FOUND),
        "expected a Next Generation policy to be unknown to the V1 read, got {status} {body:?}"
    );
}

/// RULE: a section entry names exactly one rule group — either an existing
/// engine rule group's ARN or an embedded rule group — an embedded one carries no
/// domain list, and each firewall-section entry carries a priority no other entry
/// carries; a document breaking any of those is refused as the bad request it is
/// and nothing is stored.
///
/// The rows span: both ways of naming a rule group at once, neither, an embedded
/// domain list, two entries sharing a priority, two entries with no priority at
/// all, and the same one-of rule in the IPS section.
#[tokio::test]
async fn a_malformed_section_entry_is_refused() {
    // (case, the document, the stem the refusal states)
    let rows: [(&str, Value, &str); 6] = [
        (
            "an entry naming both an ARN and an embedded rule group",
            json!({
                "FirewallRuleGroupEntries": [{
                    "Priority": 1,
                    "ResourceArn": format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:stateful-rulegroup/some-group"),
                    "InlineRuleGroup": inline_firewall_rules(1),
                }],
            }),
            "Provide either ResourceArn or InlineRuleGroup, not both",
        ),
        (
            "an entry naming neither",
            json!({ "FirewallRuleGroupEntries": [{ "Priority": 1 }] }),
            "Provide either ResourceArn or InlineRuleGroup",
        ),
        (
            "an embedded rule group using a domain list",
            json!({
                "FirewallRuleGroupEntries": [{
                    "Priority": 1,
                    "InlineRuleGroup": {
                        "RulesSource": {
                            "RulesSourceList": {
                                "Targets": ["example.com"],
                                "TargetTypes": ["HTTP_HOST"],
                                "GeneratedRulesType": "ALLOWLIST",
                            },
                        },
                    },
                }],
            }),
            "An embedded rule group can't use a domain list rules source",
        ),
        (
            "two firewall-section entries sharing one priority",
            json!({
                "FirewallRuleGroupEntries": [
                    { "Priority": 7, "InlineRuleGroup": inline_firewall_rules(1) },
                    { "Priority": 7, "InlineRuleGroup": inline_firewall_rules(1) },
                ],
            }),
            "Each entry in the firewall section must have a unique priority",
        ),
        (
            "two firewall-section entries with no priority at all",
            json!({
                "FirewallRuleGroupEntries": [
                    { "InlineRuleGroup": inline_firewall_rules(1) },
                    { "InlineRuleGroup": inline_firewall_rules(1) },
                ],
            }),
            "Each entry in the firewall section must have a unique priority",
        ),
        (
            "an IPS entry naming neither an ARN nor an embedded rule group",
            json!({ "IpsRuleGroupEntries": [{ "Override": { "Action": "DROP_TO_ALERT" } }] }),
            "Provide either ResourceArn or InlineRuleGroup",
        ),
    ];

    let harness = Harness::new();
    for (case, document, stem) in rows {
        let (status, body) = harness
            .call(
                "CreateFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": "ng-bad-entry", "FirewallPolicyV2": document }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, INVALID_REQUEST),
            "case {case}: expected the bad request refused, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.starts_with(stem),
            "case {case}: expected a message stating {stem:?}, got {message:?}"
        );

        // The refused create consumed neither the name nor any other state.
        let (status, body) = harness
            .call(
                "DescribeFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": "ng-bad-entry" }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, NOT_FOUND),
            "case {case}: expected nothing stored, got {status} {body:?}"
        );
    }
}

/// RULE: an entry that REFERENCES a rule group by ARN references an existing
/// engine rule group, so an ARN that names no engine rule group this service
/// holds is refused with the service's built reference refusal — the stem naming
/// the rule group ARN and the offending ARN in the parameter brackets.
///
/// The rows span: an ARN of a rule group that exists but is not an engine one, an
/// ARN of this service naming no stored rule group at all, and the same in the
/// IPS section.
#[tokio::test]
async fn a_reference_to_a_rule_group_that_does_not_exist_is_refused() {
    let harness = Harness::new();
    let stateful = harness
        .shared_rule_group("ng-referenced-stateful", "STATEFUL", 10)
        .await;
    let unknown =
        format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:stateful-rulegroup/ng-no-such-group");

    // (case, the document)
    let rows: [(&str, Value); 3] = [
        (
            "a stateful rule group, which is no firewall engine rule group",
            json!({
                "FirewallRuleGroupEntries": [{ "Priority": 1, "ResourceArn": stateful.clone() }],
            }),
        ),
        (
            "an ARN naming no stored rule group",
            json!({
                "FirewallRuleGroupEntries": [{ "Priority": 1, "ResourceArn": unknown.clone() }],
            }),
        ),
        (
            "the same reference in the IPS section",
            json!({ "IpsRuleGroupEntries": [{ "ResourceArn": stateful.clone() }] }),
        ),
    ];

    for (case, document) in rows {
        let expected_arn = document["FirewallRuleGroupEntries"][0]["ResourceArn"]
            .as_str()
            .or_else(|| document["IpsRuleGroupEntries"][0]["ResourceArn"].as_str())
            .unwrap_or_default()
            .to_string();
        let (status, body) = harness
            .call(
                "CreateFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": "ng-bad-reference", "FirewallPolicyV2": document }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body).as_str()),
            (
                400,
                INVALID_REQUEST,
                format!("rule group ARN does not exist, parameter: [{expected_arn}]").as_str()
            ),
            "case {case}: expected the reference refused, got {status} {body:?}"
        );
    }
}

/// RULE: every `…V2` policy operation identifies the policy it acts on by name,
/// by ARN or by both, and refuses a reference that identifies none: no
/// identifier at all, an ARN that is not one of this service's, an ARN of another
/// kind of resource, a name and an ARN naming two different policies, and a
/// well-formed reference that resolves to nothing stored.
#[tokio::test]
async fn a_reference_that_identifies_no_policy_is_refused() {
    let harness = Harness::new();
    harness
        .ok(
            "CreateFirewallPolicyV2",
            &json!({ "FirewallPolicyV2Name": "ng-resolve", "FirewallPolicyV2": json!({}) }),
        )
        .await;

    // (case, the members naming the policy, the code and message expected)
    let rows: [(&str, Value, &str, &str); 5] = [
        (
            "no identifier at all",
            json!({}),
            INVALID_REQUEST,
            "At least one of FirewallPolicyV2Name or FirewallPolicyV2Arn must be set",
        ),
        (
            "an ARN that is not an ARN",
            json!({ "FirewallPolicyV2Arn": "arn:aws:not-an-arn" }),
            INVALID_REQUEST,
            "Resource ARN has an invalid format.",
        ),
        (
            "an ARN of another kind of resource",
            json!({
                "FirewallPolicyV2Arn":
                    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/ng-resolve"),
            }),
            INVALID_REQUEST,
            "Resource Type and Arn(Type in Arn) do not match",
        ),
        (
            "a name and an ARN naming two different policies",
            json!({
                "FirewallPolicyV2Name": "ng-resolve",
                "FirewallPolicyV2Arn": policy_arn("ng-other"),
            }),
            INVALID_REQUEST,
            "Name and Arn(Name in Arn) do not match",
        ),
        (
            "a name no policy carries",
            json!({ "FirewallPolicyV2Name": "ng-unknown" }),
            NOT_FOUND,
            NOT_FOUND_MESSAGE,
        ),
    ];

    for (case, reference, code, message) in rows {
        for operation in ["DescribeFirewallPolicyV2", "DeleteFirewallPolicyV2"] {
            let (status, body) = harness.call(operation, &reference).await;
            assert_eq!(
                (status, error_code(&body).as_str(), error_message(&body).as_str()),
                (400, code, message),
                "case {case} on {operation}: expected {code} {message:?}, got {status} {body:?}"
            );
        }
    }

    // The policy survived every refused call.
    let described = harness
        .ok(
            "DescribeFirewallPolicyV2",
            &json!({ "FirewallPolicyV2Name": "ng-resolve" }),
        )
        .await;
    assert_eq!(
        described["FirewallPolicyV2Response"]["FirewallPolicyV2Name"],
        json!("ng-resolve"),
        "expected the stored policy untouched, got {described:?}"
    );
}

/// RULE: the listing pages its entries in name order, each page carrying at most
/// `MaxResults` of them and a cursor that resumes after the last one, and it
/// refuses a `NextToken` it did not issue.
///
/// The rows span page sizes below, at and above the number of stored policies.
#[tokio::test]
async fn the_listing_pages_its_entries_in_name_order() {
    let harness = Harness::new();
    let names = ["ng-page-a", "ng-page-b", "ng-page-c", "ng-page-d"];
    for name in names {
        harness
            .ok(
                "CreateFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": name, "FirewallPolicyV2": json!({}) }),
            )
            .await;
    }

    for page_size in [1usize, 2, 4, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let request = match &next {
                Some(token) => json!({ "MaxResults": page_size, "NextToken": token }),
                None => json!({ "MaxResults": page_size }),
            };
            let page = harness.ok("ListFirewallPoliciesV2", &request).await;
            let entries = page["FirewallPoliciesV2"]
                .as_array()
                .unwrap_or_else(|| panic!("expected FirewallPoliciesV2 in {page:?}"));
            assert!(
                entries.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} entries, got {page:?}"
            );
            for entry in entries {
                seen.push(entry["Name"].as_str().unwrap_or_default().to_string());
            }
            next = page["NextToken"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            names.iter().map(|name| (*name).to_string()).collect::<Vec<String>>(),
            "case page size {page_size}: expected every policy once, in name order, got {seen:?}"
        );
    }

    // A cursor another listing of this service issued is not one this listing
    // resumes.
    let foreign = harness
        .ok("ListFirewallPolicies", &json!({ "MaxResults": 1 }))
        .await;
    let foreign_token = foreign["NextToken"].as_str().unwrap_or("Zm9yZWlnbg==");
    let (status, body) = harness
        .call(
            "ListFirewallPoliciesV2",
            &json!({ "NextToken": foreign_token }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, INVALID_REQUEST),
        "expected a foreign pagination token refused, got {status} {body:?}"
    );
}

/// RULE: `RuleOutputFormat` asks for the form the rules of the policy's EMBEDDED
/// firewall engine rule groups are reported in, and an omitted one asks for the
/// form they were submitted in — so a request naming the form the stored rules
/// already carry is served with the stored document, while one that would require
/// re-rendering rules this service was never given is not served at all.
///
/// The rows span both stored forms against both formats, plus the omitted format.
#[tokio::test]
async fn an_output_format_the_stored_rules_already_carry_is_served() {
    let harness = Harness::new();
    let as_string = json!({
        "FirewallRuleGroupEntries": [{
            "Priority": 1,
            "InlineRuleGroup": inline_firewall_rules_string("pass tcp any any -> any any (sid:1;)"),
        }],
    });
    let as_rules = json!({
        "FirewallRuleGroupEntries": [{ "Priority": 1, "InlineRuleGroup": inline_firewall_rules(1) }],
    });
    for (name, document) in [("ng-fmt-string", &as_string), ("ng-fmt-rules", &as_rules)] {
        harness
            .ok(
                "CreateFirewallPolicyV2",
                &json!({ "FirewallPolicyV2Name": name, "FirewallPolicyV2": document }),
            )
            .await;
    }

    // (case, the policy, the requested format, whether it is served)
    let rows: [(&str, &str, Option<&str>, bool); 6] = [
        ("a rules string, no format asked for", "ng-fmt-string", None, true),
        (
            "a rules string, asked for as a rules string",
            "ng-fmt-string",
            Some("RULES_STRING"),
            true,
        ),
        (
            "a rules string, asked for as structured rules",
            "ng-fmt-string",
            Some("FIREWALL_RULES"),
            false,
        ),
        ("structured rules, no format asked for", "ng-fmt-rules", None, true),
        (
            "structured rules, asked for as structured rules",
            "ng-fmt-rules",
            Some("FIREWALL_RULES"),
            true,
        ),
        (
            "structured rules, asked for as a rules string",
            "ng-fmt-rules",
            Some("RULES_STRING"),
            false,
        ),
    ];

    for (case, name, format, served) in rows {
        let request = match format {
            Some(format) => {
                json!({ "FirewallPolicyV2Name": name, "RuleOutputFormat": format })
            }
            None => json!({ "FirewallPolicyV2Name": name }),
        };
        let (status, body) = harness.call("DescribeFirewallPolicyV2", &request).await;
        if served {
            let expected = if name == "ng-fmt-string" {
                &as_string
            } else {
                &as_rules
            };
            assert_eq!(
                (status, &body["FirewallPolicyV2"]),
                (200, expected),
                "case {case}: expected the stored document, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 501,
                "case {case}: expected the conversion to be reported as not implemented, got \
                 {status} {body:?}"
            );
        }
    }
}
