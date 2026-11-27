// SPDX-License-Identifier: CC0-1.0

//! What a CALL answers with and what the SCHEDULER answers with, for the service scheduler
//! and the capacity providers it places through.
//!
//! The rules under test:
//!
//! 1. A mutating call reports what the CALLER asked for: `Service$desiredCount` takes the
//!    new count at once. What the SCHEDULER is working to is its own
//!    (`Deployment$desiredCount` is "the most recent desired count of tasks that was
//!    specified for the service to deploy or maintain", and
//!    `ServiceCurrentRevisionSummary$requestedTaskCount` is "the number of requested tasks
//!    in the current service revision"), and it is taken on by the scheduler's own pass —
//!    so the call that gives the scheduler work is answered with the count it was still
//!    working to, and the calls after it read the count it has taken on. The scheduler's
//!    account of a placement it could not make (`Service$events`) is the same: it belongs
//!    to the pass, not to the call.
//! 2. `Service$runStatus` reports the stall while EITHER count is unmet, so a caller that
//!    scales away from a count the scheduler has already taken on still sees it.
//! 3. `Service$availabilityZoneRebalancing` on a create that names none is the member's
//!    documented create default, `ENABLED`, wherever the service is placed — and an update
//!    that names none resolves to the value the service carries.
//! 4. An Auto Scaling group capacity provider is stored with the managed-scaling members
//!    and the managed-draining option Amazon ECS applies to a create that names none.
//! 5. Moving a service onto a capacity provider strategy replaces the tasks it placed
//!    under what it is being taken off, which only a new deployment does, so the switch
//!    needs `forceNewDeployment`.
//! 6. A `serviceDeploymentArn` ends in a GENERATED id: a fixed-width base62 token, one per
//!    rollout.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so a member that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, count, create_auto_scaling_group, create_cluster, list, ok,
    present_count, refusal, register_definition, service, string,
};

/// The fixed part of the Amazon ECS service event message for a placement that found no
/// instance to run on.
const UNABLE_TO_PLACE: &str =
    "was unable to place a task because no container instance met all of its requirements.";

/// `Service$runStatus` of a service whose cluster holds no container instance to place its
/// tasks on.
const ERROR_CLUSTER_EMPTY: &str = "Error.ClusterEmpty";

/// The sentence an update that switches a service onto a capacity provider strategy without
/// forcing a new deployment is refused with.
const NEEDS_FORCED_DEPLOYMENT: &str =
    "When switching from launch type to capacity provider strategy on an existing service, \
     or making a change to a capacity provider strategy on a service that is already using \
     one, you must force a new deployment.";

/// Describe one service.
async fn describe(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected one described service, got {body}"))
}

/// The `(deployment desiredCount, current revision requestedTaskCount)` a service view
/// reports — the two members the scheduler's own count is reported through.
fn scheduled_counts(service_view: &Value, case: &str) -> (i64, i64) {
    let deployments = list(service_view, "deployments", case);
    let revisions = list(service_view, "currentServiceRevisions", case);
    (
        count(
            deployments
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: expected one deployment")),
            "desiredCount",
            case,
        ),
        count(
            revisions
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: expected one service revision")),
            "requestedTaskCount",
            case,
        ),
    )
}

/// One row of the scheduler-commitment table: a call that changes what a service wants.
struct CountRow {
    case: &'static str,
    /// The count the service is created wanting.
    created: i64,
    /// The count the update asks for, or `None` for a row that makes no update.
    updated: Option<i64>,
    /// What the LAST call's own response must report for the scheduler's count.
    answered_with: i64,
    /// What a read after it must report, once the scheduler's pass has taken the count on.
    taken_on: i64,
}

/// The scheduler's count is a step behind the caller's: the call that changes what the
/// service wants is answered with the count the scheduler was working to, and the read after
/// it reports the one it has taken on. The rows span the direction of the change — a create
/// that asks for tasks up front, a scale-up, a scale-down to zero, and a re-statement of the
/// count the service already holds, which changes nothing at all.
#[tokio::test]
async fn the_scheduler_reports_the_count_it_has_taken_on_a_step_behind_the_caller() {
    let rows: [CountRow; 5] = [
        CountRow {
            case: "a create that asks for two tasks",
            created: 2,
            updated: None,
            answered_with: 0,
            taken_on: 2,
        },
        CountRow {
            case: "a scale-up from zero",
            created: 0,
            updated: Some(1),
            answered_with: 0,
            taken_on: 1,
        },
        CountRow {
            case: "a scale-up from three to five",
            created: 3,
            updated: Some(5),
            answered_with: 3,
            taken_on: 5,
        },
        CountRow {
            case: "a scale-down to zero",
            created: 2,
            updated: Some(0),
            answered_with: 2,
            taken_on: 0,
        },
        CountRow {
            case: "an update re-stating the count the service holds",
            created: 1,
            updated: Some(1),
            answered_with: 1,
            taken_on: 1,
        },
    ];

    let router = service();
    register_definition(&router, "commitfamily", "setup").await;
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let cluster = format!("commit{index}");
        create_cluster(&router, &cluster, case).await;
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": "web",
                "taskDefinition": "commitfamily",
                "desiredCount": row.created,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
        let mut last = created["service"].clone();
        if row.updated.is_none() {
            assert_eq!(
                count(&last, "desiredCount", case),
                row.created,
                "case {case:?}: the service wants what the create asked for, got {last}"
            );
        }
        if let Some(target) = row.updated {
            // The read between the two calls is what lets the scheduler's pass take the
            // create's count on, so the update's own answer is about the UPDATE's step.
            describe(&router, &cluster, "web", case).await;
            let updated = ok(
                &router,
                "UpdateService",
                json!({ "cluster": cluster, "service": "web", "desiredCount": target }),
                case,
            )
            .await;
            last = updated["service"].clone();
            assert_eq!(
                count(&last, "desiredCount", case),
                target,
                "case {case:?}: the service wants what the update asked for, got {last}"
            );
        }
        assert_eq!(
            scheduled_counts(&last, case),
            (row.answered_with, row.answered_with),
            "case {case:?}: the call is answered with the count the scheduler was working \
             to, got {last}"
        );
        let read = describe(&router, &cluster, "web", case).await;
        assert_eq!(
            scheduled_counts(&read, case),
            (row.taken_on, row.taken_on),
            "case {case:?}: the read reports the count the scheduler has taken on, got \
             {read}"
        );
    }
}

/// The scheduler's own account of a placement it could NOT make belongs to its pass: the
/// call that gave it the work carries no such event, the read after it does, and the error
/// code stays while either count is unmet — including after the caller has scaled away from
/// a count the scheduler has already taken on.
#[tokio::test]
async fn the_placement_failure_event_belongs_to_the_scheduler_pass_not_to_the_call() {
    let case = "an EC2 service on a cluster with no capacity";
    let router = service();
    let cluster = "shortfallpass";
    register_definition(&router, "passfamily", case).await;
    create_cluster(&router, cluster, case).await;

    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "web",
            "taskDefinition": "passfamily",
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let created = created["service"].clone();
    assert!(
        list(&created, "events", case).is_empty(),
        "case {case:?}: the create is answered before the scheduler has tried, got {created}"
    );
    assert_eq!(
        string(&created, "runStatus", case),
        ERROR_CLUSTER_EMPTY,
        "case {case:?}: the count just asked for is already unmet, got {created}"
    );

    let read = describe(&router, cluster, "web", case).await;
    let messages: Vec<String> = list(&read, "events", case)
        .iter()
        .map(|event| string(event, "message", case))
        .collect();
    assert!(
        messages.len() == 1 && messages[0].contains(UNABLE_TO_PLACE),
        "case {case:?}: the scheduler's pass writes exactly its placement-failure event, \
         got {messages:?}"
    );

    // Scaling back to zero leaves the count the scheduler has TAKEN ON unmet, so the stall
    // is still reported — by the update's own answer and by the read after it.
    let case = "scaled back to zero while the scheduler still owes the task";
    let updated = ok(
        &router,
        "UpdateService",
        json!({ "cluster": cluster, "service": "web", "desiredCount": 0 }),
        case,
    )
    .await;
    let updated = updated["service"].clone();
    assert_eq!(
        (
            count(&updated, "desiredCount", case),
            string(&updated, "runStatus", case),
        ),
        (0, ERROR_CLUSTER_EMPTY.to_string()),
        "case {case:?}: the stall stands until the scheduler has taken the new count on, \
         got {updated}"
    );
    let read = describe(&router, cluster, "web", case).await;
    assert!(
        read["runStatus"]
            .as_str()
            .is_none_or(|status| !status.starts_with("Error.")),
        "case {case:?}: a service that wants nothing is short of nothing, got {read}"
    );
}

/// One row of the Availability Zone rebalancing table.
struct RebalancingRow {
    case: &'static str,
    /// The `launchType` the create names: where the service's tasks are placed, which the
    /// create's own default for the member does not depend on.
    launch_type: &'static str,
    /// What the CREATE names, or `None` for a create that names none.
    created_with: Option<&'static str>,
    /// What the UPDATE names, or `None` for an update that names none.
    updated_with: Option<&'static str>,
    /// What the create must report.
    expect_created: &'static str,
    /// What the update, and every read after it, must report.
    expect_updated: &'static str,
}

/// `Service$availabilityZoneRebalancing`: "For create service requests, when no value is
/// specified for AvailabilityZoneRebalancing, Amazon ECS defaults the value to ENABLED. For
/// update service requests, when no value is specified ... Amazon ECS defaults to the
/// existing service's AvailabilityZoneRebalancing value. If the service never had an
/// AvailabilityZoneRebalancing value set, Amazon ECS treats this as DISABLED." The create
/// default carries no condition on it, so a create that names no value leaves the service
/// ENABLED wherever its tasks are placed; either way the value the service CARRIES is what an
/// update that names none keeps. The rows span which of the two calls names a value, each
/// value of the enum (`smithy_query(query="shape", shape="AvailabilityZoneRebalancing")` =
/// [ENABLED, DISABLED]), and where the service is placed — which the create default does not
/// turn on.
#[tokio::test]
async fn availability_zone_rebalancing_defaults_on_a_create_and_resolves_on_an_update() {
    let rows: [RebalancingRow; 7] = [
        RebalancingRow {
            case: "neither call names a value, on capacity of the cluster's own",
            launch_type: "EC2",
            created_with: None,
            updated_with: None,
            expect_created: "ENABLED",
            expect_updated: "ENABLED",
        },
        RebalancingRow {
            case: "neither call names a value, on Fargate",
            launch_type: "FARGATE",
            created_with: None,
            updated_with: None,
            expect_created: "ENABLED",
            expect_updated: "ENABLED",
        },
        RebalancingRow {
            case: "the create sets it and the update names none",
            launch_type: "EC2",
            created_with: Some("ENABLED"),
            updated_with: None,
            expect_created: "ENABLED",
            expect_updated: "ENABLED",
        },
        RebalancingRow {
            case: "the create disables it and the update names none",
            launch_type: "EC2",
            created_with: Some("DISABLED"),
            updated_with: None,
            expect_created: "DISABLED",
            expect_updated: "DISABLED",
        },
        RebalancingRow {
            case: "the update disables what the create defaulted to",
            launch_type: "EC2",
            created_with: None,
            updated_with: Some("DISABLED"),
            expect_created: "ENABLED",
            expect_updated: "DISABLED",
        },
        RebalancingRow {
            case: "the update disables what the create set",
            launch_type: "EC2",
            created_with: Some("ENABLED"),
            updated_with: Some("DISABLED"),
            expect_created: "ENABLED",
            expect_updated: "DISABLED",
        },
        RebalancingRow {
            case: "the update sets what a Fargate create had disabled",
            launch_type: "FARGATE",
            created_with: Some("DISABLED"),
            updated_with: Some("ENABLED"),
            expect_created: "DISABLED",
            expect_updated: "ENABLED",
        },
    ];

    let router = service();
    let cluster = "rebalancing";
    create_cluster(&router, cluster, "setup").await;
    register_definition(&router, "rebalancefamily", "setup").await;
    // A service placed on Fargate needs a revision registered for it: `awsvpc`, at one of
    // the published task sizes.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "rebalancefargate",
            "requiresCompatibilities": ["FARGATE"],
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "web",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "cpu": 0,
                "memory": 128,
            }],
        }),
        "setup",
    )
    .await;
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let service_name = format!("web{index}");
        let on_fargate = row.launch_type == "FARGATE";
        let mut request = json!({
            "cluster": cluster,
            "serviceName": service_name,
            "taskDefinition": if on_fargate { "rebalancefargate" } else { "rebalancefamily" },
            "desiredCount": 0,
            "launchType": row.launch_type,
        });
        if on_fargate {
            request["networkConfiguration"] = awsvpc_network_configuration();
        }
        if let Some(requested) = row.created_with {
            request["availabilityZoneRebalancing"] = json!(requested);
        }
        let created = ok(&router, "CreateService", request, case).await;
        assert_eq!(
            string(&created["service"], "availabilityZoneRebalancing", case),
            row.expect_created,
            "case {case:?}: unexpected availabilityZoneRebalancing on the create, got \
             {created}"
        );

        let mut request = json!({ "cluster": cluster, "service": service_name });
        if let Some(requested) = row.updated_with {
            request["availabilityZoneRebalancing"] = json!(requested);
        }
        let updated = ok(&router, "UpdateService", request, case).await;
        assert_eq!(
            string(&updated["service"], "availabilityZoneRebalancing", case),
            row.expect_updated,
            "case {case:?}: unexpected availabilityZoneRebalancing on the update, got \
             {updated}"
        );
        let read = describe(&router, cluster, &service_name, case).await;
        assert_eq!(
            string(&read, "availabilityZoneRebalancing", case),
            row.expect_updated,
            "case {case:?}: the read must report what the update left, got {read}"
        );
        // A SECOND update that names nothing changes nothing either: the value a service
        // carries is the one the next update defaults to, however many updates it has seen.
        let repeated = ok(
            &router,
            "UpdateService",
            json!({ "cluster": cluster, "service": service_name, "desiredCount": 0 }),
            case,
        )
        .await;
        assert_eq!(
            string(&repeated["service"], "availabilityZoneRebalancing", case),
            row.expect_updated,
            "case {case:?}: a repeated update keeps the service's value, got {repeated}"
        );
        // And the last copy of the record a client gets reports the same value: deleting a
        // service changes none of its configuration.
        let deleted = ok(
            &router,
            "DeleteService",
            json!({ "cluster": cluster, "service": service_name }),
            case,
        )
        .await;
        assert_eq!(
            string(&deleted["service"], "availabilityZoneRebalancing", case),
            row.expect_updated,
            "case {case:?}: the deleted record reports the same value, got {deleted}"
        );
    }
}

/// One row of the capacity-provider scaling table.
struct ScalingRow {
    case: &'static str,
    /// The `managedScaling` block the create names, or `None` for a create that names none.
    managed_scaling: Option<Value>,
    /// The `managedDraining` option the create names, or `None`.
    managed_draining: Option<&'static str>,
    /// The `(targetCapacity, minimumScalingStepSize, maximumScalingStepSize,
    /// instanceWarmupPeriod)` the stored provider must report.
    expect_scaling: (i64, i64, i64, i64),
    /// The `status` the stored block must report, which is the one the request named and
    /// nothing else.
    expect_status: Option<&'static str>,
    /// The `managedDraining` the stored provider must report.
    expect_draining: &'static str,
}

/// Every member of `ManagedScaling` the request leaves out takes the default its own
/// documentation states — "The default value of 100 percent results in the Amazon EC2
/// instances in your Auto Scaling group being completely used" (`targetCapacity`), "If this
/// parameter is omitted, the default value of 1 is used" (`minimumScalingStepSize`), "the
/// default value of 10000" (`maximumScalingStepSize`), "the default value of 300 seconds"
/// (`instanceWarmupPeriod`) — and `managedDraining` is turned on for a provider created
/// without it. The rows span how much of the configuration the create supplies, including
/// the published bounds of each range.
#[tokio::test]
async fn an_auto_scaling_group_provider_reports_the_scaling_configuration_amazon_ecs_applies()
{
    let rows: [ScalingRow; 4] = [
        ScalingRow {
            case: "a create that names no scaling configuration at all",
            managed_scaling: None,
            managed_draining: None,
            expect_scaling: (100, 1, 10000, 300),
            expect_status: None,
            expect_draining: "ENABLED",
        },
        ScalingRow {
            case: "a create that turns managed scaling on and names one member",
            managed_scaling: Some(json!({ "status": "ENABLED", "targetCapacity": 90 })),
            managed_draining: None,
            expect_scaling: (90, 1, 10000, 300),
            expect_status: Some("ENABLED"),
            expect_draining: "ENABLED",
        },
        ScalingRow {
            case: "a create at the low bound of every range",
            managed_scaling: Some(json!({
                "status": "DISABLED",
                "targetCapacity": 1,
                "minimumScalingStepSize": 1,
                "maximumScalingStepSize": 1,
                "instanceWarmupPeriod": 0,
            })),
            managed_draining: Some("DISABLED"),
            expect_scaling: (1, 1, 1, 0),
            expect_status: Some("DISABLED"),
            expect_draining: "DISABLED",
        },
        ScalingRow {
            case: "a create at the high bound of every range",
            managed_scaling: Some(json!({
                "targetCapacity": 100,
                "minimumScalingStepSize": 10000,
                "maximumScalingStepSize": 10000,
                "instanceWarmupPeriod": 10000,
            })),
            managed_draining: Some("ENABLED"),
            expect_scaling: (100, 10000, 10000, 10000),
            expect_status: None,
            expect_draining: "ENABLED",
        },
    ];

    let router = service();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One Auto Scaling group per provider: managed scaling drives the group's capacity,
        // so a group already bound to a provider is not available to another.
        let group = format!("asg-scaling{index}");
        create_auto_scaling_group(&router, &group, case).await;
        let name = format!("provider{index}");
        let mut binding = json!({ "autoScalingGroupArn": group });
        if let Some(scaling) = row.managed_scaling.clone() {
            binding["managedScaling"] = scaling;
        }
        if let Some(draining) = row.managed_draining {
            binding["managedDraining"] = json!(draining);
        }
        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({ "name": name, "autoScalingGroupProvider": binding }),
            case,
        )
        .await;
        let described = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": [name] }),
            case,
        )
        .await;
        let described = list(&described, "capacityProviders", case)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the provider to be described"));

        // The create's answer and the read must agree member for member: the response is
        // the stored record, not the request.
        for reported in [&created["capacityProvider"], &described] {
            let binding = &reported["autoScalingGroupProvider"];
            let scaling = &binding["managedScaling"];
            assert_eq!(
                (
                    count(scaling, "targetCapacity", case),
                    count(scaling, "minimumScalingStepSize", case),
                    count(scaling, "maximumScalingStepSize", case),
                    count(scaling, "instanceWarmupPeriod", case),
                ),
                row.expect_scaling,
                "case {case:?}: unexpected managedScaling in {reported}"
            );
            assert_eq!(
                scaling["scalingApproach"].as_str(),
                Some("SCALING_POLICY"),
                "case {case:?}: an Auto Scaling group provider is scaled by a scaling \
                 policy, got {reported}"
            );
            assert_eq!(
                scaling["status"].as_str(),
                row.expect_status,
                "case {case:?}: managedScaling status is the one the request named, got \
                 {reported}"
            );
            assert_eq!(
                string(binding, "managedDraining", case),
                row.expect_draining,
                "case {case:?}: unexpected managedDraining in {reported}"
            );
            assert_eq!(
                string(reported, "type", case),
                "EC2_AUTOSCALING",
                "case {case:?}: a provider over a caller's group is an EC2 Auto Scaling \
                 provider, got {reported}"
            );
        }
    }
}

/// One row of the capacity-provider-switch table.
struct SwitchRow {
    case: &'static str,
    /// The `capacityProviderStrategy` the update names, as JSON, or `None`.
    strategy: Option<Value>,
    /// Whether the update forces a new deployment.
    force: bool,
    /// Whether the update is served.
    served: bool,
}

/// `UpdateService$capacityProviderStrategy` lists the transitions a service may be taken
/// through between a launch type and a capacity provider strategy, and the tasks it is
/// already running were placed under the configuration it is being taken off: only a new
/// deployment replaces them, so the switch is made with `forceNewDeployment` and refused
/// without it. The rows span what the update names — a strategy with and without the force,
/// the empty list that takes a service back to its launch type, and an update that names no
/// strategy at all.
#[tokio::test]
async fn a_switch_onto_a_capacity_provider_strategy_needs_a_forced_deployment() {
    let router = service();
    let cluster = "switching";
    let case = "setup";
    create_cluster(&router, cluster, case).await;
    register_definition(&router, "switchfamily", case).await;
    create_auto_scaling_group(&router, "asg-switch", case).await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "switchprovider",
            "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-switch" },
        }),
        case,
    )
    .await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": ["switchprovider"],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;

    let strategy = json!([{ "capacityProvider": "switchprovider", "weight": 1 }]);
    let rows: [SwitchRow; 4] = [
        SwitchRow {
            case: "a strategy without the forced deployment",
            strategy: Some(strategy.clone()),
            force: false,
            served: false,
        },
        SwitchRow {
            case: "an update that names no strategy",
            strategy: None,
            force: false,
            served: true,
        },
        SwitchRow {
            case: "the empty list that takes the service back to its launch type",
            strategy: Some(json!([])),
            force: false,
            served: true,
        },
        SwitchRow {
            case: "a strategy with the forced deployment",
            strategy: Some(strategy),
            force: true,
            served: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let service_name = format!("web{index}");
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": service_name,
                "taskDefinition": "switchfamily",
                "desiredCount": 0,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
        let mut request = json!({ "cluster": cluster, "service": service_name });
        if let Some(strategy) = row.strategy.clone() {
            request["capacityProviderStrategy"] = strategy;
        }
        if row.force {
            request["forceNewDeployment"] = json!(true);
        }
        if row.served {
            let served = ok(&router, "UpdateService", request, case).await;
            assert_eq!(
                present_count(&served["service"], "version", case),
                Some(0),
                "case {case:?}: no update advances the service version, got {served}"
            );
        } else {
            let (code, message) = refusal(&router, "UpdateService", request, case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                ("InvalidParameterException", NEEDS_FORCED_DEPLOYMENT),
                "case {case:?}: expected the forced-deployment refusal, got {code} with \
                 {message:?}"
            );
            // The refusal wrote nothing: the service still reports the launch type it was
            // created with and no strategy.
            let read = describe(&router, cluster, &service_name, case).await;
            assert_eq!(
                (
                    read["launchType"].as_str(),
                    list(&read, "capacityProviderStrategy", case).len(),
                ),
                (Some("EC2"), 0),
                "case {case:?}: a refused switch leaves the service as it was, got {read}"
            );
        }
    }
}

/// `Service$currentServiceDeployment` names the rollout the service is on, and the id that
/// ARN ends in is GENERATED: a fixed-width base62 token, distinct per rollout, and the same
/// one the service reports and `DescribeServiceDeployments` answers for.
#[tokio::test]
async fn a_service_deployment_arn_ends_in_a_generated_base62_id() {
    let case = "two rollouts of one service";
    let router = service();
    let cluster = "deploymentids";
    create_cluster(&router, cluster, case).await;
    register_definition(&router, "idfamily", case).await;
    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "web",
            "taskDefinition": "idfamily",
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let first = string(&created["service"], "currentServiceDeployment", case);
    let rolled = ok(
        &router,
        "UpdateService",
        json!({ "cluster": cluster, "service": "web", "forceNewDeployment": true }),
        case,
    )
    .await;
    let second = string(&rolled["service"], "currentServiceDeployment", case);

    let prefix = format!(
        "arn:aws:ecs:{}:{}:service-deployment/{cluster}/web/",
        crate::ecs_wire::SIGNED_REGION,
        crate::ecs_wire::ACCOUNT,
    );
    for arn in [&first, &second] {
        let id = arn
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn:?} under {prefix:?}"));
        assert_eq!(
            id.len(),
            21,
            "case {case:?}: expected a 21-character deployment id, got {id:?}"
        );
        assert!(
            id.chars().all(|digit| digit.is_ascii_alphanumeric()),
            "case {case:?}: expected a base62 deployment id, got {id:?}"
        );
        assert!(
            !id.chars().all(|digit| digit.is_ascii_digit()),
            "case {case:?}: expected a generated id rather than a decimal counter, got \
             {id:?}"
        );
    }
    assert_ne!(
        first, second,
        "case {case:?}: every rollout is named by its own deployment, got {first:?}"
    );

    // The ARN the service reports is the one the deployment answers for, and the rollout it
    // replaced is still reported beside it as the service's other revision.
    let described = ok(
        &router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [second.clone()] }),
        case,
    )
    .await;
    let deployments = list(&described, "serviceDeployments", case);
    assert_eq!(
        deployments
            .first()
            .map(|deployment| string(deployment, "serviceDeploymentArn", case)),
        Some(second.clone()),
        "case {case:?}: the deployment the service names must describe, got {described}"
    );
    let revisions = list(&rolled["service"], "currentServiceRevisions", case);
    assert_eq!(
        revisions.len(),
        2,
        "case {case:?}: a rollout reports the new revision and the one it replaces, got \
         {rolled}"
    );
    // An id outside the generated FORM is no identity to look up: the whole request is
    // refused for its syntax rather than answered with a failure of that entry.
    let (code, message) = refusal(
        &router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [format!("{prefix}not-a-generated-id")] }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "The ServiceDeployment ARN is invalid."
        ),
        "case {case:?}: an id outside the generated form is a malformed ARN, got {code} \
         with {message:?}"
    );
}
