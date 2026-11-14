// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the order a DNS Firewall deployment is torn down in: a
//! resource another one still holds cannot be deleted, and the call that
//! releases it is served whatever step it stands in.
//!
//! The rules under test:
//!
//! * a DNS Firewall resource another DNS Firewall resource still references is
//!   not deleted — the delete is refused `ConflictException` with the service's
//!   coded sentence for that fault, naming the referencing resource, the
//!   referenced one is left exactly as it was, and the very same delete is
//!   served once the reference is gone. The two references are a `FirewallRule`
//!   matching against a `FirewallDomainList`, and a
//!   `FirewallRuleGroupAssociation` holding a `FirewallRuleGroup` on a VPC;
//! * `DisassociateFirewallRuleGroup` removes the association the store holds
//!   whatever lifecycle step it stands in — removal is not a change a caller has
//!   to wait for — and only mutation protection refuses it. The removed
//!   association reports `DELETING`, and every later read of its id reports it
//!   missing;
//! * each of these refusals closes its coded sentence with the trace clause of
//!   the request, ` Trace Id: "<Root>"`, carrying the `Root` the caller named in
//!   `X-Amzn-Trace-Id` and one the service minted for the request where the
//!   caller named none.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use service_com_amazonaws_route53resolver_route53resolver::state::FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_DELETING;

use super::harness::{
    call, call_tracing, emulator, error_type, ok, settled_association, string_member,
    traced_message, tracing_chain, Emulator,
};

/// Create one rule group and answer the id Route 53 Resolver minted for it.
async fn rule_group(router: &Emulator, name: &str) -> String {
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

/// Create one domain list holding `domain`, and answer the id Route 53 Resolver
/// minted for it.
async fn domain_list(router: &Emulator, name: &str, domain: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("list-{name}"), "Name": name }),
    )
    .await;
    let id = string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers with the FirewallDomainList"),
        "Id",
    );
    ok(
        router,
        "UpdateFirewallDomains",
        json!({ "FirewallDomainListId": id, "Operation": "ADD", "Domains": [domain] }),
    )
    .await;
    id
}

/// Associate `group` with `vpc` at `priority` and answer the association id.
async fn associate(
    router: &Emulator,
    request_id: &str,
    group: &str,
    vpc: &str,
    priority: i64,
    mutation_protection: &str,
) -> String {
    let made = ok(
        router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": request_id,
            "FirewallRuleGroupId": group,
            "VpcId": vpc,
            "Priority": priority,
            "Name": format!("assoc-{request_id}"),
            "MutationProtection": mutation_protection,
        }),
    )
    .await;
    string_member(
        made.get("FirewallRuleGroupAssociation")
            .expect("the associate answers with the FirewallRuleGroupAssociation"),
        "Id",
    )
}

/// Assert one refused delete: HTTP 400, `ConflictException`, the coded sentence
/// before the trace clause, and the `Root` of the request inside it.
fn assert_conflict(
    case: &str,
    status: u16,
    parsed: &Value,
    expected_sentence: &str,
    expected_root: Option<&str>,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: the delete must be refused with HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(parsed).contains("ConflictException"),
        "case {case:?}: the refusal must name ConflictException, got {parsed}"
    );
    let message = string_member(parsed, "Message");
    let (stated, trace) = traced_message(&message).unwrap_or_else(|| {
        panic!("case {case:?}: the refusal must close with its trace clause, got {message:?}")
    });
    assert_eq!(
        stated, expected_sentence,
        "case {case:?}: the refusal must state the service's sentence for this fault, got \
         {message:?}"
    );
    if let Some(root) = expected_root {
        assert_eq!(
            trace, root,
            "case {case:?}: the clause must carry the Root the request named, got {message:?}"
        );
    }
}

/// One row of the in-use domain-list table: the rule that references the list.
struct RuleRow {
    case: &'static str,
    /// The members the referencing rule is created with beside its group, its
    /// domain list, its name and its priority.
    members: fn() -> Value,
    /// The domain the list holds, so a row is exercised on a list with content
    /// of its own.
    domain: &'static str,
}

/// Every kind of rule that can reference a domain list, so what refuses the
/// delete is the reference and not the rule's settings.
const RULE_ROWS: &[RuleRow] = &[
    RuleRow {
        case: "a rule that alerts",
        members: || json!({ "Action": "ALERT" }),
        domain: "alerted.example.com",
    },
    RuleRow {
        case: "a rule that blocks with NXDOMAIN",
        members: || json!({ "Action": "BLOCK", "BlockResponse": "NXDOMAIN" }),
        domain: "blocked.example.com",
    },
    RuleRow {
        case: "a rule that allows, selected by a query type",
        members: || json!({ "Action": "ALLOW", "Qtype": "AAAA" }),
        domain: "allowed.example.com",
    },
    RuleRow {
        case: "a rule that blocks with an override record",
        members: || {
            json!({
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "sinkhole.example.com",
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            })
        },
        domain: "overridden.example.com",
    },
];

/// A domain list a DNS Firewall rule still matches against is not deleted: the
/// delete is refused with the coded sentence naming the rule's group, the list
/// keeps its domains, and the same delete is served once the rule is gone.
#[tokio::test]
async fn a_domain_list_a_rule_matches_against_cannot_be_deleted() {
    let router = emulator();

    for (index, RuleRow {
        case,
        members,
        domain,
    }) in RULE_ROWS.iter().enumerate()
    {
        let group = rule_group(&router, &format!("in-use-{index}")).await;
        let list = domain_list(&router, &format!("in-use-{index}"), domain).await;

        let mut body = json!({
            "CreatorRequestId": format!("in-use-rule-{index}"),
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
            "Name": format!("in-use-rule-{index}"),
            "Priority": 100,
        });
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in members()
            .as_object()
            .expect("the row's members are an object")
            .clone()
        {
            object.insert(member, value);
        }
        ok(&router, "CreateFirewallRule", body).await;

        let expected = format!(
            "[RSLVR-02304] The DNS Firewall domain list is associated with a DNS Firewall rule : \
             \"{group}\". Delete the rule and then try again."
        );

        // Half the rows name a chain of their own, half leave the service to
        // mint one: the clause is there either way.
        let names_a_chain = index % 2 == 0;
        let (header, root) = tracing_chain(u32::try_from(index).expect("a small index"));
        let (status, parsed) = call_tracing(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": list }),
            names_a_chain.then_some(header.as_str()),
        )
        .await;
        assert_conflict(
            case,
            status,
            &parsed,
            &expected,
            names_a_chain.then_some(root.as_str()),
        );

        // The refusal wrote nothing: the list is still readable and still holds
        // the domain it was given, so the rule still matches against something
        // that exists.
        let read = ok(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": list }),
        )
        .await;
        assert_eq!(
            read.get("FirewallDomainList")
                .and_then(|list| list.get("DomainCount"))
                .and_then(Value::as_i64),
            Some(1),
            "case {case:?}: the refused delete must leave the list and its domains, got {read}"
        );

        // The rule goes first, and then the very same delete is served.
        ok(
            &router,
            "DeleteFirewallRule",
            json!({ "FirewallRuleGroupId": group, "FirewallDomainListId": list }),
        )
        .await;
        let deleted = ok(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": list }),
        )
        .await;
        assert_eq!(
            string_member(
                deleted
                    .get("FirewallDomainList")
                    .unwrap_or_else(|| panic!("case {case:?}: the delete answers the list")),
                "Id"
            ),
            list,
            "case {case:?}: a list no rule references must be deletable, got {deleted}"
        );
    }
}

/// A list DNS Firewall publishes itself is not a resource this account can
/// delete at all, so a delete naming one is reported MISSING even while a rule
/// of this account matches against it: the in-use conflict is about a list this
/// account holds, and a published list is not one.
#[tokio::test]
async fn a_published_list_a_rule_matches_against_is_still_reported_missing() {
    let router = emulator();
    let group = rule_group(&router, "published-in-use").await;

    // The lists the service publishes, read out of the listing rather than
    // written here, so the case names an id the service itself reports.
    let listing = ok(&router, "ListFirewallDomainLists", json!({ "MaxResults": 100 })).await;
    let published: Vec<String> = listing
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the domain lists")
        .iter()
        .filter(|item| item.get("ManagedOwnerName").is_some())
        .map(|item| string_member(item, "Id"))
        .collect();
    assert!(
        !published.is_empty(),
        "the listing must report the lists DNS Firewall publishes, got {listing}"
    );

    for (index, list) in published.iter().enumerate() {
        let case = format!("the published list {list}");
        ok(
            &router,
            "CreateFirewallRule",
            json!({
                "CreatorRequestId": format!("published-in-use-{index}"),
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Name": format!("published-in-use-{index}"),
                "Priority": 100 + i64::try_from(index).expect("a small index") * 10,
                "Action": "ALERT",
            }),
        )
        .await;

        let (status, parsed) = call(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": list }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a published list must be refused, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ResourceNotFoundException"),
            "case {case:?}: a published list is not visible as a writable resource, so the delete \
             must report it missing rather than in use, got {parsed}"
        );
    }
}

/// A rule group a VPC is associated with is not deleted: the delete is refused
/// with the coded sentence naming the group, the group keeps its record, and the
/// same delete is served once the association is gone.
///
/// The rows differ along the step the association stands in when the delete
/// arrives — one still being put in place, one that has settled — so what
/// refuses the delete is the association and not its lifecycle state.
#[tokio::test]
async fn a_rule_group_a_vpc_is_associated_with_cannot_be_deleted() {
    let router = emulator();

    for (index, (case, wait_for_the_association)) in [
        ("an association still being put in place", false),
        ("an association that has settled", true),
    ]
    .into_iter()
    .enumerate()
    {
        let group = rule_group(&router, &format!("associated-{index}")).await;
        let vpc = format!("vpc-associated-{index}");
        router.hold_vpcs([vpc.as_str()]);
        let association = associate(
            &router,
            &format!("associated-{index}"),
            &group,
            &vpc,
            150 + i64::try_from(index).expect("a small index"),
            "DISABLED",
        )
        .await;
        if wait_for_the_association {
            settled_association(&router, &association, case).await;
        }

        let expected = format!(
            "[RSLVR-02101] The DNS Firewall rule group is associated to a VPC: \"{group}\". \
             Delete the association and try again later."
        );
        let (header, root) = tracing_chain(100 + u32::try_from(index).expect("a small index"));
        let (status, parsed) = call_tracing(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": group }),
            Some(header.as_str()),
        )
        .await;
        assert_conflict(case, status, &parsed, &expected, Some(root.as_str()));

        // The refusal wrote nothing: the group is still readable, so the
        // association still names a group that exists.
        let read = ok(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": group }),
        )
        .await;
        assert_eq!(
            string_member(
                read.get("FirewallRuleGroup")
                    .unwrap_or_else(|| panic!("case {case:?}: the read answers the group")),
                "Id"
            ),
            group,
            "case {case:?}: the refused delete must leave the group in place, got {read}"
        );

        // The association goes first, and then the very same delete is served.
        ok(
            &router,
            "DisassociateFirewallRuleGroup",
            json!({ "FirewallRuleGroupAssociationId": association }),
        )
        .await;
        let deleted = ok(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": group }),
        )
        .await;
        assert_eq!(
            string_member(
                deleted
                    .get("FirewallRuleGroup")
                    .unwrap_or_else(|| panic!("case {case:?}: the delete answers the group")),
                "Id"
            ),
            group,
            "case {case:?}: a group no association names must be deletable, got {deleted}"
        );
    }
}

/// An association the store holds is removed whatever lifecycle step it stands
/// in: the disassociate answers HTTP 200 with the association reported
/// `DELETING`, and afterwards every read of that id reports it missing with the
/// coded sentence and the trace clause of the request.
///
/// The rows differ along the step the association stands in when the
/// disassociate arrives: one just created, one that has settled, and one with a
/// change of its own still in flight.
#[tokio::test]
async fn an_association_is_removed_whatever_step_it_stands_in() {
    let router = emulator();
    let group = rule_group(&router, "disassociable").await;

    for (index, (case, wait, change_it_first)) in [
        ("an association just created", false, false),
        ("an association that has settled", true, false),
        ("an association with a change in flight", true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let vpc = format!("vpc-disassociable-{index}");
        router.hold_vpcs([vpc.as_str()]);
        let association = associate(
            &router,
            &format!("disassociable-{index}"),
            &group,
            &vpc,
            300 + i64::try_from(index).expect("a small index"),
            "DISABLED",
        )
        .await;
        if wait {
            settled_association(&router, &association, case).await;
        }
        if change_it_first {
            // The change leaves the association reporting its transient status,
            // which the removal below does not wait out.
            ok(
                &router,
                "UpdateFirewallRuleGroupAssociation",
                json!({ "FirewallRuleGroupAssociationId": association, "Priority": 400 }),
            )
            .await;
        }

        let removed = ok(
            &router,
            "DisassociateFirewallRuleGroup",
            json!({ "FirewallRuleGroupAssociationId": association }),
        )
        .await;
        let projected = removed
            .get("FirewallRuleGroupAssociation")
            .unwrap_or_else(|| panic!("case {case:?}: the disassociate answers the association"));
        assert_eq!(
            string_member(projected, "Id"),
            association,
            "case {case:?}: the disassociate must answer the association it took out, got {removed}"
        );
        assert_eq!(
            string_member(projected, "Status"),
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_DELETING,
            "case {case:?}: the removed association is on its way out, got {removed}"
        );

        // The reads flip to the coded not-found sentence, which closes with the
        // trace clause of the request.
        let expected = format!("[RSLVR-02025] Can’t find the resource with ID \"{association}\".");
        let (header, root) = tracing_chain(200 + u32::try_from(index).expect("a small index"));
        for operation in [
            "GetFirewallRuleGroupAssociation",
            "DisassociateFirewallRuleGroup",
        ] {
            let (status, parsed) = call_tracing(
                &router,
                operation,
                json!({ "FirewallRuleGroupAssociationId": association }),
                Some(header.as_str()),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must report the removed id missing, got {status}: \
                 {parsed}"
            );
            assert!(
                error_type(&parsed).contains("ResourceNotFoundException"),
                "case {case:?}: {operation} must name ResourceNotFoundException, got {parsed}"
            );
            let message = string_member(&parsed, "Message");
            let (stated, trace) = traced_message(&message).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {operation}'s refusal must close with its trace clause, got \
                     {message:?}"
                )
            });
            assert_eq!(
                stated, expected,
                "case {case:?}: {operation} must state the coded sentence naming the id, got \
                 {message:?}"
            );
            assert_eq!(
                trace, root,
                "case {case:?}: the clause must carry the Root the request named, got {message:?}"
            );
        }
    }
}

/// A `FirewallRuleGroupAssociationId` no associate ever minted is reported
/// missing by the disassociate, with the coded sentence naming the id the
/// request stated and the trace clause of the request — the rows span the forms
/// the model's `ResourceId` (length 1..64) takes.
#[tokio::test]
async fn an_unknown_association_is_reported_missing_with_the_trace_clause() {
    let router = emulator();
    let longest = format!("rslvr-frgassoc-{}", "f".repeat(49));
    assert_eq!(
        longest.len(),
        64,
        "the longest row must sit on ResourceId's upper bound, got {}",
        longest.len()
    );

    for (index, (case, id)) in [
        (
            "an id in the form the service mints",
            "rslvr-frgassoc-00000000000000000".to_string(),
        ),
        ("the shortest id the model admits", "z".to_string()),
        ("the longest id the model admits", longest),
        (
            "another resource's id",
            "rslvr-frg-0123456789abcdef".to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let names_a_chain = index % 2 == 0;
        let (header, root) = tracing_chain(300 + u32::try_from(index).expect("a small index"));
        let (status, parsed) = call_tracing(
            &router,
            "DisassociateFirewallRuleGroup",
            json!({ "FirewallRuleGroupAssociationId": id }),
            names_a_chain.then_some(header.as_str()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the disassociate must report the id missing, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must name ResourceNotFoundException, got {parsed}"
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "FirewallRuleGroupAssociation",
            "case {case:?}: the refusal must name the resource that is missing, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        let (stated, trace) = traced_message(&message).unwrap_or_else(|| {
            panic!("case {case:?}: the refusal must close with its trace clause, got {message:?}")
        });
        assert_eq!(
            stated,
            format!("[RSLVR-02025] Can’t find the resource with ID \"{id}\"."),
            "case {case:?}: the refusal must name the id the request stated, got {message:?}"
        );
        if names_a_chain {
            assert_eq!(
                trace, root,
                "case {case:?}: the clause must carry the Root the request named, got {message:?}"
            );
        }
    }
}

/// Mutation protection is the ONE thing that refuses a removal, and it refuses
/// it with the coded sentence for that fault rather than with a conflict about
/// the association's state: the association stays, and the removal is served
/// once the protection is lifted.
#[tokio::test]
async fn only_mutation_protection_refuses_a_removal() {
    let router = emulator();
    let group = rule_group(&router, "protected").await;
    router.hold_vpcs(["vpc-protected-removal"]);
    let association = associate(
        &router,
        "protected-removal",
        &group,
        "vpc-protected-removal",
        500,
        "ENABLED",
    )
    .await;

    let (status, parsed) = call(
        &router,
        "DisassociateFirewallRuleGroup",
        json!({ "FirewallRuleGroupAssociationId": association }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a protected association must refuse the removal, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains("ValidationException"),
        "the refusal must name ValidationException, got {parsed}"
    );
    let message = string_member(&parsed, "Message");
    let (stated, _) = traced_message(&message)
        .unwrap_or_else(|| panic!("the refusal must close with its trace clause, got {message:?}"));
    assert_eq!(
        stated,
        "[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. \
         Remove the mutation protection and try again.",
        "the refusal must state the service's sentence for mutation protection, got {message:?}"
    );

    // The refusal wrote nothing, so the association is still there to be
    // removed once the protection is lifted.
    settled_association(&router, &association, "the protected association").await;
    ok(
        &router,
        "UpdateFirewallRuleGroupAssociation",
        json!({
            "FirewallRuleGroupAssociationId": association,
            "MutationProtection": "DISABLED",
        }),
    )
    .await;
    let removed = ok(
        &router,
        "DisassociateFirewallRuleGroup",
        json!({ "FirewallRuleGroupAssociationId": association }),
    )
    .await;
    assert_eq!(
        string_member(
            removed
                .get("FirewallRuleGroupAssociation")
                .expect("the disassociate answers the association"),
            "Status"
        ),
        FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_DELETING,
        "the removal the protection refused must be served once it is lifted, got {removed}"
    );
}
