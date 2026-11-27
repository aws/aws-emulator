// SPDX-License-Identifier: CC0-1.0

//! The members Amazon ECS answers with that the published model snapshot does not declare,
//! and the overlay model this crate declares them from.
//!
//! The rule under test: a member AWS reports is reported by this service too, whether or not
//! the snapshot declares it — and a member the snapshot declares with a `@default` is
//! satisfied by that default when a request omits it, even where the snapshot also marks it
//! `@required`. The snapshot is kept exactly as shipped, so both halves live in the overlay
//! `smithy-model/local-additions.json`, which restates the shapes concerned (see
//! `smithy-model/LOCAL-ADDITIONS.md`).
//!
//! That overlay is what those types are generated from, so anything it restates of the
//! snapshot — a member's target, its `@default`, a shape's type, an enum's value set, a
//! constraint trait — must still BE what the snapshot says, or what this service parses and
//! reports would be narrower or wider than the model with nothing else noticing. These tests
//! pin exactly that, declaration for declaration:
//!
//! * every member the snapshot declares is restated, with the same target and the same
//!   `@default`, and marked `@required` exactly where the snapshot marks it required AND
//!   gives it no default — the one deliberate difference, which is the relaxation above;
//! * the members the overlay ADDS are exactly the recorded ones listed per shape below, so a
//!   refreshed snapshot that declares one of them fails here and the local declaration can
//!   give way to the modelled member;
//! * every shape the overlay restates for those members' targets still carries the
//!   snapshot's type, value set and constraints.
//!
//! The wire half of each added member is asserted by the suite that owns its resource: the
//! deployment counters and `scale`/`stabilityStatus` by [`crate::service_run_state`], the
//! task's provisioned size by [`crate::task_platform_and_sizing`], `scalingApproach` by
//! [`crate::capacity_providers`], `previousStatus` by
//! [`crate::task_definition_deregistration`], and the relaxed deployment-configuration
//! structures by [`crate::accepted_request_classes`]. The last test here drives the two
//! members no other suite reads — the service's own `version` and
//! `extendDeploymentTimePeriod` — over the wire, so every added member is covered somewhere.

use serde_json::{json, Map, Value};

use crate::ecs_wire::{cluster_with_capacity, list, ok, register_definition, service, string};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declarations are checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json"
));

/// The overlay model the local declarations are generated from, embedded for the same
/// reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shapes this crate declares from the overlay, each with the members AWS answers with
/// that the snapshot does not declare — recorded behavior, and the whole of what the overlay
/// may add to that shape.
///
/// The four deployment-configuration structures add no member at all: they are restated for
/// the `@default` relaxation alone.
const LOCAL_SHAPES: &[(&str, &[&str])] = &[
    (
        "com.amazonaws.ecs#Service",
        &["version", "runStatus", "extendDeploymentTimePeriod"],
    ),
    (
        "com.amazonaws.ecs#Deployment",
        &[
            "healthyTaskCount",
            "failedLaunchTaskCount",
            "replacedTaskCount",
            "totalFailedTasks",
            "scale",
            "stabilityStatus",
        ],
    ),
    (
        "com.amazonaws.ecs#Task",
        &["provisionedCpu", "provisionedMemory"],
    ),
    ("com.amazonaws.ecs#TaskDefinition", &["previousStatus"]),
    ("com.amazonaws.ecs#ManagedScaling", &["scalingApproach"]),
    ("com.amazonaws.ecs#DeploymentCircuitBreaker", &[]),
    ("com.amazonaws.ecs#ThresholdConfiguration", &[]),
    ("com.amazonaws.ecs#DeploymentAlarms", &[]),
    ("com.amazonaws.ecs#DeploymentEarlySuccessCriteria", &[]),
];

/// The `@default` trait id, which decides both whether a member may be omitted and what it
/// then carries.
const DEFAULT_TRAIT: &str = "smithy.api#default";

/// The `@required` trait id, the one the overlay drops from a member that has a default.
const REQUIRED_TRAIT: &str = "smithy.api#required";

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Map<String, Value> {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    parsed["shapes"]
        .as_object()
        .expect("a model document must carry a shapes object")
        .clone()
}

/// The `members` object of one structure declaration.
fn members_of(shape_id: &str, declaration: &Value, which: &str) -> Map<String, Value> {
    declaration["members"]
        .as_object()
        .unwrap_or_else(|| panic!("{which} must declare {shape_id} with members"))
        .clone()
}

/// Whether `member` carries trait `trait_id`.
fn has_trait(member: &Value, trait_id: &str) -> bool {
    member["traits"]
        .as_object()
        .is_some_and(|traits| traits.contains_key(trait_id))
}

/// The value of trait `trait_id` on `member`, or `Value::Null`.
fn trait_value(member: &Value, trait_id: &str) -> Value {
    member["traits"]
        .get(trait_id)
        .cloned()
        .unwrap_or(Value::Null)
}

/// The values of one enum declaration, sorted — the set codegen renders and validation
/// enforces. The member NAMES are not compared: they name nothing on the wire.
fn enum_values(declaration: &Value) -> Vec<String> {
    let mut values: Vec<String> = members_of("an enum", declaration, "a model")
        .values()
        .map(|member| {
            trait_value(member, "smithy.api#enumValue")
                .as_str()
                .expect("an enum member carries its value")
                .to_string()
        })
        .collect();
    values.sort();
    values
}

#[test]
fn the_overlay_restates_every_member_the_snapshot_declares() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);

    for (shape_id, added) in LOCAL_SHAPES {
        let modelled = snapshot
            .get(*shape_id)
            .unwrap_or_else(|| panic!("the snapshot no longer declares {shape_id}"));
        let declared = overlay
            .get(*shape_id)
            .unwrap_or_else(|| panic!("the overlay must declare {shape_id}"));
        let modelled_members = members_of(shape_id, modelled, "the snapshot");
        let declared_members = members_of(shape_id, declared, "the overlay");

        for (member, modelled_member) in &modelled_members {
            let declared_member = declared_members.get(member).unwrap_or_else(|| {
                panic!("{shape_id}${member} is declared by the snapshot and not restated")
            });
            assert_eq!(
                declared_member["target"], modelled_member["target"],
                "{shape_id}${member} must be restated with the snapshot's target {}, got {}",
                modelled_member["target"], declared_member["target"]
            );
            assert_eq!(
                trait_value(declared_member, DEFAULT_TRAIT),
                trait_value(modelled_member, DEFAULT_TRAIT),
                "{shape_id}${member} must be restated with the snapshot's default {}, got {}",
                trait_value(modelled_member, DEFAULT_TRAIT),
                trait_value(declared_member, DEFAULT_TRAIT)
            );
            // The one deliberate difference: a member with a default may be omitted, so the
            // overlay drops `@required` from it and keeps it on every member without one.
            let defaulted = has_trait(modelled_member, DEFAULT_TRAIT);
            let expected_required = has_trait(modelled_member, REQUIRED_TRAIT) && !defaulted;
            assert_eq!(
                has_trait(declared_member, REQUIRED_TRAIT),
                expected_required,
                "{shape_id}${member} must be restated {} @required (the snapshot marks it \
                 required: {}, and gives it a default: {defaulted})",
                if expected_required { "with" } else { "without" },
                has_trait(modelled_member, REQUIRED_TRAIT)
            );
        }

        let mut extra: Vec<&str> = declared_members
            .keys()
            .filter(|member| !modelled_members.contains_key(*member))
            .map(String::as_str)
            .collect();
        extra.sort();
        let mut expected: Vec<&str> = added.to_vec();
        expected.sort();
        assert_eq!(
            extra, expected,
            "{shape_id} may add {expected:?} and nothing else; a member the snapshot now \
             declares should give way to the modelled one. Added {extra:?}"
        );
    }
}

#[test]
fn the_overlay_restates_every_target_shape_as_the_snapshot_declares_it() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let local: Vec<&str> = LOCAL_SHAPES.iter().map(|(shape, _)| *shape).collect();

    for (shape_id, declared) in &overlay {
        if local.contains(&shape_id.as_str()) {
            continue;
        }
        let modelled = snapshot.get(shape_id).unwrap_or_else(|| {
            panic!("the overlay restates {shape_id}, which the snapshot does not declare")
        });
        assert_eq!(
            declared["type"], modelled["type"],
            "{shape_id} must be restated with the snapshot's type {}, got {}",
            modelled["type"], declared["type"]
        );
        match declared["type"].as_str() {
            Some("enum") => assert_eq!(
                enum_values(declared),
                enum_values(modelled),
                "{shape_id} must be restated with the snapshot's value set"
            ),
            Some("list") => assert_eq!(
                declared["member"]["target"], modelled["member"]["target"],
                "{shape_id} must be restated with the snapshot's element target {}, got {}",
                modelled["member"]["target"], declared["member"]["target"]
            ),
            // A STRUCTURE target is resolved through the Rust field's type rather than
            // through the model, so the overlay declares it for its type alone.
            Some("structure") => assert!(
                declared["members"].as_object().is_some_and(Map::is_empty),
                "{shape_id} is restated for its type alone, so it must declare no members"
            ),
            _ => {}
        }
        // The constraints codegen and the validating walk read off a restated shape, which
        // decide what this service refuses. Other traits are not compared: a shape-level
        // default is read per MEMBER (restated above), and documentation is prose.
        for constraint in ["smithy.api#range", "smithy.api#length"] {
            assert_eq!(
                trait_value(declared, constraint),
                trait_value(modelled, constraint),
                "{shape_id} must be restated with the snapshot's {constraint} {}, got {}",
                trait_value(modelled, constraint),
                trait_value(declared, constraint)
            );
        }
    }
}

#[tokio::test]
async fn a_service_reports_the_version_and_deployment_time_period_members_it_is_answered_with() {
    let router = service();
    cluster_with_capacity(&router, "localmembers", "setup").await;
    register_definition(&router, "localmembersfamily", "setup").await;

    // The rows differ along the count the service is asked for: the two members are a
    // property of every service, not of one request.
    let rows: [(&str, &str, i64); 3] = [
        ("no tasks", "localzero", 0),
        ("one task", "localone", 1),
        ("two tasks", "localtwo", 2),
    ];

    for (case, name, desired) in rows {
        let request = json!({
            "cluster": "localmembers",
            "serviceName": name,
            "taskDefinition": "localmembersfamily",
            "launchType": "EC2",
            "desiredCount": desired,
        });
        let created = ok(&router, "CreateService", request, case).await;
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": "localmembers", "services": [name] }),
            case,
        )
        .await;
        let described = list(&read, "services", case)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {read}"));

        // Both surfaces, so a member computed for the create's own answer and not stored
        // cannot pass.
        for (surface, service) in [
            ("CreateService", &created["service"]),
            ("DescribeServices", &described),
        ] {
            assert_eq!(
                service["version"].as_i64(),
                Some(0),
                "case {case:?} ({surface}): expected a new service to report version 0, got \
                 {service}"
            );
            assert_eq!(
                service["extendDeploymentTimePeriod"].as_i64(),
                Some(0),
                "case {case:?} ({surface}): nothing extends a deployment's time period, so \
                 the member must read 0, got {service}"
            );
            // The deployment the create started carries the counters the overlay adds to
            // `Deployment`, each a true zero for a rollout that has lost nothing.
            let deployments = list(service, "deployments", case);
            let primary = deployments
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?} ({surface}): a primary deployment"));
            for counter in [
                "healthyTaskCount",
                "failedLaunchTaskCount",
                "replacedTaskCount",
                "totalFailedTasks",
            ] {
                assert!(
                    primary[counter].as_i64().is_some(),
                    "case {case:?} ({surface}): expected the deployment to report {counter}, \
                     got {primary}"
                );
            }
            assert_eq!(
                string(&primary["scale"], "unit", case),
                "PERCENT",
                "case {case:?} ({surface}): expected the deployment's scale in percent, got \
                 {primary}"
            );
            assert!(
                primary["stabilityStatus"].as_str().is_some(),
                "case {case:?} ({surface}): expected the deployment to report a \
                 stabilityStatus, got {primary}"
            );
        }
    }
}
