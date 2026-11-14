// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS Firewall rule slice (`CreateFirewallRule`,
//! `ListFirewallRules`) and the rule-group association slice
//! (`AssociateFirewallRuleGroup`, `GetFirewallRuleGroupAssociation`,
//! `ListFirewallRuleGroupAssociations`, `UpdateFirewallRuleGroupAssociation`,
//! `DisassociateFirewallRuleGroup`).
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53resolver_route53resolver::state::{
    ASSOCIATION_PRIORITY_RANGE, MANAGED_FIREWALL_DOMAIN_LISTS, PROVISIONING_DELAY,
};

use super::harness::{call, emulator, error_type, expect_error, ok, string_member, ACCOUNT, REGION};

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

/// The `FirewallRule` of a create response.
fn rule_of(body: &Value) -> &Value {
    body.get("FirewallRule")
        .expect("the response carries a FirewallRule")
}

/// The `FirewallRuleGroupAssociation` of an associate/get/update response.
fn association_of(body: &Value) -> &Value {
    body.get("FirewallRuleGroupAssociation")
        .expect("the response carries a FirewallRuleGroupAssociation")
}

/// The rules the named group holds, as `ListFirewallRules` reports them.
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

/// One row of the rule table: the members the declaration carries, which the
/// created rule must report back.
struct RuleCase {
    case: &'static str,
    /// What the request adds beside `CreatorRequestId`, `FirewallRuleGroupId`,
    /// `Name` and `Priority`.
    members: Value,
    priority: i64,
}

/// A rule created in a group is persisted with the members the request carried,
/// and every operation that reports the group's rules reports the same ones:
/// `ListFirewallRules` answers them in the order DNS Firewall processes them (by
/// priority, lowest first) and the group's own `RuleCount` counts exactly them.
///
/// The table spans what the rule generalises over: each `Action` the model's enum
/// carries (`ALLOW`, `ALERT`, `BLOCK`), each `BlockResponse` a blocking rule may
/// state (`NODATA`, `NXDOMAIN`, `OVERRIDE` with the three `BlockOverride*`
/// members its doc requires), a `Qtype`, each
/// `FirewallDomainRedirectionAction`, a domain list this account created and one
/// DNS Firewall publishes itself, and priorities out of ascending order so the
/// reported order is the rule's and not the creation order.
#[tokio::test]
async fn created_rules_are_listed_in_the_order_dns_firewall_processes_them() {
    let router = emulator();
    let group = rule_group(&router, "rule-table-group").await;
    let own_list = domain_list(&router, "rule-table-list").await;
    // The first list DNS Firewall publishes to every account, which a rule may
    // name exactly as it names one of this account's own.
    let managed_list = MANAGED_FIREWALL_DOMAIN_LISTS[0].id();

    let cases = vec![
        RuleCase {
            case: "BLOCK with NXDOMAIN",
            priority: 300,
            members: json!({
                "FirewallDomainListId": own_list,
                "Action": "BLOCK",
                "BlockResponse": "NXDOMAIN",
            }),
        },
        RuleCase {
            case: "BLOCK with NODATA and a Qtype",
            priority: 100,
            members: json!({
                "FirewallDomainListId": own_list,
                "Action": "BLOCK",
                "BlockResponse": "NODATA",
                "Qtype": "AAAA",
            }),
        },
        RuleCase {
            case: "BLOCK with an OVERRIDE record",
            priority: 200,
            members: json!({
                "FirewallDomainListId": own_list,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "safe.example.com.",
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            }),
        },
        RuleCase {
            case: "ALERT on a published managed domain list",
            priority: 500,
            members: json!({
                "FirewallDomainListId": managed_list,
                "Action": "ALERT",
            }),
        },
        RuleCase {
            case: "ALLOW with the redirection chain trusted",
            priority: 400,
            members: json!({
                "FirewallDomainListId": own_list,
                "Action": "ALLOW",
                "FirewallDomainRedirectionAction": "TRUST_REDIRECTION_DOMAIN",
            }),
        },
        RuleCase {
            case: "ALLOW with the redirection chain inspected",
            priority: 600,
            members: json!({
                "FirewallDomainListId": own_list,
                "Action": "ALLOW",
                "FirewallDomainRedirectionAction": "INSPECT_REDIRECTION_DOMAIN",
            }),
        },
    ];

    let mut declared: Vec<(&str, i64, Value)> = Vec::new();
    for RuleCase {
        case,
        members,
        priority,
    } in cases
    {
        let mut body = json!({
            "CreatorRequestId": format!("rule-{priority}"),
            "FirewallRuleGroupId": group,
            "Name": format!("rule-{priority}"),
            "Priority": priority,
        });
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in members
            .as_object()
            .expect("the case's members are an object")
            .clone()
        {
            object.insert(member, value);
        }

        let created = ok(&router, "CreateFirewallRule", body.clone()).await;
        let made = rule_of(&created).clone();

        assert_eq!(
            made.get("FirewallRuleGroupId").and_then(Value::as_str),
            Some(group.as_str()),
            "case {case:?}: the rule belongs to the group it was created in, got {made}"
        );
        assert_eq!(
            made.get("Priority").and_then(Value::as_i64),
            Some(priority),
            "case {case:?}: the rule reports the Priority it was created with, got {made}"
        );
        for (member, value) in members
            .as_object()
            .expect("the case's members are an object")
        {
            assert_eq!(
                made.get(member),
                Some(value),
                "case {case:?}: {member} must be reported as declared, got {made}"
            );
        }
        assert!(
            made.get("CreationTime").and_then(Value::as_str).is_some()
                && made
                    .get("ModificationTime")
                    .and_then(Value::as_str)
                    .is_some(),
            "case {case:?}: a created rule reports its creation and modification times, got {made}"
        );
        assert_eq!(
            made.get("CreatorRequestId").and_then(Value::as_str),
            Some(format!("rule-{priority}").as_str()),
            "case {case:?}: the rule echoes its CreatorRequestId, got {made}"
        );
        // A rule matching a domain list needs no asynchronous provisioning, so
        // the model lets its lifecycle state be absent rather than claiming one.
        assert_eq!(
            made.get("Status"),
            None,
            "case {case:?}: a domain-list rule reports no Status, got {made}"
        );

        declared.push((case, priority, made));
    }

    let listed = listed_rules(&router, &group).await;
    let order: Vec<i64> = listed
        .iter()
        .filter_map(|rule| rule.get("Priority").and_then(Value::as_i64))
        .collect();
    let mut ascending = order.clone();
    ascending.sort_unstable();
    assert_eq!(
        order, ascending,
        "the rules must be listed by priority, lowest first, got {order:?}"
    );
    assert_eq!(
        listed.len(),
        declared.len(),
        "every created rule must be listed, got {listed:?}"
    );
    for (case, priority, made) in &declared {
        let found = listed
            .iter()
            .find(|rule| rule.get("Priority").and_then(Value::as_i64) == Some(*priority))
            .unwrap_or_else(|| panic!("case {case:?}: the rule must be listed, got {listed:?}"));
        assert_eq!(
            found, made,
            "case {case:?}: the listed rule must be the created one, got {found}"
        );
    }

    let read_group = ok(
        &router,
        "GetFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": group }),
    )
    .await;
    assert_eq!(
        read_group
            .get("FirewallRuleGroup")
            .and_then(|group| group.get("RuleCount"))
            .and_then(Value::as_i64),
        Some(declared.len() as i64),
        "the group's RuleCount must count the rules it holds, got {read_group}"
    );

    // The page is cut out of the same sequence, so a cursor walks the rules once
    // and reports each exactly once.
    let mut walked: Vec<i64> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({ "FirewallRuleGroupId": group, "MaxResults": 2 });
        if let Some(token) = &next {
            request
                .as_object_mut()
                .expect("the request body is an object")
                .insert("NextToken".to_string(), json!(token));
        }
        let page = ok(&router, "ListFirewallRules", request).await;
        walked.extend(
            page.get("FirewallRules")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|rule| rule.get("Priority").and_then(Value::as_i64)),
        );
        next = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, ascending,
        "paging must walk the rules once, in priority order, got {walked:?}"
    );

    // A filter narrows the same collection rather than reporting another one.
    let blocking = ok(
        &router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group, "Action": "BLOCK" }),
    )
    .await;
    let blocking = blocking
        .get("FirewallRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        blocking.len() == 3
            && blocking
                .iter()
                .all(|rule| rule.get("Action").and_then(Value::as_str) == Some("BLOCK")),
        "the Action filter must report the blocking rules alone, got {blocking:?}"
    );
    let by_priority = ok(
        &router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group, "Priority": 200 }),
    )
    .await;
    let by_priority = by_priority
        .get("FirewallRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        by_priority.len() == 1
            && by_priority[0].get("Priority").and_then(Value::as_i64) == Some(200),
        "the Priority filter must report the one rule holding it, got {by_priority:?}"
    );
}

/// One row of the refusal table: a declaration the service does not admit.
struct RefusalCase {
    case: &'static str,
    /// The members the request carries beside `CreatorRequestId`.
    body: Value,
    expected: &'static str,
}

/// A rule whose match source or whose blocking details are incomplete is refused
/// — and nothing is written, so the group still holds only the rules it held.
///
/// The table spans the rules the model and the operation's own doc state: exactly
/// one match source of the three; `BlockResponse` for a blocking rule; each
/// `BlockOverride*` member for an overriding one; a unique priority per group;
/// `ALLOW` barred for a DNS Firewall Advanced rule; the confidence threshold a
/// `DnsThreatProtection` rule needs; and a group or domain list the account
/// cannot address.
#[tokio::test]
async fn an_incomplete_rule_declaration_is_refused() {
    let router = emulator();
    let group = rule_group(&router, "refusal-group").await;
    let list = domain_list(&router, "refusal-list").await;

    // One rule the refusals are judged against: the store must still hold
    // exactly this one when they are done.
    ok(
        &router,
        "CreateFirewallRule",
        json!({
            "CreatorRequestId": "standing-rule",
            "FirewallRuleGroupId": group,
            "Name": "standing-rule",
            "Priority": 100,
            "FirewallDomainListId": list,
            "Action": "ALERT",
        }),
    )
    .await;

    let cases = vec![
        RefusalCase {
            case: "no match source at all",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "no-source",
                "Priority": 200,
                "Action": "ALERT",
            }),
        },
        RefusalCase {
            case: "two match sources at once",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "two-sources",
                "Priority": 200,
                "FirewallDomainListId": list,
                "DnsThreatProtection": "DGA",
                "ConfidenceThreshold": "LOW",
                "Action": "ALERT",
            }),
        },
        RefusalCase {
            case: "BLOCK without a BlockResponse",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "block-without-response",
                "Priority": 200,
                "FirewallDomainListId": list,
                "Action": "BLOCK",
            }),
        },
        RefusalCase {
            case: "OVERRIDE without the override domain",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "override-without-domain",
                "Priority": 200,
                "FirewallDomainListId": list,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            }),
        },
        RefusalCase {
            case: "OVERRIDE without the record type",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "override-without-type",
                "Priority": 200,
                "FirewallDomainListId": list,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "safe.example.com",
                "BlockOverrideTtl": 60,
            }),
        },
        RefusalCase {
            case: "OVERRIDE without the cache lifetime",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "override-without-ttl",
                "Priority": 200,
                "FirewallDomainListId": list,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "safe.example.com",
                "BlockOverrideDnsType": "CNAME",
            }),
        },
        RefusalCase {
            case: "a priority another rule of the group already holds",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "duplicate-priority",
                "Priority": 100,
                "FirewallDomainListId": list,
                "Action": "ALERT",
            }),
        },
        RefusalCase {
            case: "ALLOW on a DNS Firewall Advanced rule",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "advanced-allow",
                "Priority": 200,
                "DnsThreatProtection": "DNS_TUNNELING",
                "ConfidenceThreshold": "HIGH",
                "Action": "ALLOW",
            }),
        },
        RefusalCase {
            case: "a DnsThreatProtection rule without its confidence threshold",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "advanced-without-threshold",
                "Priority": 200,
                "DnsThreatProtection": "DICTIONARY_DGA",
                "Action": "ALERT",
            }),
        },
        RefusalCase {
            case: "a name made of digits alone, which the pattern excludes",
            expected: "ValidationException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "12345",
                "Priority": 200,
                "FirewallDomainListId": list,
                "Action": "ALERT",
            }),
        },
        RefusalCase {
            case: "a rule group this account cannot address",
            expected: "ResourceNotFoundException",
            body: json!({
                "FirewallRuleGroupId": "rslvr-frg-0123456789abcdef",
                "Name": "unknown-group",
                "Priority": 200,
                "FirewallDomainListId": list,
                "Action": "ALERT",
            }),
        },
        RefusalCase {
            case: "a domain list this account cannot address",
            expected: "ResourceNotFoundException",
            body: json!({
                "FirewallRuleGroupId": group,
                "Name": "unknown-list",
                "Priority": 200,
                "FirewallDomainListId": "rslvr-fdl-0123456789abcdef",
                "Action": "ALERT",
            }),
        },
    ];

    for RefusalCase {
        case,
        mut body,
        expected,
    } in cases
    {
        body.as_object_mut()
            .expect("the request body is an object")
            .insert(
                "CreatorRequestId".to_string(),
                json!(format!("refused-{case}")),
            );
        expect_error(&router, "CreateFirewallRule", body, case, expected).await;

        let held = listed_rules(&router, &group).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: a refused create must write nothing, got {held:?}"
        );
    }
}

/// A repeated `CreatorRequestId` resolves to the rule it already made rather than
/// adding a second one — which is what that member exists for.
#[tokio::test]
async fn a_repeated_rule_request_id_resolves_to_the_rule_it_made() {
    let router = emulator();
    let group = rule_group(&router, "retry-group").await;
    let list = domain_list(&router, "retry-list").await;
    let body = json!({
        "CreatorRequestId": "retried-once",
        "FirewallRuleGroupId": group,
        "Name": "retried-rule",
        "Priority": 150,
        "FirewallDomainListId": list,
        "Action": "BLOCK",
        "BlockResponse": "NODATA",
    });

    let first = ok(&router, "CreateFirewallRule", body.clone()).await;
    let again = ok(&router, "CreateFirewallRule", body).await;
    assert_eq!(
        rule_of(&first),
        rule_of(&again),
        "a retried create must answer with the rule the first one made, got {again}"
    );
    assert_eq!(
        listed_rules(&router, &group).await.len(),
        1,
        "a retried create must not add a second rule"
    );
}

/// An association is a resource of its own: `AssociateFirewallRuleGroup` mints
/// the id every other operation resolves it by, it reports the transient status
/// until the move the clock owes falls due, and the tags the request carried are
/// the ones its ARN reports.
#[tokio::test]
async fn an_association_is_read_back_by_the_id_the_service_minted() {
    let router = emulator();
    let group = rule_group(&router, "assoc-group").await;
    router.hold_vpcs(["vpc-0123456789abcdef0"]);

    let created = ok(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "assoc-one",
            "FirewallRuleGroupId": group,
            "VpcId": "vpc-0123456789abcdef0",
            "Priority": 101,
            "Name": "filtering for the app VPC",
            "MutationProtection": "DISABLED",
            "Tags": [{ "Key": "env", "Value": "prod" }],
        }),
    )
    .await;
    let made = association_of(&created).clone();
    let id = string_member(&made, "Id");

    assert!(
        id.starts_with("rslvr-frgassoc-") && id.len() <= 64,
        "an association id is minted as rslvr-frgassoc-<hex> within ResourceId's length, got {id:?}"
    );
    assert_eq!(
        made.get("Arn").and_then(Value::as_str),
        Some(
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-rule-group-association/{id}"
            )
            .as_str()
        ),
        "the association's ARN is rooted in the region the request was signed for, got {made}"
    );
    assert_eq!(
        made.get("Status").and_then(Value::as_str),
        Some("UPDATING"),
        "an association the service is still putting in place reports UPDATING, got {made}"
    );
    assert_eq!(
        made.get("Priority").and_then(Value::as_i64),
        Some(101),
        "the association reports the Priority it was made with, got {made}"
    );
    assert_eq!(
        made.get("FirewallRuleGroupId").and_then(Value::as_str),
        Some(group.as_str()),
        "the association names the rule group it joins, got {made}"
    );
    assert_eq!(
        made.get("VpcId").and_then(Value::as_str),
        Some("vpc-0123456789abcdef0"),
        "the association names the VPC it joins, got {made}"
    );
    assert_eq!(
        made.get("ManagedOwnerName"),
        None,
        "an association this account made reports no ManagedOwnerName, got {made}"
    );

    // A retry of the same request id resolves to the same association.
    let retried = ok(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "assoc-one",
            "FirewallRuleGroupId": group,
            "VpcId": "vpc-0123456789abcdef0",
            "Priority": 101,
            "Name": "filtering for the app VPC",
        }),
    )
    .await;
    assert_eq!(
        string_member(association_of(&retried), "Id"),
        id,
        "a retried associate must answer with the association the first one made, got {retried}"
    );

    // The transition the clock owes is due after the provisioning span, and the
    // read is what reports it — the record itself is not moved by being read.
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
    let read = ok(
        &router,
        "GetFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": id }),
    )
    .await;
    let settled = association_of(&read).clone();
    assert_eq!(
        settled.get("Status").and_then(Value::as_str),
        Some("COMPLETE"),
        "once the clock owes it, the association reports COMPLETE, got {settled}"
    );
    assert_eq!(
        settled.get("Id"),
        made.get("Id"),
        "the read must answer the same association, got {settled}"
    );
    assert_eq!(
        settled.get("Arn"),
        made.get("Arn"),
        "a read must not change the association's identity, got {settled}"
    );

    let tags = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceArn": string_member(&made, "Arn") }),
    )
    .await;
    assert_eq!(
        tags.get("Tags"),
        Some(&json!([{ "Key": "env", "Value": "prod" }])),
        "the association reports the tags it was made with, got {tags}"
    );

    // Every filter narrows the one collection the list pages.
    for (case, request, expected) in [
        (
            "by rule group",
            json!({ "FirewallRuleGroupId": group }),
            true,
        ),
        (
            "by VPC",
            json!({ "VpcId": "vpc-0123456789abcdef0" }),
            true,
        ),
        ("by priority", json!({ "Priority": 101 }), true),
        ("by status", json!({ "Status": "COMPLETE" }), true),
        ("by another VPC", json!({ "VpcId": "vpc-other" }), false),
        ("by another priority", json!({ "Priority": 999 }), false),
        ("by a transient status", json!({ "Status": "DELETING" }), false),
    ] {
        let listed = ok(&router, "ListFirewallRuleGroupAssociations", request).await;
        let found = listed
            .get("FirewallRuleGroupAssociations")
            .and_then(Value::as_array)
            .map(|associations| {
                associations
                    .iter()
                    .any(|association| string_member(association, "Id") == id)
            })
            .unwrap_or(false);
        assert_eq!(
            found, expected,
            "case {case:?}: the filter must select the association exactly when it matches, got {listed}"
        );
    }
}

/// An association's `Priority` is held to the values its doc publishes, and to
/// one per VPC.
///
/// The table walks both published bounds, a value inside them, and one past each
/// bound; then the uniqueness rule, which is per VPC — the same priority is free
/// in another VPC.
#[tokio::test]
async fn an_association_priority_is_held_to_its_published_range() {
    let router = emulator();
    let lowest = *ASSOCIATION_PRIORITY_RANGE.start();
    let highest = *ASSOCIATION_PRIORITY_RANGE.end();

    for (case, priority, admitted) in [
        ("the lowest published value", lowest, true),
        ("a value inside the range", (lowest + highest) / 2, true),
        ("the highest published value", highest, true),
        ("one below the lowest", lowest - 1, false),
        ("one above the highest", highest + 1, false),
        ("zero, far below the range", 0, false),
    ] {
        let group = rule_group(&router, &format!("priority-group-{priority}")).await;
        let vpc = format!("vpc-{priority}");
        // The association is made with a VPC of the account's own, which the
        // associate resolves against the Amazon EC2 store.
        router.hold_vpcs([&vpc]);
        let body = json!({
            "CreatorRequestId": format!("priority-{priority}"),
            "FirewallRuleGroupId": group,
            "VpcId": vpc,
            "Priority": priority,
            "Name": format!("assoc-{priority}"),
        });
        let (status, parsed) = call(&router, "AssociateFirewallRuleGroup", body).await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: priority {priority} is within the published range, got {status}: {parsed}"
            );
            assert_eq!(
                association_of(&parsed).get("Priority").and_then(Value::as_i64),
                Some(i64::from(priority)),
                "case {case:?}: the association reports the priority it was made with, got {parsed}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: priority {priority} is outside the published range, got {status}: {parsed}"
            );
            assert!(
                error_type(&parsed).contains("ValidationException"),
                "case {case:?}: a priority outside the range is a ValidationException, got {parsed}"
            );
        }
    }

    // One priority per VPC: a second rule group cannot take a priority an
    // association of the same VPC already holds, and the same priority is free in
    // another VPC.
    let first = rule_group(&router, "uniqueness-first").await;
    let second = rule_group(&router, "uniqueness-second").await;
    router.hold_vpcs(["vpc-shared", "vpc-elsewhere"]);
    ok(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "uniqueness-first",
            "FirewallRuleGroupId": first,
            "VpcId": "vpc-shared",
            "Priority": 700,
            "Name": "first",
        }),
    )
    .await;
    expect_error(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "uniqueness-clash",
            "FirewallRuleGroupId": second,
            "VpcId": "vpc-shared",
            "Priority": 700,
            "Name": "clash",
        }),
        "a priority another association of the same VPC holds",
        "ValidationException",
    )
    .await;
    ok(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "uniqueness-other-vpc",
            "FirewallRuleGroupId": second,
            "VpcId": "vpc-elsewhere",
            "Priority": 700,
            "Name": "elsewhere",
        }),
    )
    .await;
    expect_error(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "uniqueness-repeat-pair",
            "FirewallRuleGroupId": first,
            "VpcId": "vpc-shared",
            "Priority": 800,
            "Name": "repeat pair",
        }),
        "a rule group already associated with the VPC",
        "ValidationException",
    )
    .await;
}

/// An association is changed through its own update and removed through its own
/// disassociate, and both are refused while mutation protection is in force —
/// except for the change that lifts the protection, which is what makes it
/// liftable at all.
#[tokio::test]
async fn an_association_is_updated_and_then_removed() {
    let router = emulator();
    let group = rule_group(&router, "update-group").await;
    router.hold_vpcs(["vpc-update"]);
    let created = ok(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "update-one",
            "FirewallRuleGroupId": group,
            "VpcId": "vpc-update",
            "Priority": 300,
            "Name": "before",
            "MutationProtection": "ENABLED",
        }),
    )
    .await;
    let id = string_member(association_of(&created), "Id");
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    // While mutation protection is in force, neither a change to the association
    // nor its removal is admitted.
    for (case, operation, body) in [
        (
            "a priority change under mutation protection",
            "UpdateFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": id, "Priority": 400 }),
        ),
        (
            "a removal under mutation protection",
            "DisassociateFirewallRuleGroup",
            json!({ "FirewallRuleGroupAssociationId": id }),
        ),
    ] {
        expect_error(&router, operation, body, case, "ValidationException").await;
    }

    // The one change it admits is the one that lifts it.
    let lifted = ok(
        &router,
        "UpdateFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": id, "MutationProtection": "DISABLED" }),
    )
    .await;
    assert_eq!(
        association_of(&lifted)
            .get("MutationProtection")
            .and_then(Value::as_str),
        Some("DISABLED"),
        "lifting mutation protection must report it lifted, got {lifted}"
    );
    assert_eq!(
        association_of(&lifted).get("Status").and_then(Value::as_str),
        Some("UPDATING"),
        "an association with a change in flight reports UPDATING, got {lifted}"
    );
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    // Each member the update names is changed; the others keep their values.
    for (case, request, member, expected) in [
        (
            "the priority alone",
            json!({ "FirewallRuleGroupAssociationId": id, "Priority": 400 }),
            "Priority",
            json!(400),
        ),
        (
            "the name alone",
            json!({ "FirewallRuleGroupAssociationId": id, "Name": "after" }),
            "Name",
            json!("after"),
        ),
    ] {
        let updated = ok(&router, "UpdateFirewallRuleGroupAssociation", request).await;
        assert_eq!(
            association_of(&updated).get(member),
            Some(&expected),
            "case {case:?}: {member} must be reported as updated, got {updated}"
        );
        tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
        let read = ok(
            &router,
            "GetFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": id }),
        )
        .await;
        assert_eq!(
            association_of(&read).get(member),
            Some(&expected),
            "case {case:?}: a later read must report the changed {member}, got {read}"
        );
        assert_eq!(
            association_of(&read)
                .get("FirewallRuleGroupId")
                .and_then(Value::as_str),
            Some(group.as_str()),
            "case {case:?}: an update must not move the association to another group, got {read}"
        );
    }

    let removed = ok(
        &router,
        "DisassociateFirewallRuleGroup",
        json!({ "FirewallRuleGroupAssociationId": id }),
    )
    .await;
    assert_eq!(
        association_of(&removed)
            .get("Status")
            .and_then(Value::as_str),
        Some("DELETING"),
        "the removed association is on its way out, got {removed}"
    );

    for (case, operation, body) in [
        (
            "a read of a removed association",
            "GetFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": id }),
        ),
        (
            "a second removal of the same association",
            "DisassociateFirewallRuleGroup",
            json!({ "FirewallRuleGroupAssociationId": id }),
        ),
        (
            "an update of a removed association",
            "UpdateFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": id, "Priority": 500 }),
        ),
    ] {
        expect_error(&router, operation, body, case, "ResourceNotFoundException").await;
    }

    let listed = ok(
        &router,
        "ListFirewallRuleGroupAssociations",
        json!({ "FirewallRuleGroupId": group }),
    )
    .await;
    assert_eq!(
        listed
            .get("FirewallRuleGroupAssociations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a removed association is no longer listed, got {listed}"
    );
}
