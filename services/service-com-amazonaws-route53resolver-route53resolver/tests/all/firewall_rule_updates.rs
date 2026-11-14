// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateFirewallRule`: the update of ONE DNS Firewall
//! rule, whose refusal is the modelled exception that states it rather than an
//! entry of a response body.
//!
//! Two rules are asserted here. The first is the operation's own: a call names
//! one rule of a rule group by the source that rule matches on, replaces
//! exactly the members it states and leaves every other member as stored, and
//! the rule it answers is the rule a later read reports.
//!
//! The second is the one this suite spans a table over: the block settings —
//! `BlockResponse` and the three `BlockOverride*` members — state how a
//! BLOCKING rule answers a query it matched, so a write that states one for a
//! rule whose action lets the query through (`ALLOW` or `ALERT`) is refused
//! with the service's coded `ValidationException`, whether the action comes
//! from the request or from the stored rule, and whether the write is a create,
//! a single update or one entry of a batch update.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53resolver_route53resolver::state::RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_FOR_ACTION;

use super::harness::{call, emulator, error_type, ok, parse_time, string_member};

/// Create one rule group and answer the id Route 53 Resolver minted for it.
async fn rule_group(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": format!("group-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallRuleGroup")
            .expect("the create answers with the FirewallRuleGroup"),
        "Id",
    )
}

/// Create one domain list and answer the id Route 53 Resolver minted for it.
async fn domain_list(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("list-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers with the FirewallDomainList"),
        "Id",
    )
}

/// Create one rule from the members `members` states and answer it as the
/// create reported it.
async fn create_rule(router: &MultiServiceRouter, name: &str, members: Value) -> Value {
    let mut request = members;
    let object = request
        .as_object_mut()
        .expect("a rule's members are stated as an object");
    object.insert("Name".to_string(), json!(name));
    object.insert("CreatorRequestId".to_string(), json!(format!("rule-{name}")));
    let created = ok(router, "CreateFirewallRule", request).await;
    created
        .get("FirewallRule")
        .cloned()
        .unwrap_or_else(|| panic!("the create of {name:?} answers with the FirewallRule"))
}

/// The rules the named group holds, as `ListFirewallRules` reports them — the
/// reader every write in this suite is proved through.
async fn listed_rules(router: &MultiServiceRouter, group_id: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group_id }),
    )
    .await;
    listed
        .get("FirewallRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one rule the group `group_id` holds, as `ListFirewallRules` reports it.
async fn listed_rule(router: &MultiServiceRouter, case: &str, group_id: &str) -> Value {
    let rules = listed_rules(router, group_id).await;
    rules
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the group must hold its one rule, got {rules:?}"))
}

/// The whole `Message` the service answers a block setting stated for an action
/// that lets the query through with.
///
/// Entirely fixed text: the sentence names neither the option the request
/// stated nor the action it stated it with, so one expectation serves every row
/// of every table below.
fn block_option_refusal() -> String {
    format!(
        "[{RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_FOR_ACTION}] You can't provide a block \
         option if the ACTION is ALLOW or ALERT."
    )
}

/// Assert one call was refused as a block setting stated for a non-blocking
/// action is: HTTP 400, `ValidationException`, and that one sentence.
async fn expect_block_option_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    case: &str,
    body: Value,
) {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {answered}"
    );
    assert!(
        error_type(&answered).contains("ValidationException"),
        "case {case:?}: {operation} must answer ValidationException, got {answered}"
    );
    assert_eq!(
        string_member(&answered, "Message"),
        block_option_refusal(),
        "case {case:?}: the refusal must state the service's sentence for it, got {answered}"
    );
}

/// One row of the block-settings table: the rule the row starts from, the
/// members the update states, and whether the service admits it.
struct BlockOptionCase {
    case: &'static str,
    /// What the rule is created with, beside its group and its domain list.
    created: Value,
    /// What the update states, beside the group and the domain list that name
    /// the rule.
    stated: Value,
    /// Whether the update must be applied, as against refused.
    admitted: bool,
}

/// A block setting is refused for a rule whose action lets the query through,
/// and admitted for one that blocks.
///
/// The table spans what the rule generalises over: which of the four block
/// settings the request states (each one alone, and all four at once), whether
/// the non-blocking action comes from the request or from the stored rule,
/// which of the two non-blocking actions it is, and — as the rows the service
/// must still admit — the same settings stated for a rule that blocks, the
/// action moving ONTO `BLOCK` with the response it then requires, and the
/// action moving OFF `BLOCK` with no block setting stated at all.
#[tokio::test]
async fn a_block_setting_belongs_to_the_action_that_blocks() {
    let override_members = json!({
        "BlockResponse": "OVERRIDE",
        "BlockOverrideDomain": "override.example.test",
        "BlockOverrideDnsType": "CNAME",
        "BlockOverrideTtl": 60,
    });

    let cases = [
        BlockOptionCase {
            case: "ALLOW stated with the block response",
            created: json!({ "Action": "BLOCK", "BlockResponse": "NXDOMAIN" }),
            stated: json!({ "Action": "ALLOW", "BlockResponse": "NODATA" }),
            admitted: false,
        },
        BlockOptionCase {
            case: "ALERT stated with the block response",
            created: json!({ "Action": "BLOCK", "BlockResponse": "NODATA" }),
            stated: json!({ "Action": "ALERT", "BlockResponse": "NXDOMAIN" }),
            admitted: false,
        },
        BlockOptionCase {
            case: "the override domain on a rule that alerts",
            created: json!({ "Action": "ALERT" }),
            stated: json!({ "BlockOverrideDomain": "override.example.test" }),
            admitted: false,
        },
        BlockOptionCase {
            case: "the override record type on a rule that allows",
            created: json!({ "Action": "ALLOW" }),
            stated: json!({ "BlockOverrideDnsType": "CNAME" }),
            admitted: false,
        },
        BlockOptionCase {
            case: "the override lifetime on a rule that alerts",
            created: json!({ "Action": "ALERT" }),
            stated: json!({ "BlockOverrideTtl": 120 }),
            admitted: false,
        },
        BlockOptionCase {
            case: "every block setting at once with ALLOW",
            created: json!({ "Action": "BLOCK", "BlockResponse": "NODATA" }),
            stated: {
                let mut stated = override_members.clone();
                stated
                    .as_object_mut()
                    .expect("the override members are an object")
                    .insert("Action".to_string(), json!("ALLOW"));
                stated
            },
            admitted: false,
        },
        BlockOptionCase {
            case: "the block response of a rule that blocks",
            created: json!({ "Action": "BLOCK", "BlockResponse": "NXDOMAIN" }),
            stated: json!({ "BlockResponse": "NODATA" }),
            admitted: true,
        },
        BlockOptionCase {
            case: "BLOCK stated with the block response it requires",
            created: json!({ "Action": "ALERT" }),
            stated: json!({ "Action": "BLOCK", "BlockResponse": "NXDOMAIN" }),
            admitted: true,
        },
        BlockOptionCase {
            case: "the action moving off BLOCK with no block setting stated",
            created: json!({ "Action": "BLOCK", "BlockResponse": "NXDOMAIN" }),
            stated: json!({ "Action": "ALERT" }),
            admitted: true,
        },
        BlockOptionCase {
            case: "the override lifetime of a rule that blocks",
            created: {
                let mut created = override_members.clone();
                created
                    .as_object_mut()
                    .expect("the override members are an object")
                    .insert("Action".to_string(), json!("BLOCK"));
                created
            },
            stated: json!({ "BlockOverrideTtl": 300 }),
            admitted: true,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        // Each row gets its own group and its own domain list, so the rule it
        // starts from is the one rule its group holds and nothing of the row
        // before it can name it.
        let router = emulator();
        let group = rule_group(&router, &format!("block-option-group-{index}")).await;
        let list = domain_list(&router, &format!("block-option-list-{index}")).await;

        let mut created = row.created.clone();
        let members = created
            .as_object_mut()
            .expect("a rule's members are stated as an object");
        members.insert("FirewallRuleGroupId".to_string(), json!(group));
        members.insert("FirewallDomainListId".to_string(), json!(list));
        members.insert("Priority".to_string(), json!(100));
        create_rule(&router, &format!("block-option-rule-{index}"), created).await;
        let before = listed_rule(&router, case, &group).await;

        let mut request = row.stated.clone();
        let members = request
            .as_object_mut()
            .expect("an update's members are stated as an object");
        members.insert("FirewallRuleGroupId".to_string(), json!(group));
        members.insert("FirewallDomainListId".to_string(), json!(list));

        if !row.admitted {
            expect_block_option_refusal(&router, "UpdateFirewallRule", case, request.clone()).await;
            assert_eq!(
                listed_rule(&router, case, &group).await,
                before,
                "case {case:?}: a refused update must write nothing at all"
            );

            // The same settings stated to the batch update are refused on the
            // same grounds, reported beside the entry that caused them.
            let body = ok(
                &router,
                "BatchUpdateFirewallRule",
                json!({ "UpdateFirewallRuleEntries": [request.clone()] }),
            )
            .await;
            let errors = body
                .get("UpdateErrors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                errors.len(),
                1,
                "case {case:?}: the batch must refuse the one entry it carries, got {body}"
            );
            assert_eq!(
                string_member(&errors[0], "Code"),
                "ValidationException",
                "case {case:?}: the entry's error must carry the exception's code, got {body}"
            );
            assert_eq!(
                string_member(&errors[0], "Message"),
                block_option_refusal(),
                "case {case:?}: the entry's error must state the same sentence, got {body}"
            );
            continue;
        }

        let answered = ok(&router, "UpdateFirewallRule", request).await;
        let answered = answered.get("FirewallRule").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the update must answer with the FirewallRule, got {answered}")
        });
        for (member, value) in row
            .stated
            .as_object()
            .expect("an update's members are stated as an object")
        {
            assert_eq!(
                answered.get(member),
                Some(value),
                "case {case:?}: {member} must become {value}, got {answered}"
            );
        }
        assert_eq!(
            listed_rule(&router, case, &group).await,
            answered,
            "case {case:?}: a later read must report the rule the update answered"
        );
    }
}

/// The same rule holds on the create path: a rule cannot be MADE with a block
/// setting for an action that lets the query through.
///
/// The table spans the two non-blocking actions against each of the four block
/// settings, with the create that states the action and no block setting at all
/// as the row the service admits.
#[tokio::test]
async fn a_create_cannot_state_a_block_setting_for_a_non_blocking_action() {
    let router = emulator();
    let group = rule_group(&router, "create-block-option-group").await;

    let mut priority = 100;
    for action in ["ALLOW", "ALERT"] {
        // One domain list per action, so the create each action's rows admit
        // names a source no rule of the group matches yet.
        let list = domain_list(&router, &format!("create-block-option-list-{action}")).await;
        let held = listed_rules(&router, &group).await.len();
        for (option, value) in [
            ("BlockResponse", json!("NODATA")),
            ("BlockOverrideDomain", json!("override.example.test")),
            ("BlockOverrideDnsType", json!("CNAME")),
            ("BlockOverrideTtl", json!(60)),
        ] {
            let case = format!("{action} with {option}");
            priority += 10;
            expect_block_option_refusal(
                &router,
                "CreateFirewallRule",
                &case,
                json!({
                    "CreatorRequestId": format!("create-{action}-{option}"),
                    "FirewallRuleGroupId": group,
                    "FirewallDomainListId": list,
                    "Name": format!("create-{action}-{option}"),
                    "Priority": priority,
                    "Action": action,
                    option.to_string(): value,
                }),
            )
            .await;
            assert_eq!(
                listed_rules(&router, &group).await.len(),
                held,
                "case {case:?}: a refused create must add no rule to the group"
            );
        }

        // The same action with no block setting at all is admitted, so what the
        // rule refuses is the block setting and not the action.
        priority += 10;
        let created = create_rule(
            &router,
            &format!("create-{action}"),
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": priority,
                "Action": action,
            }),
        )
        .await;
        assert_eq!(
            created.get("Action"),
            Some(&json!(action)),
            "case {action:?}: a rule that states no block setting must be created, got {created}"
        );
    }
}

/// An update replaces exactly the members it states: every other member of the
/// rule is the one it was stored with, and the rule the call answers is the rule
/// a later read reports.
///
/// The table spans the members one call may change — a number, a name, an enum
/// value, a query type, a block response and the override settings that go with
/// it — one at a time, each row starting from the rule the row before it left.
#[tokio::test]
async fn an_update_changes_only_the_members_it_states() {
    let router = emulator();
    let group = rule_group(&router, "update-members-group").await;
    let list = domain_list(&router, "update-members-list").await;

    create_rule(
        &router,
        "member-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Priority": 100,
            "Action": "BLOCK",
            "BlockResponse": "OVERRIDE",
            "BlockOverrideDomain": "example.test",
            "BlockOverrideDnsType": "CNAME",
            "BlockOverrideTtl": 60,
            "Qtype": "A",
            "FirewallDomainRedirectionAction": "INSPECT_REDIRECTION_DOMAIN",
        }),
    )
    .await;

    // Each row is (the member, the value the call states for it). The members
    // and their admissible values are the ones `UpdateFirewallRuleRequest`
    // declares
    // (`smithy_query(query="operation",operation="UpdateFirewallRule")`).
    let rows = [
        ("Priority", json!(250)),
        ("Name", json!("member-rule-renamed")),
        ("BlockOverrideTtl", json!(120)),
        // The override target is a DNS name the rule reports canonical, so the
        // row states the canonical spelling: the write is idempotent over it.
        ("BlockOverrideDomain", json!("other.example.test.")),
        ("Qtype", json!("AAAA")),
        (
            "FirewallDomainRedirectionAction",
            json!("TRUST_REDIRECTION_DOMAIN"),
        ),
        ("BlockResponse", json!("NXDOMAIN")),
    ];

    let mut stored = listed_rule(&router, "the created rule", &group).await;

    for (member, value) in &rows {
        let case = format!("a call stating {member}");
        let answered = ok(
            &router,
            "UpdateFirewallRule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                member.to_string(): value,
            }),
        )
        .await;
        let answered = answered.get("FirewallRule").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the update must answer with the FirewallRule, got {answered}")
        });
        assert_eq!(
            answered.get(*member),
            Some(value),
            "case {case:?}: expected {member} to become {value}, got {answered}"
        );

        let before = stored
            .as_object()
            .expect("a rule is reported as an object")
            .clone();
        for (other, was) in &before {
            if other == member {
                continue;
            }
            if other == "ModificationTime" {
                let moved = parse_time(&case, "ModificationTime", &string_member(&answered, other));
                let had = parse_time(&case, "ModificationTime", was.as_str().unwrap_or_default());
                assert!(
                    moved >= had,
                    "case {case:?}: ModificationTime must not go backwards, was {was} \
                     and is now {moved:?}"
                );
                continue;
            }
            assert_eq!(
                answered.get(other),
                Some(was),
                "case {case:?}: {other} must stay {was}, got {answered}"
            );
        }

        // The rule the call answered is the rule the store now holds.
        let read_back = listed_rule(&router, &case, &group).await;
        assert_eq!(
            read_back, answered,
            "case {case:?}: a later read must report the rule the update answered, \
             got {read_back}"
        );
        stored = read_back;
    }
}

/// A call that names no single stored rule is refused, and the refusal is the
/// modelled exception that states it — this call carries ONE rule, so there is
/// no response body to report it in.
///
/// The table spans the ways naming can fail: an unresolvable rule group, a
/// match source no rule of the group matches, both match-source ids at once,
/// and neither of them.
#[tokio::test]
async fn a_call_that_names_no_single_rule_is_refused() {
    let router = emulator();
    let group = rule_group(&router, "update-naming-group").await;
    let list = domain_list(&router, "update-naming-list").await;
    let detector = create_rule(
        &router,
        "naming-detector",
        json!({
            "FirewallRuleGroupId": group,
            "DnsThreatProtection": "DGA",
            "ConfidenceThreshold": "HIGH",
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;
    let detector_id = string_member(&detector, "FirewallThreatProtectionId");
    create_rule(
        &router,
        "naming-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Priority": 200,
            "Action": "ALERT",
        }),
    )
    .await;

    for (case, body, expected) in [
        (
            "an unresolvable rule group",
            json!({
                "FirewallRuleGroupId": "rslvr-frg-ffffffffffffffff",
                "FirewallDomainListId": list,
                "Priority": 300,
            }),
            "ResourceNotFoundException",
        ),
        (
            "a match source no rule of the group matches",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": "rslvr-fdl-ffffffffffffffff",
                "Priority": 300,
            }),
            "ResourceNotFoundException",
        ),
        (
            "both match source ids at once",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "FirewallThreatProtectionId": detector_id,
                "Priority": 300,
            }),
            "ValidationException",
        ),
        (
            "neither match source id",
            json!({ "FirewallRuleGroupId": group, "Priority": 300 }),
            "ValidationException",
        ),
        (
            "a priority another rule of the group holds",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 100,
            }),
            "ValidationException",
        ),
    ] {
        let (status, answered) = call(&router, "UpdateFirewallRule", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the call must answer HTTP 400, got {status}: {answered}"
        );
        assert!(
            error_type(&answered).contains(expected),
            "case {case:?}: expected {expected}, got {answered}"
        );
        assert!(
            !string_member(&answered, "Message").is_empty(),
            "case {case:?}: the refusal must state why, got {answered}"
        );
    }
}
