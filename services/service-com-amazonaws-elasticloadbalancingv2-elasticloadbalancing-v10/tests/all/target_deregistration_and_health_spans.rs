// SPDX-License-Identifier: CC0-1.0

//! The TARGET HEALTH state machine of a target group, driven over the wire exactly as a
//! client drives it: `RegisterTargets`, `DeregisterTargets`,
//! `ModifyTargetGroupAttributes` and `DescribeTargetHealth`.
//!
//! Two rules are asserted here, each over a SET of inputs rather than over the one call a
//! recording happens to carry:
//!
//! * DEREGISTRATION. A registration a load balancer is sending traffic to enters
//!   `draining` (reason `Target.DeregistrationInProgress`) when it is deregistered,
//!   receives no new request, and leaves the target group once that group's
//!   `deregistration_delay.timeout_seconds` has passed — so a client that drains
//!   connections before tearing a target down can observe the drain. A registration
//!   nothing is sending traffic to (a target group no load balancer forwards to, or a
//!   delay of zero) has no connections to drain and leaves the group at once.
//! * THE SETTLING SPAN. Health checking on a registration BEGINS when the registration is
//!   made or when the target group first becomes one a load balancer forwards to,
//!   whichever is later — so the first state reported after the attach is `initial`
//!   (reason `Elb.RegistrationInProgress`) and only then `healthy`. Before any load
//!   balancer forwards to the group nothing is checking its targets at all, which is
//!   `unused`/`Target.NotInUse`.
//!
//! The rows differ along what those rules generalise over: the configured delay (the
//! documented default, an hour, a minute, a second, none at all, and a value no
//! deregistration can drain for), whether the group is one a load balancer forwards to,
//! how the association was made (a listener's default action or a rule's), how many
//! registrations the group holds, and how many deregistrations one registration gets.
//!
//! The spans are the service's own: the deregistration delay is the value the attribute
//! carries, and the settling span is `state::SETTLE_AFTER` — read from the crate rather
//! than restated, so these rows cannot drift from the handler.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, forward_to, host_header,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, health_block, members, p, Reply,
};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    DEREGISTRATION_DELAY_ATTRIBUTE, SETTLE_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

/// Long enough past a span for the clock to owe the transition, whatever scheduling the
/// test run sees.
const SLACK: Duration = Duration::from_millis(400);

/// Wait out `span` plus [`SLACK`]. This suite waits on the std clock as the rest of this
/// crate's suites do; nothing else is pending on this runtime.
fn wait_out(span: Duration) {
    std::thread::sleep(span + SLACK);
}

/// Register `ids` with the target group `arn` names, each on the group's own port.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, ids: &[&str]) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    for (index, id) in ids.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), id));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// Deregister `id` from the target group `arn` names.
async fn deregister(router: &MultiServiceRouter, case: &str, arn: &str, id: &str) -> Reply {
    let mut form = action("DeregisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", id));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DeregisterTargets, got {} body {}",
        reply.status, reply.body
    );
    reply
}

/// Write one target group attribute.
async fn set_attribute(router: &MultiServiceRouter, case: &str, arn: &str, key: &str, value: &str) {
    let mut form = action("ModifyTargetGroupAttributes");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Attributes.member.1.Key", key));
    form.push(p("Attributes.member.1.Value", value));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ModifyTargetGroupAttributes, got {} body {}",
        reply.status, reply.body
    );
}

/// `(target id, state, reason)` for every registration the group reports, in the order
/// reported. A reason the service does not report reads as the empty string.
async fn health(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String, String)> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DescribeTargetHealth, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            // The health's own element, because the override beside it carries members of
            // the same names ([`health_block`]).
            let health = health_block(description);
            (
                first_text(description, "Id").unwrap_or_default(),
                first_text(&health, "State").unwrap_or_default(),
                first_text(&health, "Reason").unwrap_or_default(),
            )
        })
        .collect()
}

/// `(state, reason)` of the registration `id`, as a read NAMING that target reports it —
/// which is the read a client polling one target makes.
async fn health_of(router: &MultiServiceRouter, case: &str, arn: &str, id: &str) -> (String, String) {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", id));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DescribeTargetHealth, got {} body {}",
        reply.status, reply.body
    );
    // The health's own element, because the override beside it carries members of the same
    // names ([`health_block`]).
    let health = health_block(&reply.body);
    (
        first_text(&health, "State").unwrap_or_default(),
        first_text(&health, "Reason").unwrap_or_default(),
    )
}

/// The ids the group under test registers: two, so a deregistration of one is visibly not
/// a deregistration of the other.
const DRAINED: &str = "i-0aaaaaaaaaaaaaaa1";
const KEPT: &str = "i-0aaaaaaaaaaaaaaa2";

/// A load balancer with an HTTP listener forwarding to `group`, so the group is one a
/// load balancer sends traffic to.
async fn forwarding_load_balancer(router: &MultiServiceRouter, name: &str, group: &str) -> String {
    let lb = create_load_balancer(router, name).await;
    create_listener(router, &lb, group, 80).await;
    lb
}

// ---------------------------------------------------------------------------
// Deregistration: the drain, and the removal the delay owes.
// ---------------------------------------------------------------------------

/// One row of the drain table: the delay the group carries, and whether a deregistration
/// of a target the load balancer is sending traffic to drains at all.
struct DrainRow {
    case: &'static str,
    /// The value written to `deregistration_delay.timeout_seconds`, or `None` to leave
    /// the group at the documented default.
    configured: Option<&'static str>,
    /// Whether the deregistration leaves the registration in the group, draining.
    drains: bool,
}

/// A deregistered target drains for exactly as long as the group's
/// `deregistration_delay.timeout_seconds` says, and a group that drains for no time at
/// all releases the registration at once.
///
/// The rows walk the delay: the documented default, the published maximum, a minute, the
/// shortest drain there is, and zero.
#[tokio::test]
async fn a_deregistered_target_drains_for_the_delay_its_target_group_carries() {
    let rows = [
        DrainRow {
            case: "the documented default delay",
            configured: None,
            drains: true,
        },
        DrainRow {
            case: "the published maximum delay",
            configured: Some("3600"),
            drains: true,
        },
        DrainRow {
            case: "a one-minute delay",
            configured: Some("60"),
            drains: true,
        },
        DrainRow {
            case: "the shortest drain there is",
            configured: Some("1"),
            drains: true,
        },
        DrainRow {
            case: "no delay at all",
            configured: Some("0"),
            drains: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // A fresh emulator per row: each row starts from the same two registrations.
        let (router, _engine) = emulator();
        let group = create_http_target_group(&router, &format!("drain-tg-{index}")).await;
        forwarding_load_balancer(&router, &format!("drain-alb-{index}"), &group).await;
        if let Some(configured) = row.configured {
            set_attribute(
                &router,
                row.case,
                &group,
                DEREGISTRATION_DELAY_ATTRIBUTE,
                configured,
            )
            .await;
        }
        register(&router, row.case, &group, &[DRAINED, KEPT]).await;

        deregister(&router, row.case, &group, DRAINED).await;

        // The read immediately after the deregistration, which is what a client that
        // drains connections polls.
        let reported = health_of(&router, row.case, &group, DRAINED).await;
        let registered: Vec<String> = health(&router, row.case, &group)
            .await
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        if row.drains {
            assert_eq!(
                reported,
                ("draining".to_string(), "Target.DeregistrationInProgress".to_string()),
                "case {:?}: a target the load balancer was sending traffic to drains before \
                 it is removed, got {reported:?}",
                row.case
            );
            assert_eq!(
                registered,
                vec![DRAINED.to_string(), KEPT.to_string()],
                "case {:?}: a draining registration is still in the group, got {registered:?}",
                row.case
            );
        } else {
            assert_eq!(
                reported,
                ("unused".to_string(), "Target.NotRegistered".to_string()),
                "case {:?}: with nothing to drain the registration is gone at once, got \
                 {reported:?}",
                row.case
            );
            assert_eq!(
                registered,
                vec![KEPT.to_string()],
                "case {:?}: only the deregistered registration is gone, got {registered:?}",
                row.case
            );
        }

        // Whatever the delay, the registration the request did not name is untouched.
        let kept = health_of(&router, row.case, &group, KEPT).await;
        assert!(
            kept.0 == "initial" || kept.0 == "healthy",
            "case {:?}: the registration nothing deregistered is coming up or up, got {kept:?}",
            row.case
        );
    }
}

/// When the configured delay runs out the registration LEAVES the group, with nothing
/// having called in the meantime — and a second deregistration does not restart the drain.
///
/// The rows are the two shortest delays a drain can be configured for, so the removal is
/// observable inside a test, crossed with how many deregistrations the registration gets.
#[tokio::test]
async fn a_drain_ends_by_the_clock_and_a_second_deregistration_does_not_restart_it() {
    for (case, delay, deregistrations) in [
        ("a one-second drain, deregistered once", 1_u64, 1_usize),
        ("a two-second drain, deregistered once", 2, 1),
        ("a one-second drain, deregistered twice", 1, 2),
    ] {
        let (router, _engine) = emulator();
        let group = create_http_target_group(&router, "drain-clock-tg").await;
        forwarding_load_balancer(&router, "drain-clock-alb", &group).await;
        set_attribute(
            &router,
            case,
            &group,
            DEREGISTRATION_DELAY_ATTRIBUTE,
            &delay.to_string(),
        )
        .await;
        register(&router, case, &group, &[DRAINED, KEPT]).await;
        deregister(&router, case, &group, DRAINED).await;

        for repeat in 1..deregistrations {
            // A repeat partway through the drain: the instant the first one set stands,
            // so the removal still comes due at the same time.
            std::thread::sleep(Duration::from_millis(200 * repeat as u64));
            deregister(&router, case, &group, DRAINED).await;
            let during = health_of(&router, case, &group, DRAINED).await;
            assert_eq!(
                during.0, "draining",
                "case {case:?}: the drain is still running, got {during:?}"
            );
        }

        wait_out(Duration::from_secs(delay));
        let after = health_of(&router, case, &group, DRAINED).await;
        assert_eq!(
            after,
            ("unused".to_string(), "Target.NotRegistered".to_string()),
            "case {case:?}: the delay ran out, so the registration left the group, got {after:?}"
        );
        let registered: Vec<String> = health(&router, case, &group)
            .await
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(
            registered,
            vec![KEPT.to_string()],
            "case {case:?}: the group holds only the registration nothing deregistered, got \
             {registered:?}"
        );
    }
}

/// A target group no load balancer forwards to has no connections to drain, so a
/// deregistration releases the registration at once however long the configured delay is
/// — and a registration that was draining and is REGISTERED AGAIN is back in service: it
/// is checked from the new registration rather than removed when the old drain runs out.
#[tokio::test]
async fn nothing_to_drain_releases_at_once_and_re_registering_returns_a_target_to_service() {
    let (router, _engine) = emulator();

    // No load balancer forwards to this group, and its delay is the published maximum.
    let detached = create_http_target_group(&router, "detached-tg").await;
    set_attribute(
        &router,
        "a group no load balancer forwards to",
        &detached,
        DEREGISTRATION_DELAY_ATTRIBUTE,
        "3600",
    )
    .await;
    register(
        &router,
        "a group no load balancer forwards to",
        &detached,
        &[DRAINED],
    )
    .await;
    let before = health_of(
        &router,
        "a group no load balancer forwards to",
        &detached,
        DRAINED,
    )
    .await;
    assert_eq!(
        before,
        ("unused".to_string(), "Target.NotInUse".to_string()),
        "case a group no load balancer forwards to: nothing is checking its targets, got \
         {before:?}"
    );
    deregister(
        &router,
        "a group no load balancer forwards to",
        &detached,
        DRAINED,
    )
    .await;
    let registered = health(&router, "a group no load balancer forwards to", &detached).await;
    assert!(
        registered.is_empty(),
        "case a group no load balancer forwards to: the registration is gone at once, got \
         {registered:?}"
    );

    // And a group a load balancer forwards to, whose draining target is registered again.
    let group = create_http_target_group(&router, "re-register-tg").await;
    forwarding_load_balancer(&router, "re-register-alb", &group).await;
    set_attribute(
        &router,
        "a draining target registered again",
        &group,
        DEREGISTRATION_DELAY_ATTRIBUTE,
        "1",
    )
    .await;
    register(&router, "a draining target registered again", &group, &[DRAINED]).await;
    deregister(&router, "a draining target registered again", &group, DRAINED).await;
    let draining = health_of(&router, "a draining target registered again", &group, DRAINED).await;
    assert_eq!(
        draining.0, "draining",
        "case a draining target registered again: it drains first, got {draining:?}"
    );

    register(&router, "a draining target registered again", &group, &[DRAINED]).await;
    let back = health_of(&router, "a draining target registered again", &group, DRAINED).await;
    assert_eq!(
        back,
        ("initial".to_string(), "Elb.RegistrationInProgress".to_string()),
        "case a draining target registered again: the new registration is coming up, got {back:?}"
    );
    // Past the drain the OLD deregistration owed, the registration is still there: the
    // new registration is not removed by the drain it replaced.
    wait_out(Duration::from_secs(1));
    let settled = health_of(&router, "a draining target registered again", &group, DRAINED).await;
    assert_eq!(
        settled,
        ("healthy".to_string(), String::new()),
        "case a draining target registered again: it is in service, got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// The settling span: when health checking begins.
// ---------------------------------------------------------------------------

/// Health checking on a registration begins when the target group first becomes one a
/// load balancer forwards to, so the first state reported after the attach is `initial`
/// however long the registration has been waiting — and only once the settling span has
/// passed is the target `healthy`.
///
/// The rows are the two ways an association is made: a listener's own default action, and
/// a RULE of a listener that defaults to something else.
#[tokio::test]
async fn health_checking_begins_when_a_load_balancer_first_forwards_to_the_group() {
    for (index, (case, by_rule)) in [
        ("attached by a listener's default action", false),
        ("attached by a rule of a listener", true),
    ]
    .into_iter()
    .enumerate()
    {
        let (router, _engine) = emulator();
        let group = create_http_target_group(&router, &format!("attach-tg-{index}")).await;
        register(&router, case, &group, &[DRAINED, KEPT]).await;

        // Nothing forwards to the group yet, so nothing is checking its targets — and the
        // registration's own settling span passes while that is still true.
        let detached = health(&router, case, &group).await;
        for (id, state, reason) in &detached {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                ("unused", "Target.NotInUse"),
                "case {case:?}: with nothing forwarding to the group, {id} is unused, got \
                 {state:?}/{reason:?}"
            );
        }
        wait_out(SETTLE_AFTER);

        // The attach is what starts the health checks.
        let lb = create_load_balancer(&router, &format!("attach-alb-{index}")).await;
        if by_rule {
            // The listener defaults to another target group, so only the RULE forwards to
            // the group under test.
            let listener_group =
                create_http_target_group(&router, &format!("attach-default-tg-{index}")).await;
            let listener = create_listener(&router, &lb, &listener_group, 80).await;
            add_rule(
                &router,
                case,
                &listener,
                10,
                &[host_header("attach.example.com"), forward_to(&group)].concat(),
            )
            .await;
        } else {
            create_listener(&router, &lb, &group, 80).await;
        }

        let attached = health(&router, case, &group).await;
        for (id, state, reason) in &attached {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                ("initial", "Elb.RegistrationInProgress"),
                "case {case:?}: the first state after the attach is the registration coming \
                 up, for {id}, got {state:?}/{reason:?}"
            );
        }

        // Once the span has passed, a read materializes the move without anything having
        // written.
        wait_out(SETTLE_AFTER);
        let settled = health(&router, case, &group).await;
        for (id, state, reason) in &settled {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                ("healthy", ""),
                "case {case:?}: past the settling span {id} is in service, got \
                 {state:?}/{reason:?}"
            );
        }

        // A SECOND LISTENER forwarding to the same group does not restart the span: the
        // group was already one a load balancer sends traffic to. It is a listener of the
        // SAME load balancer, because a target group is routed to by at most one load
        // balancer (`all/listener_port_and_target_group_exclusivity.rs`).
        create_listener(&router, &lb, &group, 8080).await;
        let still = health(&router, case, &group).await;
        for (id, state, reason) in &still {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                ("healthy", ""),
                "case {case:?}: a second listener does not re-start health checking of \
                 {id}, got {state:?}/{reason:?}"
            );
        }
    }
}
