// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `Priority` band a DNS Firewall rule-group
//! association admits, and for the two coded sentences a priority outside that
//! band is refused with.
//!
//! The rule under test: a rule-group association's `Priority` has to fall in the
//! band the service publishes for callers, and a value outside it is refused
//! `ValidationException` with one of TWO service-coded sentences — a value the
//! setting takes but the service keeps back is reported as RESERVED, quoting the
//! band a caller may use, while a value at or beyond the bound the service states
//! for the setting is reported as not valid, quoting that bound. Neither
//! sentence names the value that was asked for, and both are the same for every
//! operation that writes a `Priority`: `AssociateFirewallRuleGroup` and
//! `UpdateFirewallRuleGroupAssociation`.
//!
//! The band and the bound are recorded behavior: `Priority` carries no `range`
//! in the model (`smithy_query(query="shape",shape="Priority")`), and
//! `AssociateFirewallRuleGroup$Priority`'s doc publishes only the usable band
//! ("the allowed values for Priority are between 100 and 9900").
//!
//! The table spans what the rule generalises over: where the value falls — far
//! below the band, each published bound and the value just outside it on either
//! side, the stated bound itself and values beyond it up to the largest integer
//! the wire carries — and both operations for every row.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{call, emulator, error_type, ok, settled_association, string_member, Emulator};

/// The whole message a priority the service keeps back is refused with, written
/// out here rather than built from the emulator's own constants so the wording
/// itself is what this suite pins: the service error code in square brackets,
/// the fault sentence, and the sentence quoting the band a caller may use. The
/// quoted numbers are the service's own band bounds and no part of the request.
const RESERVED: &str = "[RSLVR-02017] The priority value you provided is reserved. \
                        Provide a number between \"100\" and \"9900\".";

/// The whole message a priority at or beyond the bound the service states for
/// the setting is refused with, quoting that bound.
const NOT_VALID: &str =
    "[RSLVR-02018] The Priority you provided is not valid. Provide a number smaller than \"10000\".";

/// One row: a `Priority` and the message the service refuses it with, or `None`
/// for a priority it admits.
///
/// Which SIDE of the band a value falls on is what decides the sentence. The
/// recorded refusals pin the two sides of the bound the service states — a
/// value below the band's floor is reported reserved, a value far past that
/// bound is reported not valid — so every row below the bound takes the
/// reserved sentence and every row at or past it takes the other.
struct Row {
    case: &'static str,
    priority: i64,
    refusal: Option<&'static str>,
}

/// The priorities this suite drives, spanning the band and what lies on either
/// side of it.
const ROWS: &[Row] = &[
    Row {
        case: "a negative priority",
        priority: -1,
        refusal: Some(RESERVED),
    },
    Row {
        case: "zero",
        priority: 0,
        refusal: Some(RESERVED),
    },
    Row {
        case: "the smallest priority the service keeps back",
        priority: 1,
        refusal: Some(RESERVED),
    },
    Row {
        case: "a priority below the published floor",
        priority: 50,
        refusal: Some(RESERVED),
    },
    Row {
        case: "one below the published floor",
        priority: 99,
        refusal: Some(RESERVED),
    },
    Row {
        case: "the published floor",
        priority: 100,
        refusal: None,
    },
    Row {
        case: "a priority inside the band",
        priority: 5_000,
        refusal: None,
    },
    Row {
        case: "the published ceiling",
        priority: 9_900,
        refusal: None,
    },
    Row {
        case: "one above the published ceiling",
        priority: 9_901,
        refusal: Some(RESERVED),
    },
    Row {
        case: "one below the bound the service states",
        priority: 9_999,
        refusal: Some(RESERVED),
    },
    Row {
        case: "the bound the service states",
        priority: 10_000,
        refusal: Some(NOT_VALID),
    },
    Row {
        case: "one past the bound the service states",
        priority: 10_001,
        refusal: Some(NOT_VALID),
    },
    Row {
        case: "far past the bound the service states",
        priority: 99_999,
        refusal: Some(NOT_VALID),
    },
    Row {
        case: "the largest integer the wire carries",
        // `Priority` is a 32-bit integer in the model, so this is the largest
        // value a request can state at all.
        priority: 2_147_483_647,
        refusal: Some(NOT_VALID),
    },
];

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

/// The `Priority` an association response reports.
fn reported_priority(body: &Value) -> Option<i64> {
    body.get("FirewallRuleGroupAssociation")
        .and_then(|association| association.get("Priority"))
        .and_then(Value::as_i64)
}

/// Every priority outside the published band is refused with the coded sentence
/// its position calls for, by BOTH operations that write one, and a refusal
/// writes nothing: the association the update asked to change keeps the priority
/// it had.
#[tokio::test]
async fn a_priority_outside_the_published_band_is_refused_with_its_coded_sentence() {
    let router = emulator();
    let group = rule_group(&router, "band-group").await;
    let other_group = rule_group(&router, "band-other-group").await;
    router.hold_vpcs(["vpc-band", "vpc-band-written"]);

    // One association the update rows address, settled so a refusal is about
    // the priority they ask for and nothing else.
    let made = ok(
        &router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": "band-standing",
            "FirewallRuleGroupId": other_group,
            "VpcId": "vpc-band-written",
            "Priority": 7_000,
            "Name": "band-standing",
        }),
    )
    .await;
    let standing = string_member(
        made.get("FirewallRuleGroupAssociation")
            .expect("the associate answers the association"),
        "Id",
    );
    settled_association(&router, &standing, "the standing association").await;

    for Row {
        case,
        priority,
        refusal,
    } in ROWS
    {
        let Some(expected) = refusal else {
            continue;
        };

        let (status, parsed) = call(
            &router,
            "AssociateFirewallRuleGroup",
            json!({
                "CreatorRequestId": format!("band-associate-{priority}"),
                "FirewallRuleGroupId": group,
                "VpcId": "vpc-band",
                "Priority": priority,
                "Name": "band-associate",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: AssociateFirewallRuleGroup must refuse {priority}, got {status}: \
             {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        // The comparison is against the FIXED sentence, which is what shows the
        // priority that was asked for is not named anywhere in it: two rows of
        // one class differ in the value and not in the message.
        assert_eq!(
            &message, expected,
            "case {case:?}: AssociateFirewallRuleGroup must state the service's sentence for this \
             priority, got {message:?}"
        );

        let (status, parsed) = call(
            &router,
            "UpdateFirewallRuleGroupAssociation",
            json!({
                "FirewallRuleGroupAssociationId": standing,
                "Priority": priority,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateFirewallRuleGroupAssociation must refuse {priority}, got \
             {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        assert_eq!(
            &message, expected,
            "case {case:?}: the update must state the very same sentence as the associate, got \
             {message:?}"
        );
    }

    // No refused associate wrote a record, and the association the refused
    // updates addressed keeps the priority it was made with.
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
            .map_or(0, Vec::len),
        0,
        "a refused associate must write no record, got {listed}"
    );
    let read = ok(
        &router,
        "GetFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": standing }),
    )
    .await;
    assert_eq!(
        reported_priority(&read),
        Some(7_000),
        "a refused update must leave the association's Priority alone, got {read}"
    );
}

/// Every priority inside the published band is admitted by both operations: the
/// associate reports it and a later update moves the association onto another
/// value of the band.
#[tokio::test]
async fn every_priority_inside_the_published_band_is_admitted() {
    let router = emulator();
    let group = rule_group(&router, "admitted-group").await;

    for Row {
        case,
        priority,
        refusal,
    } in ROWS
    {
        if refusal.is_some() {
            continue;
        }
        // One VPC per row, so a row is about the priority and never about a
        // priority another association of the same VPC already holds.
        let vpc = format!("vpc-admitted-{priority}");
        router.hold_vpcs([vpc.as_str()]);

        let made = ok(
            &router,
            "AssociateFirewallRuleGroup",
            json!({
                "CreatorRequestId": format!("admitted-{priority}"),
                "FirewallRuleGroupId": group,
                "VpcId": vpc,
                "Priority": priority,
                "Name": "admitted",
            }),
        )
        .await;
        assert_eq!(
            reported_priority(&made),
            Some(*priority),
            "case {case:?}: the associate must report the priority it was made with, got {made}"
        );
        let id = string_member(
            made.get("FirewallRuleGroupAssociation")
                .expect("the associate answers the association"),
            "Id",
        );
        settled_association(&router, &id, case).await;

        // The other published bound of the band is a value the update admits
        // too, so what the band governs is the value and not the operation.
        let moved_to = if *priority == 100 { 9_900 } else { 100 };
        let changed = ok(
            &router,
            "UpdateFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": id, "Priority": moved_to }),
        )
        .await;
        assert_eq!(
            reported_priority(&changed),
            Some(moved_to),
            "case {case:?}: the update must report the priority it moved to, got {changed}"
        );
        settled_association(&router, &id, case).await;
        let read = ok(
            &router,
            "GetFirewallRuleGroupAssociation",
            json!({ "FirewallRuleGroupAssociationId": id }),
        )
        .await;
        assert_eq!(
            reported_priority(&read),
            Some(moved_to),
            "case {case:?}: a later read must report the priority the update wrote, got {read}"
        );
    }
}
