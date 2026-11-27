// SPDX-License-Identifier: CC0-1.0

//! Which refusal CLASS a managed-scaling block outside its published bounds earns, driven end
//! to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: `ManagedScaling$targetCapacity` is "the target capacity utilization
//! as a percentage for the capacity provider ... must be greater than `0` and less than or
//! equal to `100`", and Amazon ECS answers a value outside that in the class it answers a
//! request whose VALUES it will not take — `ClientException`, with the one fixed sentence
//! `crate::state::MANAGED_SCALING_TARGET_CAPACITY_MESSAGE` states, naming neither the value
//! nor the member — not in the class a request it cannot read earns. So the bound is judged
//! by the operations that take such a block rather than by the framework's boundary walk,
//! which is why the shape's own declaration carries
//! `skip_validation = ["targetCapacity"]` (see `src/ops/types/managed_scaling_types.rs`).
//! A value INSIDE the bounds is served, so the skip relaxes the class and not the bound.
//!
//! The rows span what the rule generalises over: each published bound, one step past each,
//! and the two operations that take a managed-scaling block. The step sizes are the contrast
//! case: their own `@range` IS refused at the boundary, in the other class, so one row pins
//! that the skip reaches `targetCapacity` alone.

use serde_json::json;

use crate::capacity_providers::group_arn;
use crate::ecs_wire::{create_auto_scaling_group, list, ok, refusal, service, string};

/// The class a utilization outside the published range is refused in.
const CLIENT_EXCEPTION: &str = "ClientException";

/// The class a value outside a bound the framework's walk enforces is refused in.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The whole sentence that refusal carries, from the state module that owns it, so the test
/// cannot drift from the service's own wording.
const TARGET_CAPACITY_MESSAGE: &str =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MANAGED_SCALING_TARGET_CAPACITY_MESSAGE;

/// The `autoScalingGroupProvider` a request carries for `group`, targeting `target`.
fn configuration(group: &str, target: i64) -> serde_json::Value {
    json!({
        "autoScalingGroupArn": group_arn(group),
        "managedScaling": { "status": "ENABLED", "targetCapacity": target },
    })
}

#[tokio::test]
async fn a_target_utilization_outside_its_published_range_is_refused_as_a_client_exception() {
    let router = service();
    for group in ["asg-utilmin", "asg-utilmax", "asg-utiledge"] {
        create_auto_scaling_group(&router, group, "setup").await;
    }
    // One provider for the update rows to act on, created inside the range.
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "utilprovider",
            "autoScalingGroupProvider": configuration("asg-utiledge", 50),
        }),
        "setup",
    )
    .await;

    // Each published bound, and one step past each.
    let refused: [(&str, i64); 2] = [
        ("one step below the lowest utilization", 0),
        ("one step past the highest utilization", 101),
    ];
    for (case, target) in refused {
        for (operation, provider, group) in [
            ("CreateCapacityProvider", "utilrefused", "asg-utilmin"),
            ("UpdateCapacityProvider", "utilprovider", "asg-utiledge"),
        ] {
            let (code, message) = refusal(
                &router,
                operation,
                json!({
                    "name": provider,
                    "autoScalingGroupProvider": configuration(group, target),
                }),
                case,
            )
            .await;
            assert_eq!(
                code, CLIENT_EXCEPTION,
                "case {case:?}: expected {operation} to refuse a utilization of {target} with \
                 {CLIENT_EXCEPTION}, got {code:?} saying {message:?}"
            );
            assert_eq!(
                message, TARGET_CAPACITY_MESSAGE,
                "case {case:?}: expected {operation} to carry the service's own sentence, got \
                 {message:?}"
            );
        }
        // Nothing is stored for the refused create.
        let described = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": ["utilrefused"] }),
            case,
        )
        .await;
        assert!(
            list(&described, "capacityProviders", case).is_empty(),
            "case {case:?}: a refused create must store no provider, got {described}"
        );
    }

    // Inside the range, at each bound: served, and the provider reports what it was asked
    // for — so what the declaration relaxes is the refusal's class, not the bound itself.
    for (case, target, provider, group) in [
        ("the lowest utilization", 1_i64, "utilmin", "asg-utilmin"),
        ("the highest utilization", 100, "utilmax", "asg-utilmax"),
    ] {
        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider,
                "autoScalingGroupProvider": configuration(group, target),
            }),
            case,
        )
        .await;
        assert_eq!(
            created["capacityProvider"]["autoScalingGroupProvider"]["managedScaling"]
                ["targetCapacity"]
                .as_i64(),
            Some(target),
            "case {case:?}: expected the provider to target {target}, got {created}"
        );
        assert_eq!(
            string(&created["capacityProvider"], "name", case),
            provider,
            "case {case:?}: expected the provider the create named, got {created}"
        );
    }

    // The contrast case: a step size outside its own `@range` is a bound the framework's
    // walk enforces, so it is refused in the other class — the skip above reaches
    // `targetCapacity` alone.
    let (code, message) = refusal(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "utilstep",
            "autoScalingGroupProvider": {
                "autoScalingGroupArn": group_arn("asg-utilmin"),
                "managedScaling": { "status": "ENABLED", "minimumScalingStepSize": 0 },
            },
        }),
        "a scaling step below its own range",
    )
    .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "expected a step size outside its modelled range to be refused with \
         {INVALID_PARAMETER}, got {code:?} saying {message:?}"
    );
}
