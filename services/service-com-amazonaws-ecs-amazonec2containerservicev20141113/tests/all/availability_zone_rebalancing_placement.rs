// SPDX-License-Identifier: CC0-1.0

//! What `availabilityZoneRebalancing` a create that NAMES NONE leaves a service with.
//!
//! The rule under test: the member states its create default with no condition on it — "For
//! create service requests, when no value is specified for AvailabilityZoneRebalancing,
//! Amazon ECS defaults the value to ENABLED" — so every service a create that names no value
//! leaves behind carries `ENABLED`, wherever its tasks are placed. `DISABLED` is the value of
//! a service whose own request stated it. Whatever the create resolved is the value every
//! later read reports and the value an update that names none keeps ("For update service
//! requests, when no value is specified ... Amazon ECS defaults to the existing service's
//! AvailabilityZoneRebalancing value").
//!
//! The rows differ along WHERE the service is placed, which is exactly what the create
//! default does NOT depend on: a launch type the create names (the one Amazon ECS places
//! itself and the one it places on the cluster's own capacity), a capacity provider strategy
//! naming each of the two predefined Fargate providers, a strategy naming a provider over an
//! Auto Scaling group of the account, the cluster's own default strategy where the create
//! names neither, and a create that names nothing at all on a cluster that has no default
//! either. Each placement is driven twice: once by a create that names no value, and once by
//! a create that names the other value of the enum
//! (`smithy_query(query="shape", shape="AvailabilityZoneRebalancing")` = [ENABLED,
//! DISABLED]), so a value that was reported without being read off the stored record cannot
//! pass either row.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so a value not computed from the stored record cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, create_auto_scaling_group, create_cluster, ok,
    register_definition, service, string,
};

/// The two values `AvailabilityZoneRebalancing` declares
/// (`smithy_query(query="shape", shape="AvailabilityZoneRebalancing")`).
const ENABLED: &str = "ENABLED";
/// See [`ENABLED`].
const DISABLED: &str = "DISABLED";

/// What one row's `CreateService` names for its placement.
#[derive(Debug, Clone, Copy)]
enum Places {
    /// A `launchType`.
    LaunchType(&'static str),
    /// A `capacityProviderStrategy` naming this provider.
    Strategy(&'static str),
    /// Neither, so the cluster's own default strategy — if it has one — decides.
    Nothing,
}

/// One row: where the service is placed.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    places: Places,
    /// Whether the row runs on the cluster that carries a default strategy.
    on_cluster_with_default: bool,
}

/// The cluster whose capacity providers are the two predefined Fargate ones and the group
/// provider below, with a DEFAULT strategy that places on Fargate.
const CLUSTER_WITH_DEFAULT: &str = "rebalancedefaulted";

/// The cluster that carries no default strategy at all, so a create that names neither a
/// launch type nor a strategy falls back on the capacity registered to it.
const CLUSTER_WITHOUT_DEFAULT: &str = "rebalanceplain";

/// The capacity provider this suite creates over an Auto Scaling group of the account:
/// capacity the ACCOUNT owns, which is not Fargate.
const GROUP_PROVIDER: &str = "rebalancegroupprovider";

/// A revision placed on Fargate: `awsvpc`, at one of the published task sizes.
const FARGATE_FAMILY: &str = "rebalancefargatefamily";

/// A revision placed on the cluster's own capacity.
const EC2_FAMILY: &str = "rebalanceec2family";

/// Register the Fargate-compatible revision every Fargate-placed row is created from.
async fn register_fargate_definition(router: &MultiServiceRouter, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FARGATE_FAMILY,
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
        case,
    )
    .await;
}

/// Whether this row's placement is one Amazon ECS itself places the tasks of, which decides
/// both the revision the create names and whether it carries a network configuration.
fn placed_on_fargate(row: &Row) -> bool {
    match row.places {
        Places::LaunchType(launch_type) => launch_type == "FARGATE",
        Places::Strategy(provider) => provider.starts_with("FARGATE"),
        Places::Nothing => row.on_cluster_with_default,
    }
}

/// The cluster one row's services are created on.
fn cluster_of(row: &Row) -> &'static str {
    if row.on_cluster_with_default {
        CLUSTER_WITH_DEFAULT
    } else {
        CLUSTER_WITHOUT_DEFAULT
    }
}

/// The `CreateService` body for one row, naming `requested` — or no value at all for `None`.
fn create_request(row: &Row, service_name: &str, requested: Option<&str>) -> Value {
    let fargate = placed_on_fargate(row);
    let mut request = json!({
        "cluster": cluster_of(row),
        "serviceName": service_name,
        "taskDefinition": if fargate { FARGATE_FAMILY } else { EC2_FAMILY },
        // These rows are about the stored record, so no row asks for a task.
        "desiredCount": 0,
    });
    if fargate {
        request["networkConfiguration"] = awsvpc_network_configuration();
    }
    match row.places {
        Places::LaunchType(launch_type) => request["launchType"] = json!(launch_type),
        Places::Strategy(provider) => {
            request["capacityProviderStrategy"] = json!([{ "capacityProvider": provider, "weight": 1 }]);
        }
        Places::Nothing => {}
    }
    if let Some(value) = requested {
        request["availabilityZoneRebalancing"] = json!(value);
    }
    request
}

/// Create the service, then assert the create, the read after it and an update that names no
/// value all report `expect`.
async fn create_and_assert(
    router: &MultiServiceRouter,
    row: &Row,
    service_name: &str,
    requested: Option<&str>,
    expect: &str,
) {
    let case = row.case;
    let cluster = cluster_of(row);
    let created = ok(
        router,
        "CreateService",
        create_request(row, service_name, requested),
        case,
    )
    .await;
    let reported = string(&created["service"], "availabilityZoneRebalancing", case);
    assert_eq!(
        reported, expect,
        "case {case:?} (create naming {requested:?}): expected availabilityZoneRebalancing \
         {expect:?}, got {reported:?} in {created}"
    );

    // The value the create resolved is the one the STORED record carries, so the read
    // reports it too.
    let described = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    let read = string(
        &described["services"][0],
        "availabilityZoneRebalancing",
        case,
    );
    assert_eq!(
        read, expect,
        "case {case:?} (create naming {requested:?}): the read must report what the create \
         resolved, expected {expect:?}, got {read:?} in {described}"
    );

    // "For update service requests, when no value is specified ... Amazon ECS defaults
    // to the existing service's AvailabilityZoneRebalancing value": an unrelated update
    // keeps it.
    let updated = ok(
        router,
        "UpdateService",
        json!({ "cluster": cluster, "service": service_name, "desiredCount": 0 }),
        case,
    )
    .await;
    let after = string(&updated["service"], "availabilityZoneRebalancing", case);
    assert_eq!(
        after, expect,
        "case {case:?} (create naming {requested:?}): an update naming no value keeps the \
         service's value, expected {expect:?}, got {after:?} in {updated}"
    );
}

#[tokio::test]
async fn a_create_that_names_no_rebalancing_enables_it_wherever_the_service_is_placed() {
    let router = service();
    let setup = "setup";
    create_cluster(&router, CLUSTER_WITHOUT_DEFAULT, setup).await;
    create_cluster(&router, CLUSTER_WITH_DEFAULT, setup).await;
    register_definition(&router, EC2_FAMILY, setup).await;
    register_fargate_definition(&router, setup).await;
    create_auto_scaling_group(&router, "asg-rebalance", setup).await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": GROUP_PROVIDER,
            "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-rebalance" },
        }),
        setup,
    )
    .await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": CLUSTER_WITH_DEFAULT,
            "capacityProviders": ["FARGATE", "FARGATE_SPOT", GROUP_PROVIDER],
            "defaultCapacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }],
        }),
        setup,
    )
    .await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": CLUSTER_WITHOUT_DEFAULT,
            "capacityProviders": ["FARGATE", "FARGATE_SPOT", GROUP_PROVIDER],
            "defaultCapacityProviderStrategy": [],
        }),
        setup,
    )
    .await;

    let rows: [Row; 7] = [
        Row {
            case: "a launch type on the capacity registered to the cluster",
            places: Places::LaunchType("EC2"),
            on_cluster_with_default: false,
        },
        Row {
            case: "the Fargate launch type",
            places: Places::LaunchType("FARGATE"),
            on_cluster_with_default: false,
        },
        Row {
            case: "a strategy naming the predefined Fargate provider",
            places: Places::Strategy("FARGATE"),
            on_cluster_with_default: false,
        },
        Row {
            case: "a strategy naming the predefined Fargate Spot provider",
            places: Places::Strategy("FARGATE_SPOT"),
            on_cluster_with_default: false,
        },
        Row {
            case: "a strategy naming a provider over an Auto Scaling group",
            places: Places::Strategy(GROUP_PROVIDER),
            on_cluster_with_default: false,
        },
        Row {
            case: "neither, on a cluster whose default strategy places on Fargate",
            places: Places::Nothing,
            on_cluster_with_default: true,
        },
        Row {
            case: "neither, on a cluster that has no default strategy",
            places: Places::Nothing,
            on_cluster_with_default: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // A create that names NO value: the member's documented create default, whatever
        // this row's placement is.
        create_and_assert(&router, row, &format!("rebalance{index}"), None, ENABLED).await;
        // And a create that NAMES the other value of the enum keeps that one, so the row
        // above is reading the record rather than a constant.
        create_and_assert(
            &router,
            row,
            &format!("rebalanceoff{index}"),
            Some(DISABLED),
            DISABLED,
        )
        .await;
    }
}
