// SPDX-License-Identifier: CC0-1.0

//! The SCOPE a request's references have to be inside, and what the agent plane records —
//! driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test, each asserted over a table that spans the class rather than over
//! one request:
//!
//! * a capacity provider has to be ASSOCIATED with the cluster before a placement in that
//!   cluster may name it. `PutClusterCapacityProviders$defaultCapacityProviderStrategy`: "A
//!   capacity provider must be associated with the cluster to be used in a capacity provider
//!   strategy. The PutClusterCapacityProviders API is used to associate a capacity provider
//!   with a cluster", and of the two providers Amazon ECS publishes for every account, "The
//!   Fargate capacity providers are available to all accounts and only need to be associated
//!   with a cluster to be used". The rows span the operation that names the strategy, the
//!   KIND of provider it names (each managed Fargate provider, one the account created) and
//!   how the entry spells it (short name, full ARN);
//! * the provider a cluster's TASKS run on cannot be dropped from that association
//!   ("You can only disassociate an existing capacity provider from a cluster if it's not
//!   being used by any existing tasks"), while a managed Fargate provider — which stands no
//!   capacity up for the cluster — is dropped while its tasks run on;
//! * a reference whose ARN names one cluster while the request's `cluster` names another
//!   names no resource at all, so the call is refused rather than answered about another
//!   cluster's resource (or with a `MISSING` failure). The rows span the operations and the
//!   two resources that carry such a reference;
//! * a launch may not name a subnet the account has DELETED: `AwsVpcConfiguration$subnets`
//!   is "The IDs of the subnets associated with the task or service", and Amazon ECS
//!   retrieves each subnet from Amazon EC2 before it accepts the launch. The rows span the
//!   operation that launches and the state of the subnet (live, deleted, and one Amazon EC2
//!   holds nothing about, which this emulator does not resolve);
//! * a service-deployment or service-revision ARN carries the cluster whose service it
//!   belongs to, so a read of one under a cluster this account holds no record of is refused
//!   with `ClusterNotFoundException`, while an ARN under a cluster that IS there and names
//!   nothing stored stays a `MISSING` failure;
//! * the `Submit*StateChange` family is the container agent's ("This action is only used by
//!   the Amazon ECS agent"), and what it checks is the report: the VALUES it carries — a
//!   status outside the ECS task lifecycle, and an attachment state other than `ATTACHED`,
//!   are refused — and that the resources it names resolve inside the cluster it names. The
//!   capacity hosting the task is no input to either, so a readable report about any task the
//!   named cluster holds is recorded and acknowledged, and read back through `DescribeTasks`;
//! * `ListServicesByNamespace$namespace` is "The namespace name or full Amazon Resource Name
//!   (ARN) of the Cloud Map namespace", so either form names the same namespace.
//!
//! Nothing here reaches into a state struct: every value asserted is read back through
//! `DescribeTasks`, `DescribeServices`, `DescribeClusters` or `ListServicesByNamespace`, and
//! every Amazon EC2 resource is created through EC2's own API.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, awsvpc_network_configuration, cloud_map, cluster_with_capacity,
    create_auto_scaling_group, ec2, ec2_element, emulator_with_cloud_map, emulator_with_ec2, list,
    ok, refusal, register_capacity, register_definition, service, string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// Long enough for the attachments update an association starts to complete: "Cluster
/// attachments must be in UPDATE_COMPLETE or UPDATE_FAILED state before they can be
/// updated", a transition the clock owes one
/// `state::TRANSITION_SPAN` after the call, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
        .as_millis() as u64
        + 400,
);

/// The image every container below starts from; irrelevant to these rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The two capacity providers Amazon ECS publishes for every account in every Region.
const FARGATE: &str = "FARGATE";
const FARGATE_SPOT: &str = "FARGATE_SPOT";

/// The sentence a strategy naming a provider the cluster is not associated with is refused
/// with.
const NOT_ASSOCIATED: &str = "The specified capacity provider strategy cannot contain a capacity \
                              provider that is not associated with the cluster. Associate the \
                              capacity provider with the cluster or specify a valid capacity \
                              provider and try again.";

/// The sentence a `service` reference of another cluster is refused with.
const SERVICE_NOT_WITHIN_CLUSTER: &str = "Service is not within the provided Cluster.";

/// The sentence a `task` reference of another cluster is refused with.
const CLUSTER_IDENTIFIERS_MISMATCH: &str = "Invalid identifier: cluster identifiers mismatch";

/// The sentence a cluster this account holds no record of is refused with.
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";

/// The sentence a report naming an attachment state the plane does not take is refused with.
const ATTACHMENT_STATUS_SET: &str = "attachmentStatus should be one of [ATTACHED]";

/// How many managed agents one container holds, from this crate's own statement of the
/// bound — the most a report may leave a container with.
const MAX_MANAGED_AGENTS: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_MANAGED_AGENTS;

/// The ARN a capacity provider of `name` carries in this account and Region.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}")
}

/// Register one Fargate-capable revision of `family`: `awsvpc` with task-level cpu and
/// memory, so it may be placed under either launch type.
async fn register_fargate_definition(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("web", IMAGE)],
        }),
        case,
    )
    .await;
}

/// Associate `providers` with `cluster`, with no default strategy.
async fn associate(
    router: &MultiServiceRouter,
    cluster: &str,
    providers: &[&str],
    case: &str,
) -> Value {
    ok(
        router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": providers,
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await
}

/// Create one capacity provider over an Auto Scaling group the ACCOUNT holds — the group
/// first, through Auto Scaling's own API, since a provider over a group the account does not
/// hold is refused.
async fn create_provider(router: &MultiServiceRouter, name: &str, case: &str) {
    let group = format!("asg-{name}");
    create_auto_scaling_group(router, &group, case).await;
    ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": name,
            "autoScalingGroupProvider": { "autoScalingGroupArn": group },
        }),
        case,
    )
    .await;
}

// ---------------------------------------------------------------------------
// A capacity provider the cluster is not associated with
// ---------------------------------------------------------------------------

/// One row of the association table: the operation that names the strategy, and the request
/// it sends beyond the `capacityProviderStrategy` the row supplies. The suffix makes each
/// row's resource its own, so a refused create and the served one that follows it never
/// collide over a name.
struct StrategyRow {
    case: &'static str,
    operation: &'static str,
    request: fn(&str, &str, &str) -> Value,
}

/// The operations that take a `capacityProviderStrategy` the caller names, each built
/// against one cluster and one family.
const STRATEGY_ROWS: [StrategyRow; 3] = [
    StrategyRow {
        case: "RunTask",
        operation: "RunTask",
        request: |cluster, family, _suffix| {
            json!({
                "cluster": cluster,
                "taskDefinition": family,
                "count": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            })
        },
    },
    StrategyRow {
        case: "CreateService",
        operation: "CreateService",
        request: |cluster, family, suffix| {
            json!({
                "cluster": cluster,
                "serviceName": format!("scopedsvc{suffix}"),
                "taskDefinition": family,
                "desiredCount": 0,
                "networkConfiguration": awsvpc_network_configuration(),
            })
        },
    },
    StrategyRow {
        case: "UpdateService",
        operation: "UpdateService",
        request: |cluster, _family, _suffix| {
            json!({
                "cluster": cluster,
                "service": "updatedsvc",
                "desiredCount": 0,
            })
        },
    },
];

#[tokio::test]
async fn a_capacity_provider_strategy_may_only_name_providers_the_cluster_holds() {
    let router = service();
    let case = "setup";
    let family = "scopedfamily";
    // Two clusters: one associated with every provider the rows name, one with none at all.
    let associated = "providerheld";
    let bare = "providerfree";
    for cluster in [associated, bare] {
        cluster_with_capacity(&router, cluster, case).await;
    }
    register_fargate_definition(&router, family, case).await;
    create_provider(&router, "accountheld", case).await;
    associate(
        &router,
        associated,
        &[FARGATE, FARGATE_SPOT, "accountheld"],
        case,
    )
    .await;
    // The service `UpdateService` rows act on, in each cluster, created with no strategy of
    // its own so the update is what names one.
    for cluster in [associated, bare] {
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": "updatedsvc",
                "taskDefinition": family,
                "desiredCount": 0,
                "launchType": "EC2",
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            case,
        )
        .await;
    }

    // The rows differ along the provider the strategy names — each of the two providers
    // Amazon ECS publishes and one this account created — and along how the entry spells it:
    // "The short name or full Amazon Resource Name (ARN) of the capacity provider", so
    // either spelling names the same provider on both sides of the comparison.
    let providers = [
        ("the managed Fargate provider", FARGATE.to_string()),
        ("the managed Fargate Spot provider", FARGATE_SPOT.to_string()),
        ("a provider this account created", "accountheld".to_string()),
        ("the managed provider by ARN", provider_arn(FARGATE)),
        ("an account provider by ARN", provider_arn("accountheld")),
    ];

    for (index, row) in STRATEGY_ROWS.iter().enumerate() {
        for (provider_index, (provider_case, provider)) in providers.iter().enumerate() {
            let case = &format!("{}: {provider_case}", row.case);
            let suffix = format!("{index}{provider_index}");
            let strategy = json!([{ "capacityProvider": provider, "weight": 1 }]);

            // The cluster that is associated with it serves the call.
            let mut request = (row.request)(associated, family, &suffix);
            request["capacityProviderStrategy"] = strategy.clone();
            let (status, body) = answered(&router, row.operation, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the associated cluster to serve the call, got \
                 {status} with {body}"
            );

            // The cluster that is associated with nothing refuses it, with the sentence the
            // condition carries and nothing of the request in it.
            let mut request = (row.request)(bare, family, &suffix);
            request["capacityProviderStrategy"] = strategy;
            let (code, message) = refusal(&router, row.operation, request, case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                ("InvalidParameterException", NOT_ASSOCIATED),
                "case {case:?}: expected the association refusal, got {code} with {message:?}"
            );
        }
    }

    // And a refused create stored nothing: the bare cluster holds only the service its own
    // setup created, so neither the refused `CreateService` nor the refused `RunTask` left a
    // record behind.
    let case = "what the refusals stored";
    let listed = ok(&router, "ListServices", json!({ "cluster": bare }), case).await;
    assert_eq!(
        list(&listed, "serviceArns", case).len(),
        1,
        "case {case:?}: a refused create leaves no service, got {listed}"
    );
    let tasks = ok(&router, "ListTasks", json!({ "cluster": bare }), case).await;
    assert!(
        list(&tasks, "taskArns", case).is_empty(),
        "case {case:?}: a refused run leaves no task, got {tasks}"
    );
}

#[tokio::test]
async fn the_provider_a_cluster_runs_tasks_on_cannot_be_dropped_from_the_association() {
    let router = service();
    let case = "setup";
    let cluster = "dropscope";
    let family = "dropfamily";
    cluster_with_capacity(&router, cluster, case).await;
    register_fargate_definition(&router, family, case).await;
    create_provider(&router, "standsupcapacity", case).await;
    associate(
        &router,
        cluster,
        &[FARGATE, FARGATE_SPOT, "standsupcapacity"],
        case,
    )
    .await;
    // "Cluster attachments must be in UPDATE_COMPLETE or UPDATE_FAILED state before they can
    // be updated": the association the setup wrote completes one span later, so the drops
    // below are weighed on their own merits and not refused for a busy cluster.
    tokio::time::sleep(ONE_SPAN).await;

    // One task on the provider the ACCOUNT created — capacity that provider stands up for
    // the cluster — and one on a managed Fargate provider, which stands nothing up.
    for provider in ["standsupcapacity", FARGATE_SPOT] {
        ok(
            &router,
            "RunTask",
            json!({
                "cluster": cluster,
                "taskDefinition": family,
                "count": 1,
                "capacityProviderStrategy": [{ "capacityProvider": provider, "weight": 1 }],
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            case,
        )
        .await;
    }

    // Dropping the provider whose capacity a task runs on is refused, and the association
    // is left as it was.
    let case = "dropping the provider a task runs on";
    let (code, message) = refusal(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": [FARGATE, FARGATE_SPOT],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "ResourceInUseException",
        "case {case:?}: expected the in-use refusal, got {code} with {message:?}"
    );
    let described = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": [cluster], "include": ["ATTACHMENTS"] }),
        case,
    )
    .await;
    let held: Vec<String> = list(&described, "clusters", case)
        .first()
        .map(|cluster| list(cluster, "capacityProviders", case))
        .unwrap_or_default()
        .iter()
        .map(|provider| provider.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        held,
        vec![
            FARGATE.to_string(),
            FARGATE_SPOT.to_string(),
            "standsupcapacity".to_string()
        ],
        "case {case:?}: a refused put leaves the association where it was, got {described}"
    );

    // A managed Fargate provider stands no capacity up for the cluster, so dropping it is
    // served while its task runs on — and the task is still there afterwards.
    let case = "dropping a managed Fargate provider a task runs on";
    associate(&router, cluster, &[FARGATE, "standsupcapacity"], case).await;
    let tasks = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
    assert_eq!(
        list(&tasks, "taskArns", case).len(),
        2,
        "case {case:?}: the running tasks outlive the disassociation, got {tasks}"
    );
}

// ---------------------------------------------------------------------------
// A reference whose ARN names another cluster
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_reference_of_another_cluster_is_refused_rather_than_resolved() {
    let router = service();
    let case = "setup";
    let here = "scopehere";
    let elsewhere = "scopethere";
    let family = "crossfamily";
    for cluster in [here, elsewhere] {
        cluster_with_capacity(&router, cluster, case).await;
    }
    register_definition(&router, family, case).await;
    // The SAME service name in both clusters ("Service names must be unique within a
    // cluster, but you can have similarly named services in multiple clusters"), so a bare
    // name resolves in each and only the ARN says which cluster is meant.
    for cluster in [here, elsewhere] {
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": "shared",
                "taskDefinition": family,
                "desiredCount": 1,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
    }
    let task_here = string(
        &list(
            &ok(
                &router,
                "RunTask",
                json!({ "cluster": here, "taskDefinition": family, "count": 1 }),
                case,
            )
            .await,
            "tasks",
            case,
        )[0],
        "taskArn",
        case,
    );
    let task_elsewhere = string(
        &list(
            &ok(
                &router,
                "RunTask",
                json!({ "cluster": elsewhere, "taskDefinition": family, "count": 1 }),
                case,
            )
            .await,
            "tasks",
            case,
        )[0],
        "taskArn",
        case,
    );
    let service_here = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{here}/shared");
    let service_elsewhere =
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{elsewhere}/shared");

    // Each row: the operation, the request it sends for a reference, the sentence a
    // reference of ANOTHER cluster earns, and the reference that IS inside the cluster.
    let rows: [(&str, &str, Value, Value, &str); 4] = [
        (
            "DescribeServices, one foreign ARN",
            "DescribeServices",
            json!({ "cluster": here, "services": [service_elsewhere.clone()] }),
            json!({ "cluster": here, "services": [service_here.clone()] }),
            SERVICE_NOT_WITHIN_CLUSTER,
        ),
        (
            "DescribeServices, a foreign ARN beside one of this cluster",
            "DescribeServices",
            json!({
                "cluster": here,
                "services": [service_here.clone(), service_elsewhere.clone()],
            }),
            json!({ "cluster": here, "services": [service_here.clone(), "shared"] }),
            SERVICE_NOT_WITHIN_CLUSTER,
        ),
        (
            "GetTaskProtection",
            "GetTaskProtection",
            json!({ "cluster": here, "tasks": [task_elsewhere.clone()] }),
            json!({ "cluster": here, "tasks": [task_here.clone()] }),
            CLUSTER_IDENTIFIERS_MISMATCH,
        ),
        (
            "UpdateTaskProtection",
            "UpdateTaskProtection",
            json!({
                "cluster": here,
                "tasks": [task_elsewhere.clone()],
                "protectionEnabled": true,
            }),
            json!({
                "cluster": here,
                "tasks": [task_here.clone()],
                "protectionEnabled": true,
            }),
            CLUSTER_IDENTIFIERS_MISMATCH,
        ),
    ];

    for (case, operation, foreign, inside, expected) in rows {
        let (code, message) = refusal(&router, operation, foreign, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", expected),
            "case {case:?}: expected the cross-cluster refusal, got {code} with {message:?}"
        );
        // The same request whose reference IS inside the cluster is answered, so the rule is
        // about the two scopes disagreeing and not about the ARN form.
        let (status, body) = answered(&router, operation, inside).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a reference of this cluster to be answered, got \
             {status} with {body}"
        );
    }

    // And the foreign service is still reported by the cluster that DOES hold it: the
    // refusals changed nothing.
    let case = "the cluster that holds it";
    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": elsewhere, "services": [service_elsewhere] }),
        case,
    )
    .await;
    assert_eq!(
        list(&described, "services", case).len(),
        1,
        "case {case:?}: expected the service of its own cluster, got {described}"
    );
}

// ---------------------------------------------------------------------------
// A launch naming a subnet the account deleted
// ---------------------------------------------------------------------------

/// Create one subnet of `vpc_id` through Amazon EC2's own API and answer its id.
async fn subnet(router: &MultiServiceRouter, vpc_id: &str, cidr: &str, case: &str) -> String {
    let body = ec2(
        router,
        "CreateSubnet",
        &[
            ("VpcId", vpc_id),
            ("CidrBlock", &cidr.replace('/', "%2F")),
            ("AvailabilityZone", &format!("{SIGNED_REGION}a")),
        ],
        case,
    )
    .await;
    ec2_element(&body, "subnetId", case)
}

/// The `networkConfiguration` a launch into `subnet_id` carries.
fn placement(subnet_id: &str) -> Value {
    json!({ "awsvpcConfiguration": { "subnets": [subnet_id] } })
}

/// How a LAUNCH names its placement: the request one operation sends for a cluster, a
/// family, the subnet it launches into and the container instance it may need.
type LaunchRequest = fn(&str, &str, &str, &str) -> Value;

/// One row of the launch table: the case, the operation, and the request it sends.
struct LaunchRow {
    case: &'static str,
    operation: &'static str,
    request: LaunchRequest,
}

#[tokio::test]
async fn a_launch_naming_a_subnet_the_account_deleted_is_refused() {
    let router = emulator_with_ec2();
    let case = "setup";
    let cluster = "subnetscope";
    let family = "subnetfamily";
    let instance_arn = {
        cluster_with_capacity(&router, cluster, case).await;
        register_capacity(&router, cluster, case).await
    };
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
        .to_string();
    register_fargate_definition(&router, family, case).await;

    let vpc = ec2_element(
        &ec2(&router, "CreateVpc", &[("CidrBlock", "10.80.0.0%2F16")], case).await,
        "vpcId",
        case,
    );
    // One subnet per state the rows need: three the account deletes (one per launching
    // operation), and one it keeps.
    let live = subnet(&router, &vpc, "10.80.0.0/20", case).await;
    let mut deleted = Vec::new();
    for (index, cidr) in ["10.80.16.0/20", "10.80.32.0/20", "10.80.48.0/20"]
        .into_iter()
        .enumerate()
    {
        let subnet_id = subnet(&router, &vpc, cidr, case).await;
        ec2(
            &router,
            "DeleteSubnet",
            &[("SubnetId", &subnet_id)],
            &format!("delete {index}"),
        )
        .await;
        deleted.push(subnet_id);
    }
    // And an id Amazon EC2 holds nothing about: a subnet the account never created is as
    // unplaceable as one it deleted, which is the rule a launch is held to.
    let unknown = "subnet-0123456789abcdef0";

    // Each row: the operation that launches, and the request it sends for a placement.
    let rows: [LaunchRow; 3] = [
        LaunchRow {
            case: "RunTask",
            operation: "RunTask",
            request: |cluster, family, subnet_id, _instance| {
                json!({
                    "cluster": cluster,
                    "taskDefinition": family,
                    "count": 1,
                    "networkConfiguration": placement(subnet_id),
                })
            },
        },
        LaunchRow {
            case: "StartTask",
            operation: "StartTask",
            request: |cluster, family, subnet_id, instance| {
                json!({
                    "cluster": cluster,
                    "taskDefinition": family,
                    "containerInstances": [instance],
                    "networkConfiguration": placement(subnet_id),
                })
            },
        },
        LaunchRow {
            case: "CreateService",
            operation: "CreateService",
            request: |cluster, family, subnet_id, _instance| {
                json!({
                    "cluster": cluster,
                    "serviceName": format!("svc{}", &subnet_id[subnet_id.len() - 6..]),
                    "taskDefinition": family,
                    "desiredCount": 1,
                    "launchType": "FARGATE",
                    "networkConfiguration": placement(subnet_id),
                })
            },
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let LaunchRow {
            case,
            operation,
            request,
        } = row;
        let gone = &deleted[index];
        let (code, message) = refusal(
            &router,
            operation,
            request(cluster, family, gone, &instance_id),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                format!(
                    "Error retrieving subnet information for [{gone}]: The subnet ID \
                     '{gone}' does not exist (ErrorCode: InvalidSubnetID.NotFound)"
                )
                .as_str()
            ),
            "case {case:?}: expected the subnet-retrieval refusal, got {code} with {message:?}"
        );

        // The same launch into a subnet that still exists is served; one into an id Amazon
        // EC2 holds no subnet for earns the same lookup refusal, since the interface has
        // nowhere to be created either way.
        let case_name = case;
        let case = &format!("{case_name}, a live subnet");
        let (status, body) = answered(
            &router,
            operation,
            request(cluster, family, live.as_str(), &instance_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the launch to be served, got {status} with {body}"
        );
        let case = &format!("{case_name}, an id Amazon EC2 holds no subnet for");
        let (code, message) = refusal(
            &router,
            operation,
            request(cluster, family, unknown, &instance_id),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                format!(
                    "Error retrieving subnet information for [{unknown}]: The subnet ID \
                     '{unknown}' does not exist (ErrorCode: InvalidSubnetID.NotFound)"
                )
                .as_str()
            ),
            "case {case:?}: expected the subnet-retrieval refusal, got {code} with {message:?}"
        );
    }

    // Nothing the refusals named was launched: no task of the cluster sits in a subnet the
    // account deleted, and the service whose create was refused was never stored.
    let case = "what the refusals launched";
    let listed = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
    let arns: Vec<Value> = list(&listed, "taskArns", case);
    assert!(
        !arns.is_empty(),
        "case {case:?}: expected the served launches' tasks, got {listed}"
    );
    let described = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": arns }),
        case,
    )
    .await;
    for task in list(&described, "tasks", case) {
        for attachment in list(&task, "attachments", case) {
            for detail in list(&attachment, "details", case) {
                if detail["name"].as_str() == Some("subnetId") {
                    let subnet_id = string(&detail, "value", case);
                    assert!(
                        !deleted.contains(&subnet_id),
                        "case {case:?}: no task may sit in a deleted subnet, got {task}"
                    );
                }
            }
        }
    }
    let refused_name = format!("svc{}", &deleted[2][deleted[2].len() - 6..]);
    let services = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [refused_name.clone()] }),
        case,
    )
    .await;
    assert!(
        list(&services, "services", case).is_empty(),
        "case {case:?}: a refused create leaves no service, got {services}"
    );
}

// ---------------------------------------------------------------------------
// A deployment read under a cluster that does not exist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deployment_read_resolves_the_cluster_its_arn_carries() {
    let router = service();
    let case = "setup";
    let cluster = "deployscope";
    cluster_with_capacity(&router, cluster, case).await;
    register_definition(&router, "deployfamily", case).await;
    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "rolled",
            "taskDefinition": "deployfamily",
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let deployment = string(&created["service"], "currentServiceDeployment", case);
    // The service revision the rollout targets, read back through the deployment's own
    // reader — the one place a revision ARN is reported from.
    let described = ok(
        &router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [deployment.clone()] }),
        case,
    )
    .await;
    let revision = string(
        &list(&described, "serviceDeployments", case)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected the rollout, got {described}"))
            ["targetServiceRevision"],
        "arn",
        case,
    );
    let deployment_id = deployment
        .rsplit('/')
        .next()
        .expect("a service-deployment ARN ends in its id")
        .to_string();
    let revision_id = revision
        .rsplit('/')
        .next()
        .expect("a service-revision ARN ends in its id")
        .to_string();

    // Each row: the read, the member it takes its ARNs in, and how an ARN of this cluster
    // and one of a cluster nothing created are built.
    let rows: [(&str, &str, String, String); 2] = [
        (
            "DescribeServiceDeployments",
            "serviceDeploymentArns",
            deployment.clone(),
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service-deployment/nosuchcluster/rolled/{deployment_id}"
            ),
        ),
        (
            "DescribeServiceRevisions",
            "serviceRevisionArns",
            revision.clone(),
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service-revision/nosuchcluster/rolled/{revision_id}"
            ),
        ),
    ];

    for (operation, member, held, foreign) in rows {
        // An ARN under a cluster this account holds no record of leaves the read nothing to
        // report on at all.
        let case = &format!("{operation} under a cluster that was never created");
        let (code, message) = refusal(
            &router,
            operation,
            json!({ member: [foreign.clone()] }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClusterNotFoundException", CLUSTER_NOT_FOUND),
            "case {case:?}: expected the cluster-not-found refusal, got {code} with {message:?}"
        );

        // The same read is refused for the cluster even when an ARN it CAN resolve is named
        // beside it: the cluster is resolved before anything is reported.
        let case = &format!("{operation} with a foreign ARN beside a stored one");
        let (code, _) = refusal(
            &router,
            operation,
            json!({ member: [held.clone(), foreign.clone()] }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {case:?}: expected the cluster-not-found refusal, got {code}"
        );

        // An ARN under a cluster that IS there and names nothing stored stays a failure of
        // its own entry, and the stored ARN is still reported beside it.
        let case = &format!("{operation} naming nothing stored under a live cluster");
        let absent = held.replace(
            held.rsplit('/').next().unwrap_or_default(),
            &"9".repeat(held.rsplit('/').next().unwrap_or_default().len()),
        );
        let body = ok(
            &router,
            operation,
            json!({ member: [held.clone(), absent.clone()] }),
            case,
        )
        .await;
        let failures: Vec<(String, String)> = list(&body, "failures", case)
            .iter()
            .map(|failure| {
                (
                    string(failure, "arn", case),
                    string(failure, "reason", case),
                )
            })
            .collect();
        assert_eq!(
            failures,
            vec![(absent, "MISSING".to_string())],
            "case {case:?}: expected the MISSING failure of that entry, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The agent plane
// ---------------------------------------------------------------------------

/// Run one task of `family` in `cluster` with `placement` overlaid on the request and
/// answer its ARN.
async fn run(
    router: &MultiServiceRouter,
    cluster: &str,
    family: &str,
    overlay: Value,
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": cluster,
        "taskDefinition": family,
        "count": 1,
        "networkConfiguration": awsvpc_network_configuration(),
    });
    let Value::Object(members) = overlay else {
        panic!("a request overlay is an object");
    };
    for (member, value) in members {
        request[member] = value;
    }
    let body = ok(router, "RunTask", request, case).await;
    string(&list(&body, "tasks", case)[0], "taskArn", case)
}

/// The id of the one attachment an `awsvpc` task holds, read back through `DescribeTasks`.
async fn attachment_id(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let task = list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {body}"));
    let attachments = list(&task, "attachments", case);
    string(
        attachments
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected the interface, got {task}")),
        "id",
        case,
    )
}

/// The status one attachment of a task holds, read back through `DescribeTasks`.
async fn attachment_status(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    attachment: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let task = list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {body}"));
    let found = list(&task, "attachments", case)
        .into_iter()
        .find(|entry| entry["id"].as_str() == Some(attachment))
        .unwrap_or_else(|| panic!("case {case:?}: expected the interface back, got {task}"));
    string(&found, "status", case)
}

/// The status one container of a task holds, read back through `DescribeTasks`.
async fn container_status(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    container: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let task = list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {body}"));
    let found = list(&task, "containers", case)
        .into_iter()
        .find(|entry| entry["name"].as_str() == Some(container))
        .unwrap_or_else(|| panic!("case {case:?}: expected the container back, got {task}"));
    string(&found, "lastStatus", case)
}

/// RULE: a `Submit*StateChange` report is about the resources it NAMES — the cluster, the
/// task, the container, the attachment — and the capacity that hosts the task is not one of
/// its inputs, so a report whose values the plane reads and whose references resolve inside
/// the named cluster is recorded and acknowledged whatever the task runs on.
///
/// The rows differ along WHERE the task runs, which is the only thing this rule generalises
/// over: the Fargate launch type, each managed Fargate capacity provider, and the capacity
/// the account registered to the cluster — reached by the launch type and by naming nothing
/// at all.
#[tokio::test]
async fn the_agent_plane_records_a_report_about_any_task_the_named_cluster_holds() {
    let router = service();
    let case = "setup";
    let cluster = "agentscope";
    let family = "agentfamily";
    cluster_with_capacity(&router, cluster, case).await;
    register_fargate_definition(&router, family, case).await;
    associate(&router, cluster, &[FARGATE, FARGATE_SPOT], case).await;

    let rows: [(&str, Value); 5] = [
        ("the FARGATE launch type", json!({ "launchType": "FARGATE" })),
        (
            "the FARGATE capacity provider",
            json!({ "capacityProviderStrategy": [{ "capacityProvider": FARGATE, "weight": 1 }] }),
        ),
        (
            "the FARGATE_SPOT capacity provider",
            json!({
                "capacityProviderStrategy": [{ "capacityProvider": FARGATE_SPOT, "weight": 1 }],
            }),
        ),
        ("the EC2 launch type", json!({ "launchType": "EC2" })),
        ("no placement named at all", json!({})),
    ];

    for (case, overlay) in rows {
        let task_arn = run(&router, cluster, family, overlay, case).await;
        let attachment = attachment_id(&router, cluster, &task_arn, case).await;

        // Amazon ECS creates the interface and carries it as far as `ATTACHING` itself; the
        // attach the agent completed is what the report below carries, so the attachment is
        // not in place before it.
        assert_ne!(
            attachment_status(&router, cluster, &task_arn, &attachment, case).await,
            "ATTACHED",
            "case {case:?}: expected an interface Amazon ECS has not attached yet"
        );

        // Each operation of the family, with the reference each of them names the task by.
        let reports: [(&str, Value); 3] = [
            (
                "SubmitTaskStateChange",
                json!({ "cluster": cluster, "task": task_arn.clone(), "status": "RUNNING" }),
            ),
            (
                "SubmitContainerStateChange",
                json!({
                    "cluster": cluster,
                    "task": task_arn.clone(),
                    "containerName": "web",
                    "status": "RUNNING",
                }),
            ),
            (
                "SubmitAttachmentStateChanges",
                json!({
                    "cluster": cluster,
                    "attachments": [{ "attachmentArn": attachment.clone(), "status": "ATTACHED" }],
                }),
            ),
        ];

        for (operation, request) in reports {
            let case = &format!("{operation}: {case}");
            let acknowledged = ok(&router, operation, request, case).await;
            assert_eq!(
                acknowledged["acknowledgment"].as_str(),
                Some("ACK"),
                "case {case:?}: expected the report to be taken, got {acknowledged}"
            );
        }

        // What the reports left, read back through `DescribeTasks`: every one of them is in
        // the record the next caller sees.
        assert_eq!(
            container_status(&router, cluster, &task_arn, "web", case).await,
            "RUNNING",
            "case {case:?}: the container report the plane took is what the record holds"
        );
        assert_eq!(
            attachment_status(&router, cluster, &task_arn, &attachment, case).await,
            "ATTACHED",
            "case {case:?}: the attachment report the plane took is what the record holds"
        );
        let read = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": cluster, "tasks": [task_arn.clone()] }),
            case,
        )
        .await;
        let task = list(&read, "tasks", case)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {read}"));
        assert_eq!(
            task["lastStatus"].as_str(),
            Some("RUNNING"),
            "case {case:?}: the report the plane took is what the task holds, got {task}"
        );
    }
}

#[tokio::test]
async fn a_report_is_refused_for_the_values_it_carries_whatever_hosts_the_task() {
    let router = service();
    let case = "setup";
    let cluster = "reportvalues";
    let family = "reportfamily";
    cluster_with_capacity(&router, cluster, case).await;
    register_fargate_definition(&router, family, case).await;
    // One task on capacity the account registered and one on Fargate: the capacity hosting
    // the task is no input to the value checks, so each row is refused the same way on both.
    let ec2_task = run(&router, cluster, family, json!({ "launchType": "EC2" }), case).await;
    let fargate_task = run(
        &router,
        cluster,
        family,
        json!({ "launchType": "FARGATE" }),
        case,
    )
    .await;
    let fargate_attachment = attachment_id(&router, cluster, &fargate_task, case).await;

    // `SubmitTaskStateChange$status` has no modelled value set, and the stages the plane
    // takes are the ECS task lifecycle's. Each row is a value and whether the plane reads
    // it: the lifecycle's own stages, a value no lifecycle names, one that differs only in
    // case, and one that is a stage of a CONTAINER rather than of a task.
    let statuses: [(&str, bool); 12] = [
        ("PROVISIONING", true),
        ("PENDING", true),
        ("ACTIVATING", true),
        ("RUNNING", true),
        ("DEACTIVATING", true),
        ("STOPPING", true),
        ("DEPROVISIONING", true),
        ("STOPPED", true),
        ("NOT_A_REAL_ECS_TASK_STATUS", false),
        ("running", false),
        ("", false),
        ("PULLED", false),
    ];

    for (status, readable) in statuses {
        let case = &format!("status {status:?}");
        let request = json!({
            "cluster": cluster,
            "task": ec2_task.clone(),
            "status": status,
        });
        if readable {
            let acknowledged = ok(&router, "SubmitTaskStateChange", request, case).await;
            assert_eq!(
                acknowledged["acknowledgment"].as_str(),
                Some("ACK"),
                "case {case:?}: expected a lifecycle stage to be read, got {acknowledged}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "SubmitTaskStateChange", request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                format!("The status {status} is not a valid status.").as_str()
            ),
            "case {case:?}: expected the invalid-status refusal, got {code} with {message:?}"
        );

        // The SAME value on a task Fargate hosts is refused the same way: what the report
        // says is read whatever capacity the task runs on.
        let case = &format!("status {status:?} on a Fargate task");
        let (code, message) = refusal(
            &router,
            "SubmitTaskStateChange",
            json!({
                "cluster": cluster,
                "task": fargate_task.clone(),
                "status": status,
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                format!("The status {status} is not a valid status.").as_str()
            ),
            "case {case:?}: expected the value to be read whatever hosts the task, got {code} \
             with {message:?}"
        );
    }

    // The same ordering for the attachment state a report carries, in both the operations
    // that carry one.
    for (operation, request) in [
        (
            "SubmitTaskStateChange",
            json!({
                "cluster": cluster,
                "task": fargate_task.clone(),
                "attachments": [{ "attachmentArn": fargate_attachment, "status": "ATTACHING" }],
            }),
        ),
        (
            "SubmitAttachmentStateChanges",
            json!({
                "cluster": cluster,
                "attachments": [{ "attachmentArn": fargate_attachment, "status": "ATTACHING" }],
            }),
        ),
    ] {
        let case = &format!("{operation} with an attachment state the plane does not take");
        let (code, message) = refusal(&router, operation, request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", ATTACHMENT_STATUS_SET),
            "case {case:?}: expected the attachment-state refusal, got {code} with {message:?}"
        );
    }

    // A managed-agent entry names an agent the model lists
    // (`ManagedAgentStateChange$managedAgentName` targets `ManagedAgentName`), so no report
    // can mint a record under a name of its own: a name outside that set is refused, and the
    // one agent it does name is held ONCE however many entries report it. The report's own
    // list is bounded at `state::MAX_MANAGED_AGENTS`, so the bound itself is taken and one
    // entry past it is refused and records nothing.
    const MODELLED_AGENT: &str = "ExecuteCommandAgent";
    let agents = |count: usize, name: &str| -> Value {
        (0..count)
            .map(|_| {
                json!({
                    "containerName": "web",
                    "managedAgentName": name,
                    "status": "RUNNING",
                })
            })
            .collect::<Vec<Value>>()
            .into()
    };

    let case = "an agent the model does not name";
    let (code, message) = refusal(
        &router,
        "SubmitTaskStateChange",
        json!({
            "cluster": cluster,
            "task": ec2_task.clone(),
            "managedAgents": agents(1, "AnAgentNobodyModels"),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            format!("managed agent name should be one of [{MODELLED_AGENT}]").as_str()
        ),
        "case {case:?}: expected the modelled-set refusal, got {code} with {message:?}"
    );
    assert!(
        managed_agent_names(&router, cluster, &ec2_task, case)
            .await
            .is_empty(),
        "case {case:?}: a refused report mints no record"
    );

    let case = "the managed-agent list bound itself";
    let acknowledged = ok(
        &router,
        "SubmitTaskStateChange",
        json!({
            "cluster": cluster,
            "task": ec2_task.clone(),
            "managedAgents": agents(MAX_MANAGED_AGENTS, MODELLED_AGENT),
        }),
        case,
    )
    .await;
    assert_eq!(
        acknowledged["acknowledgment"].as_str(),
        Some("ACK"),
        "case {case:?}: the bound itself must be taken, got {acknowledged}"
    );
    assert_eq!(
        managed_agent_names(&router, cluster, &ec2_task, case).await,
        vec![MODELLED_AGENT.to_string()],
        "case {case:?}: the one agent the entries named is held once"
    );

    let case = "one managed-agent entry past the bound";
    let (code, message) = refusal(
        &router,
        "SubmitTaskStateChange",
        json!({
            "cluster": cluster,
            "task": ec2_task.clone(),
            "managedAgents": agents(MAX_MANAGED_AGENTS + 1, MODELLED_AGENT),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            format!("managed agents can have at most {MAX_MANAGED_AGENTS} items.").as_str()
        ),
        "case {case:?}: expected the list-bound refusal, got {code} with {message:?}"
    );
    assert_eq!(
        managed_agent_names(&router, cluster, &ec2_task, case).await,
        vec![MODELLED_AGENT.to_string()],
        "case {case:?}: a refused report leaves the container's agents where they were"
    );
}

/// The names of the managed agents the one container of `task_arn` holds, read back through
/// `DescribeTasks`.
async fn managed_agent_names(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let task = list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {body}"));
    let container = list(&task, "containers", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the container, got {task}"));
    let mut names: Vec<String> = list(&container, "managedAgents", case)
        .iter()
        .map(|agent| agent["name"].as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// A namespace named by ARN
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_namespace_is_named_by_its_name_or_by_its_full_arn() {
    let router = emulator_with_cloud_map();
    let case = "setup";
    let cluster = "namespacescope";
    cluster_with_capacity(&router, cluster, case).await;
    register_definition(&router, "nsfamily", case).await;

    // The namespace is Cloud Map's resource, so it is created through Cloud Map's own API
    // and its id is the one that service minted.
    cloud_map(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "arn-namespace" }),
        case,
    )
    .await;
    let listed = cloud_map(&router, "ListNamespaces", json!({}), case).await;
    let namespace = list(&listed, "Namespaces", case)
        .into_iter()
        .find(|namespace| namespace["Name"].as_str() == Some("arn-namespace"))
        .unwrap_or_else(|| panic!("case {case:?}: expected the namespace back, got {listed}"));
    let namespace_id = string(&namespace, "Id", case);
    let namespace_arn = string(&namespace, "Arn", case);

    // Two services: one that joined the namespace by NAME and one that joined it by the
    // full ARN. "The namespace name or full Amazon Resource Name (ARN) of the Cloud Map
    // namespace" is what either member takes, and both of those name the SAME namespace — so
    // both services are in it, however each of them spelled it.
    let mut joined = Vec::new();
    for (name, reference) in [
        ("joinedbyname", "arn-namespace".to_string()),
        ("joinedbyarn", namespace_arn.clone()),
    ] {
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": name,
                "taskDefinition": "nsfamily",
                "desiredCount": 0,
                "launchType": "EC2",
                "serviceConnectConfiguration": { "enabled": true, "namespace": reference },
            }),
            case,
        )
        .await;
        joined.push(string(&created["service"], "serviceArn", case));
    }

    // "Gets a list of services ... that are associated with a Cloud Map namespace", in the
    // order a page walks them (by service ARN), so the expectation is the same set however
    // the request names the namespace.
    let mut both = joined.clone();
    both.sort();

    // The rows span the forms a `namespace` reference takes — the NAME, the full ARN and the
    // id that ARN ends with — each of which names the one namespace both services are in, so
    // each selects both of them.
    let found: [(&str, String); 3] = [
        ("the namespace name", "arn-namespace".to_string()),
        ("the full namespace ARN", namespace_arn.clone()),
        ("the namespace id the ARN ends with", namespace_id.clone()),
    ];
    for (case, namespace) in found {
        let body = ok(
            &router,
            "ListServicesByNamespace",
            json!({ "namespace": namespace }),
            case,
        )
        .await;
        let arns: Vec<String> = list(&body, "serviceArns", case)
            .iter()
            .map(|arn| arn.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            arns, both,
            "case {case:?}: expected the namespace's services, got {body}"
        );
    }

    // A value outside both forms names no namespace and is still refused for its FORM: an
    // ARN is read as the ARN it is, and a NAME is held to the pattern a name must match.
    let malformed: [(&str, String); 4] = [
        ("a name with a slash in it", "name/space".to_string()),
        ("a name with a space in it", "not a namespace".to_string()),
        ("no namespace at all", String::new()),
        (
            "an ARN of another service's resource type",
            format!("arn:aws:servicediscovery:{SIGNED_REGION}:{ACCOUNT}:service/srv-abc"),
        ),
    ];
    for (case, namespace) in malformed {
        let (code, message) = refusal(
            &router,
            "ListServicesByNamespace",
            json!({ "namespace": namespace.clone() }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                format!("Namespace ({namespace}) must match ^[!-~&&[^<>/\"]]{{1,1024}}$.")
                    .as_str()
            ),
            "case {case:?}: expected the form refusal, got {code} with {message:?}"
        );
    }

    // And an ARN of a namespace nothing holds is read as the ARN it is, so it earns the
    // not-found refusal rather than the form one.
    let case = "an ARN of a namespace nothing holds";
    let reference =
        format!("arn:aws:servicediscovery:{SIGNED_REGION}:{ACCOUNT}:namespace/ns-neverminted");
    let (code, message) = refusal(
        &router,
        "ListServicesByNamespace",
        json!({ "namespace": reference.clone() }),
        case,
    )
    .await;
    // The refusal names the reference the request carried, spelled as it was sent.
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "NamespaceNotFoundException",
            format!("Failed to retrieve namespace for {reference}").as_str()
        ),
        "case {case:?}: expected the namespace-not-found refusal, got {code} with {message:?}"
    );
}
