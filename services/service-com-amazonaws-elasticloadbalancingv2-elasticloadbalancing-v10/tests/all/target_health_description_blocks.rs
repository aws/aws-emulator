// SPDX-License-Identifier: CC0-1.0

//! The two members a target health description carries BESIDE the target's health: the
//! administrative override in effect on it, and the anomaly detection a request can ask for.
//!
//! RULE 1: `TargetHealthDescription$AdministrativeOverride` is "the administrative override
//! information for the target", and the states it may report are `no_override` plus the ones
//! a zonal shift puts a target in
//! (`smithy_query(query="shape",shape="TargetAdministrativeOverrideStateEnum")`). So every
//! description a health read reports carries that structure — a client reading it to decide
//! whether a target is administratively drained must be able to tell "no override" from "not
//! reported" — and a target nothing overrides carries the state, the reason code and the
//! description of exactly that.
//!
//! RULE 2: `DescribeTargetHealthInput$Include` is "used to include anomaly detection
//! information", whose values are `AnomalyDetection` and `All`
//! (`smithy_query(query="shape",shape="DescribeTargetHealthInputIncludeEnum")`). So the
//! `AnomalyDetection` block is part of every description of an answer exactly when an option
//! selecting it was named, and absent from an answer that asked for no optional block; its
//! `Result` is "the latest anomaly detection result ... if no anomalies were detected, the
//! result is `normal`" and its `MitigationInEffect` says whether mitigation "is in progress".
//!
//! The rows of the first table span what rule 1 generalises over: the group's target type,
//! whether a load balancer forwards to the group at all, and whether the target the read
//! names is registered, draining or was never registered — so the override is asserted
//! across four of the health states a description can be under. The rows of the second span
//! what rule 2 generalises over: which options the request names, and how many of them.

use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::{create_listener, create_load_balancer};
use crate::provisioning_loop::{account, action, call, emulator, first_text, members, p, REGION};
use crate::target_id_forms_and_reported_ports::target_group_on;

/// The administrative override a target nothing overrides is under: the state, the reason
/// code and the description Elastic Load Balancing words it with.
const NO_OVERRIDE: (&str, &str, &str) = (
    "no_override",
    "AdministrativeOverride.NoOverride",
    "No override is currently active on target",
);

/// The anomaly detection reported for a target no anomaly was detected for: the result and
/// whether mitigation is in progress.
const NO_ANOMALY: (&str, &str) = ("normal", "no");

/// The two `DescribeTargetHealthInputIncludeEnum` values, from the model's own enum
/// (`smithy_query(query="shape",shape="DescribeTargetHealthInputIncludeEnum")`).
const INCLUDE_ANOMALY_DETECTION: &str = "AnomalyDetection";
const INCLUDE_ALL: &str = "All";

/// One description as a caller reads it: the health state it is under, the administrative
/// override triple, and the anomaly-detection pair where the answer carries that block.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    state: Option<String>,
    administrative_override: Option<(Option<String>, Option<String>, Option<String>)>,
    anomaly_detection: Option<(Option<String>, Option<String>)>,
}

/// The descriptions a health read of `arn` reports for the `(id, registered)` targets of
/// `named`, under the `Include` options of `include`.
///
/// Every target is NAMED in the request, so a target the group holds no registration of is
/// described too — "describes the health of the specified targets ... or all of your
/// targets".
async fn reported(
    router: &MultiServiceRouter,
    arn: &str,
    named: &[&str],
    include: &[&str],
) -> Vec<Reported> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    for (index, id) in named.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), id));
    }
    for (index, option) in include.iter().enumerate() {
        form.push(p(&format!("Include.member.{}", index + 1), option));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "a health read must be served, got {} {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            // Each block is read out of its OWN element, so the `State` of the health and
            // the `State` of the override cannot be confused for one another.
            let health = first_text(description, "TargetHealth").unwrap_or_default();
            Reported {
                state: first_text(&health, "State"),
                administrative_override: first_text(description, "AdministrativeOverride").map(
                    |block| {
                        (
                            first_text(&block, "State"),
                            first_text(&block, "Reason"),
                            first_text(&block, "Description"),
                        )
                    },
                ),
                anomaly_detection: first_text(description, "AnomalyDetection").map(|block| {
                    (
                        first_text(&block, "Result"),
                        first_text(&block, "MitigationInEffect"),
                    )
                }),
            }
        })
        .collect()
}

/// The override triple the rules expect, as a reader reports one.
fn no_override() -> Option<(Option<String>, Option<String>, Option<String>)> {
    Some((
        Some(NO_OVERRIDE.0.to_string()),
        Some(NO_OVERRIDE.1.to_string()),
        Some(NO_OVERRIDE.2.to_string()),
    ))
}

/// The anomaly-detection pair the rules expect for a target no anomaly was detected for.
fn no_anomaly() -> Option<(Option<String>, Option<String>)> {
    Some((
        Some(NO_ANOMALY.0.to_string()),
        Some(NO_ANOMALY.1.to_string()),
    ))
}

/// Where the target of one row stands: what the group it is in is attached to, and what has
/// been done to its registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Registered with a target group no load balancer forwards to.
    InAnUnattachedGroup,
    /// Registered with a target group an HTTP listener forwards to.
    InAForwardedGroup,
    /// Registered with such a group and then deregistered, so it is draining.
    Draining,
    /// Named in the read and never registered at all.
    NeverRegistered,
}

/// One row of the override table: the case, the group's target type, the target id, where
/// the target stands and the health state it is then reported under.
struct OverrideRow {
    case: &'static str,
    target_type: &'static str,
    id: String,
    standing: Standing,
    state: &'static str,
}

/// Register `id` with the group at `arn`, which must be accepted.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, id: &str) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", id));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the registration must be served, got {} {}",
        reply.status, reply.body
    );
}

/// Deregister `id` from the group at `arn`, which must be accepted.
async fn deregister(router: &MultiServiceRouter, case: &str, arn: &str, id: &str) {
    let mut form = action("DeregisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", id));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the deregistration must be served, got {} {}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn every_reported_target_carries_the_override_in_effect_on_it() {
    let (router, _engine) = emulator();
    // A Lambda function ARN of this account, which is what a target of a `lambda` group is
    // written as.
    let lambda_target = format!("arn:aws:lambda:{REGION}:{}:function:greeter", account());

    let rows: [OverrideRow; 5] = [
        OverrideRow {
            case: "a lambda target of a group nothing forwards to",
            target_type: "lambda",
            id: lambda_target.clone(),
            standing: Standing::InAnUnattachedGroup,
            state: "unused",
        },
        OverrideRow {
            case: "a lambda target of a forwarded group whose health checks are off",
            target_type: "lambda",
            id: lambda_target,
            standing: Standing::InAForwardedGroup,
            // A `lambda` target group carries health checking off unless a caller asks for
            // it, so its registration's health is the one a group with no checks reports.
            state: "unavailable",
        },
        OverrideRow {
            case: "an address target of a group a listener forwards to",
            target_type: "ip",
            id: "10.0.0.7".to_string(),
            standing: Standing::InAForwardedGroup,
            state: "initial",
        },
        OverrideRow {
            case: "an address target that is draining",
            target_type: "ip",
            id: "10.0.0.8".to_string(),
            standing: Standing::Draining,
            state: "draining",
        },
        OverrideRow {
            case: "an instance target the group holds no registration of",
            target_type: "instance",
            id: "i-0123456789abcdef0".to_string(),
            standing: Standing::NeverRegistered,
            state: "unused",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let arn = target_group_on(
            &router,
            row.case,
            &format!("tg-override-{index}"),
            row.target_type,
            // A target type whose targets are invoked rather than connected to names
            // neither a protocol nor a port of its own.
            (row.target_type != "lambda").then_some("80"),
            None,
        )
        .await;
        if row.standing != Standing::InAnUnattachedGroup {
            let load_balancer = create_load_balancer(&router, &format!("lb-override-{index}")).await;
            create_listener(&router, &load_balancer, &arn, 80).await;
        }
        if row.standing != Standing::NeverRegistered {
            register(&router, row.case, &arn, &row.id).await;
        }
        if row.standing == Standing::Draining {
            deregister(&router, row.case, &arn, &row.id).await;
        }

        let described = reported(&router, &arn, &[&row.id], &[]).await;
        assert_eq!(
            described.len(),
            1,
            "case {:?}: expected the one named target described, got {described:?}",
            row.case
        );
        assert_eq!(
            (
                described[0].state.as_deref(),
                described[0].administrative_override.clone()
            ),
            (Some(row.state), no_override()),
            "case {:?}: expected the state {:?} under no administrative override, got {:?}",
            row.case,
            row.state,
            described[0]
        );
    }
}

/// One row of the include table: the case, the options the request names, and whether the
/// anomaly-detection block is then part of every description.
struct IncludeRow {
    case: &'static str,
    options: &'static [&'static str],
    reports_anomaly_detection: bool,
}

#[tokio::test]
async fn the_anomaly_detection_block_is_reported_exactly_where_the_include_options_ask_for_it() {
    let (router, _engine) = emulator();
    let arn = target_group_on(
        &router,
        "the group every include row reads",
        "tg-include",
        "ip",
        Some("80"),
        None,
    )
    .await;
    let load_balancer = create_load_balancer(&router, "lb-include").await;
    create_listener(&router, &load_balancer, &arn, 80).await;
    let registered = "10.0.0.21";
    // A target the group holds no registration of, named beside the registered one, so each
    // row asserts the block over BOTH kinds of description one answer can carry.
    let unregistered = "10.0.0.22";
    register(&router, "the group every include row reads", &arn, registered).await;

    let rows: [IncludeRow; 5] = [
        IncludeRow {
            case: "no include option at all",
            options: &[],
            reports_anomaly_detection: false,
        },
        IncludeRow {
            case: "the anomaly-detection option",
            options: &[INCLUDE_ANOMALY_DETECTION],
            reports_anomaly_detection: true,
        },
        IncludeRow {
            case: "the option that selects every block",
            options: &[INCLUDE_ALL],
            reports_anomaly_detection: true,
        },
        IncludeRow {
            case: "both options at once",
            options: &[INCLUDE_ALL, INCLUDE_ANOMALY_DETECTION],
            reports_anomaly_detection: true,
        },
        IncludeRow {
            case: "the anomaly-detection option named twice",
            options: &[INCLUDE_ANOMALY_DETECTION, INCLUDE_ANOMALY_DETECTION],
            reports_anomaly_detection: true,
        },
    ];

    for row in &rows {
        let described = reported(&router, &arn, &[registered, unregistered], row.options).await;
        assert_eq!(
            described.len(),
            2,
            "case {:?}: expected both named targets described, got {described:?}",
            row.case
        );
        let expected = row.reports_anomaly_detection.then(no_anomaly).flatten();
        for (position, description) in described.iter().enumerate() {
            assert_eq!(
                description.anomaly_detection.clone(),
                expected.clone(),
                "case {:?}: expected the anomaly detection {expected:?} on description \
                 {position}, got {description:?}",
                row.case
            );
            // The override is no optional block: it is reported whatever the options say.
            assert_eq!(
                description.administrative_override.clone(),
                no_override(),
                "case {:?}: expected the override on description {position}, got {description:?}",
                row.case
            );
        }
    }
}
