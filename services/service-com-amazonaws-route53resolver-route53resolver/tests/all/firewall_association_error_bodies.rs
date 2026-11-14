// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the exception BODIES the DNS Firewall rule-group
//! association slice answers with: the refusal of a `Priority` another
//! association of the same VPC already holds, and the refusal of a change to, or
//! a removal of, an association that carries mutation protection.
//!
//! The rule these span: every Route 53 Resolver exception message opens with the
//! service's own error code in square brackets and continues with that fault's
//! own wording, because the HTTP status (400 for every modelled Resolver error)
//! and the exception shape name together still do not say which Resolver fault
//! occurred — and the operations that can raise one fault report ONE body for
//! it, with only the values the request named or the store holds interpolated
//! into it. A refusal of this slice then closes with the trace clause of the
//! request, ` Trace Id: "<Root>"`, so a caller can tie it back to the call that
//! drew it.
//!
//! The suite that asserts the same shape of body for the Resolver rule slice is
//! [`super::resolver_rule_error_bodies`]; this one is the firewall rule-group
//! association slice's.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use service_com_amazonaws_route53resolver_route53resolver::state::ASSOCIATION_PRIORITY_RANGE;

use super::harness::{
    call, call_tracing, emulator, error_type, ok, settled_association, string_member,
    traced_message, tracing_chain, Emulator,
};

/// The Route 53 Resolver service error code for a rule-group association asked
/// for a priority another association of the same VPC already holds, from the
/// recorded `ValidationException` bodies of `AssociateFirewallRuleGroup` and
/// `UpdateFirewallRuleGroupAssociation`.
const CODE_PRIORITY_IN_USE: &str = "RSLVR-02303";

/// The code for a change to, or a removal of, a mutation-protected association,
/// from the recorded `ValidationException` bodies of
/// `UpdateFirewallRuleGroupAssociation` and `DisassociateFirewallRuleGroup`.
const CODE_MUTATION_PROTECTED: &str = "RSLVR-02307";

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

/// Associate the rule group `group` with the VPC `vpc` at `priority` and answer
/// the association id Route 53 Resolver minted — never an id written out here.
async fn associate(
    router: &Emulator,
    request_id: &str,
    group: &str,
    vpc: &str,
    priority: i32,
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

/// Assert one refusal: HTTP 400, the modelled shape, the exact sentence, and
/// the trace clause the service closes a refusal of this slice with — the
/// `Root` the request named where `expected_root` names one, and a trace the
/// service minted for the request where it does not.
fn assert_refusal(
    case: &str,
    operation: &str,
    status: u16,
    parsed: &Value,
    expected_shape: &str,
    expected_message: &str,
    expected_root: Option<&str>,
) {
    assert_eq!(
        status, 400,
        "case {case:?}/{operation}: expected HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(parsed).contains(expected_shape),
        "case {case:?}/{operation}: expected {expected_shape}, got {parsed}"
    );
    let message = string_member(parsed, "Message");
    let (stated, trace) = traced_message(&message).unwrap_or_else(|| {
        panic!(
            "case {case:?}/{operation}: the refusal must close with its trace clause, got \
             {message:?}"
        )
    });
    assert_eq!(
        stated, expected_message,
        "case {case:?}/{operation}: expected Message {expected_message:?} before the clause, got \
         {message:?}"
    );
    if let Some(root) = expected_root {
        assert_eq!(
            trace, root,
            "case {case:?}/{operation}: the clause must carry the Root the request named, got \
             {message:?}"
        );
    }
}

/// Both operations that write a rule-group association's `Priority`
/// (`AssociateFirewallRuleGroup`, `UpdateFirewallRuleGroupAssociation`) refuse a
/// priority another association of the same VPC already holds with the one
/// RSLVR-coded body, which names the association that holds it.
///
/// The table spans what the rule generalises over: both published bounds of the
/// priority range and a value between them, each in its own VPC, so the body is
/// the same whatever value clashed; and the association the refusal names is
/// the one the store holds the priority under, read out of the response that
/// minted it rather than written here. A row also updates an association to the
/// priority it already holds, which clashes with nothing.
#[tokio::test]
async fn a_priority_another_association_holds_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let lowest = *ASSOCIATION_PRIORITY_RANGE.start();
    let highest = *ASSOCIATION_PRIORITY_RANGE.end();

    for (case, priority) in [
        ("the lowest published priority", lowest),
        ("a priority inside the range", (lowest + highest) / 2),
        ("the highest published priority", highest),
    ] {
        let vpc = format!("vpc-clash-{priority}");
        router.hold_vpcs([&vpc]);
        let holding_group = rule_group(&router, &format!("holder-{priority}")).await;
        let other_group = rule_group(&router, &format!("other-{priority}")).await;

        let holder = associate(
            &router,
            &format!("holder-{priority}"),
            &holding_group,
            &vpc,
            priority,
            "DISABLED",
        )
        .await;
        assert!(
            !holder.is_empty(),
            "case {case:?}: the associate must answer the id it minted, got {holder:?}"
        );
        let expected = format!(
            "[{CODE_PRIORITY_IN_USE}] This priority is the same with another association: \
             \"{holder}\" . Try again with another priority number."
        );

        // The associate of a SECOND rule group with the same VPC at that
        // priority is refused with the body naming the association that holds
        // it, under the chain the request named.
        let (header, root) = tracing_chain(priority.unsigned_abs());
        let (status, parsed) = call_tracing(
            &router,
            "AssociateFirewallRuleGroup",
            json!({
                "CreatorRequestId": format!("clash-{priority}"),
                "FirewallRuleGroupId": other_group,
                "VpcId": vpc,
                "Priority": priority,
                "Name": format!("clash-{priority}"),
            }),
            Some(&header),
        )
        .await;
        assert_refusal(
            case,
            "AssociateFirewallRuleGroup",
            status,
            &parsed,
            "ValidationException",
            &expected,
            Some(&root),
        );

        // The same clash reached through the update: the second group is
        // associated at a free priority first, then asked for the taken one.
        let free = if priority == highest {
            priority - 1
        } else {
            priority + 1
        };
        let updated_association = associate(
            &router,
            &format!("free-{priority}"),
            &other_group,
            &vpc,
            free,
            "DISABLED",
        )
        .await;
        settled_association(&router, &updated_association, case).await;
        let (status, parsed) = call(
            &router,
            "UpdateFirewallRuleGroupAssociation",
            json!({
                "FirewallRuleGroupAssociationId": updated_association,
                "Priority": priority,
            }),
        )
        .await;
        assert_refusal(
            case,
            "UpdateFirewallRuleGroupAssociation",
            status,
            &parsed,
            "ValidationException",
            &expected,
            None,
        );
        assert!(
            !string_member(&parsed, "Message").contains(&updated_association),
            "case {case:?}: the refusal names the association HOLDING the priority, not the one \
             asking for it, got {parsed}"
        );

        // An association asked for the priority it already holds clashes with
        // nothing: it is itself.
        let served = ok(
            &router,
            "UpdateFirewallRuleGroupAssociation",
            json!({
                "FirewallRuleGroupAssociationId": updated_association,
                "Priority": free,
            }),
        )
        .await;
        assert_eq!(
            served
                .get("FirewallRuleGroupAssociation")
                .and_then(|association| association.get("Priority"))
                .and_then(Value::as_i64),
            Some(i64::from(free)),
            "case {case:?}: an association keeps the priority it already holds, got {served}"
        );
    }
}

/// Both operations a mutation-protected association refuses
/// (`UpdateFirewallRuleGroupAssociation`, `DisassociateFirewallRuleGroup`)
/// answer the one RSLVR-coded body for that fault, whose sentence is fixed: it
/// names neither the association nor the member that was to change, and it
/// closes with the trace clause of the request.
///
/// The table spans what the rule generalises over: each member an update may
/// name while the protection stands, the removal, and whether the request named
/// a chain of its own or left the service to mint one.
#[tokio::test]
async fn a_mutation_protected_association_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let vpc = "vpc-protected";
    router.hold_vpcs([vpc]);
    let group = rule_group(&router, "protected-group").await;
    let protected = associate(&router, "protected", &group, vpc, 500, "ENABLED").await;
    settled_association(&router, &protected, "the protected association").await;

    let expected = format!(
        "[{CODE_MUTATION_PROTECTED}] The DNS Firewall rule group association is under mutation \
         protection. Remove the mutation protection and try again."
    );
    let (header, root) = tracing_chain(7);

    for (index, (case, operation, body)) in [
        (
            "a priority change under mutation protection",
            "UpdateFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": protected, "Priority": 600 }),
        ),
        (
            "a name change under mutation protection",
            "UpdateFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": protected, "Name": "renamed" }),
        ),
        (
            "a restatement of the protection itself",
            "UpdateFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": protected, "MutationProtection": "ENABLED" }),
        ),
        (
            "a removal under mutation protection",
            "DisassociateFirewallRuleGroup",
            json!({ "FirewallRuleGroupAssociationId": protected }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        // Half the rows name a chain of their own, half leave the service to
        // mint one: the clause is there either way.
        let names_a_chain = index % 2 == 0;
        let (status, parsed) = call_tracing(
            &router,
            operation,
            body,
            names_a_chain.then_some(header.as_str()),
        )
        .await;
        assert_refusal(
            case,
            operation,
            status,
            &parsed,
            "ValidationException",
            &expected,
            names_a_chain.then_some(root.as_str()),
        );
        assert!(
            !string_member(&parsed, "Message").contains(&protected),
            "case {case:?}: the fixed sentence must not name the association, got {parsed}"
        );
    }

    // The protection is liftable, and once lifted the same change is served —
    // so the refusals above are about the protection and not about the
    // association.
    ok(
        &router,
        "UpdateFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": protected, "MutationProtection": "DISABLED" }),
    )
    .await;
    settled_association(
        &router,
        &protected,
        "the association once the protection is lifted",
    )
    .await;
    let changed = ok(
        &router,
        "UpdateFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": protected, "Priority": 600 }),
    )
    .await;
    assert_eq!(
        changed
            .get("FirewallRuleGroupAssociation")
            .and_then(|association| association.get("Priority"))
            .and_then(Value::as_i64),
        Some(600),
        "the change the protection refused must be served once it is lifted, got {changed}"
    );
}
