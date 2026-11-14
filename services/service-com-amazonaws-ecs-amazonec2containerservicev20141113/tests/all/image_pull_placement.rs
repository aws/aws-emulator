// SPDX-License-Identifier: CC0-1.0

//! Whether a launched task can FETCH the images its containers run from the network
//! placement it was launched into — driven end to end over the shared wire harness in
//! [`crate::ecs_wire`], with the Amazon EC2 service mounted beside this one on the same
//! engine so every subnet, route table, gateway and VPC endpoint is created through EC2's
//! OWN API rather than written into a store by hand.
//!
//! The rule under test: a container runs an image, and that image is fetched from its
//! registry before the container can start — so a task whose placement has no path to the
//! registry never runs. Amazon ECS stops it instead, and the stop carries the
//! `TaskStopCode` member for a task that failed before it started (`Task$stopCode` is "the
//! stop code indicating why a task was stopped") with a `Task$stoppedReason` ("The reason
//! that the task was stopped") reporting the runtime's `CannotPullContainerError` for the
//! image that could not be fetched. A task that never ran has no `startedAt` and its
//! containers no `exitCode`.
//!
//! The path is Amazon VPC's, and the rows below differ along every way it can be open or
//! closed, which is what the rule generalises over:
//!
//! * the ROUTE the placement's subnet has for traffic leaving the VPC — none at all, an
//!   internet gateway, a public NAT gateway, a private NAT gateway (which "can't" route
//!   internet-bound traffic, `CreateNatGateway`);
//! * whether the task is given a public address (`AwsVpcConfiguration$assignPublicIp`),
//!   which is what lets an internet gateway carry its traffic;
//! * the REGISTRY the image reference resolves to — a public registry, a bare name a
//!   container runtime resolves against Docker Hub, and a repository in Amazon ECR, which
//!   a VPC with the endpoints that service publishes reaches with no route off the VPC at
//!   all;
//! * which route table applies: one a caller associated with the subnet, and the VPC's
//!   MAIN table for a subnet associated with none.
//!
//! Nothing here reaches into either state struct: every value asserted is read back
//! through `DescribeTasks`, `ListTasks` and `DescribeServices`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, ec2, ec2_element, ecr, emulator_with_ec2, list, ok, string, ACCOUNT,
    AWSVPC_START, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster every task below runs in.
const CLUSTER: &str = "imagefetch";

/// How often a poll re-reads a task.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll keeps trying before it gives up: the whole start of an `awsvpc` task
/// ([`AWSVPC_START`]) plus the two further spans a launch failure takes (the fetch is
/// attempted where the start would have been, and the stop falls due one span after
/// that), with margin for a loaded test machine.
const POLL_BUDGET: Duration = AWSVPC_START.saturating_add(Duration::from_secs(3));

/// An image in a PUBLIC container registry: its registry is no AWS service, so it is
/// reachable only over the internet.
const PUBLIC_IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// An image reference that names no registry at all, which a container runtime resolves
/// against Docker Hub — the other form of a public image, and the row that proves the rule
/// is about the registry the reference RESOLVES to rather than about its text.
const HUB_IMAGE: &str = "nginx:1.27";

/// `Task$lastStatus` of a task that is running.
const RUNNING: &str = "RUNNING";

/// `Task$lastStatus` of a task that has stopped.
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of
/// (`smithy_query(query="shape",shape="TaskStopCode")`).
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// The error class a container runtime reports a failed image fetch under, which is the
/// fixed part of the `stoppedReason`.
const CANNOT_PULL: &str = "CannotPullContainerError";

/// An image in a repository of THIS account's Amazon ECR registry, whose registry URI is
/// `<account>.dkr.ecr.<region>.amazonaws.com` (Amazon ECR User Guide). The repository and
/// the image it names are pushed through Amazon ECR's own API ([`push_ecr_image`]), because
/// what these rows are about is the PATH to the registry: an image the registry does not
/// hold cannot be fetched from any placement.
fn ecr_image() -> String {
    format!("{ACCOUNT}.dkr.ecr.{SIGNED_REGION}.amazonaws.com/{ECR_REPOSITORY}:{ECR_TAG}")
}

/// The repository of this account's registry the rows below pull from.
const ECR_REPOSITORY: &str = "app";

/// The tag that repository's one image carries.
const ECR_TAG: &str = "1";

/// A Docker image manifest V2 schema 2 referencing no blob: what these rows are about is
/// the path to the registry, not which blobs an image is built from.
const ECR_MANIFEST: &str = concat!(
    r#"{"schemaVersion":2,"#,
    r#""mediaType":"application/vnd.docker.distribution.manifest.v2+json"}"#
);

/// Create [`ECR_REPOSITORY`] and push its one tagged image, through Amazon ECR's own API.
async fn push_ecr_image(router: &MultiServiceRouter, case: &str) {
    ecr(
        router,
        "CreateRepository",
        json!({ "repositoryName": ECR_REPOSITORY }),
        case,
    )
    .await;
    ecr(
        router,
        "PutImage",
        json!({
            "repositoryName": ECR_REPOSITORY,
            "imageManifest": ECR_MANIFEST,
            "imageTag": ECR_TAG,
        }),
        case,
    )
    .await;
}

/// The subnets the rows place tasks in, each created through Amazon EC2's own API.
struct Network {
    /// A subnet whose route table sends `0.0.0.0/0` to an attached internet gateway.
    routed_by_gateway: String,
    /// A subnet whose route table sends `0.0.0.0/0` to a PUBLIC NAT gateway.
    routed_by_nat: String,
    /// A subnet whose route table sends `0.0.0.0/0` to a PRIVATE NAT gateway.
    routed_by_private_nat: String,
    /// A subnet whose own route table has no route for traffic leaving the VPC.
    isolated: String,
    /// A subnet of a VPC that holds the three endpoints Amazon ECR is reached through,
    /// and that is associated with no route table of its own — so its VPC's MAIN table,
    /// which has no route off the VPC either, is the one that applies.
    reached_by_endpoints: String,
}

/// The one zone every subnet below is created in, bar the ones that need their own: "if
/// you create more than one subnet in a VPC, the subnets' CIDR blocks must not overlap"
/// and a zone may hold several subnets.
fn zone(suffix: &str) -> String {
    format!("{SIGNED_REGION}{suffix}")
}

/// One subnet of `vpc_id`, created over the wire.
async fn subnet(
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone_suffix: &str,
    case: &str,
) -> String {
    let body = ec2(
        router,
        "CreateSubnet",
        &[
            ("VpcId", vpc_id),
            ("CidrBlock", cidr_block),
            ("AvailabilityZone", &zone(zone_suffix)),
        ],
        case,
    )
    .await;
    ec2_element(&body, "subnetId", case)
}

/// A route table of `vpc_id` associated with `subnet_id` — the table that then decides
/// where that subnet's traffic goes.
async fn route_table(
    router: &MultiServiceRouter,
    vpc_id: &str,
    subnet_id: &str,
    case: &str,
) -> String {
    let body = ec2(router, "CreateRouteTable", &[("VpcId", vpc_id)], case).await;
    let route_table_id = ec2_element(&body, "routeTableId", case);
    ec2(
        router,
        "AssociateRouteTable",
        &[("RouteTableId", &route_table_id), ("SubnetId", subnet_id)],
        case,
    )
    .await;
    route_table_id
}

/// Add the default route `0.0.0.0/0` to `route_table_id`, pointing at the target the
/// `(parameter, value)` pair names.
async fn default_route(
    router: &MultiServiceRouter,
    route_table_id: &str,
    target: (&str, &str),
    case: &str,
) {
    ec2(
        router,
        "CreateRoute",
        &[
            ("RouteTableId", route_table_id),
            ("DestinationCidrBlock", "0.0.0.0/0"),
            target,
        ],
        case,
    )
    .await;
}

/// One VPC endpoint of `vpc_id` for the service `service`, of `endpoint_type`.
async fn vpc_endpoint(
    router: &MultiServiceRouter,
    vpc_id: &str,
    subnet_id: &str,
    service: &str,
    endpoint_type: &str,
    case: &str,
) {
    let service_name = format!("com.amazonaws.{SIGNED_REGION}.{service}");
    let mut params: Vec<(&str, &str)> = vec![
        ("VpcId", vpc_id),
        ("ServiceName", &service_name),
        ("VpcEndpointType", endpoint_type),
    ];
    // An interface endpoint stands up an endpoint network interface in a subnet; a
    // gateway endpoint is an entry a route table routes to and stands up none.
    if endpoint_type == "Interface" {
        params.push(("SubnetId.1", subnet_id));
        // A VPC you create has DNS hostnames off, which a private hosted zone needs; these
        // endpoints are only what an image pull is reached through, so they ask for none.
        params.push(("PrivateDnsEnabled", "false"));
    }
    ec2(router, "CreateVpcEndpoint", &params, case).await;
}

/// Build the whole network the table below places tasks in, through Amazon EC2's own API.
async fn network(router: &MultiServiceRouter) -> Network {
    let case = "network";
    let vpc = ec2_element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.96.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let routed_by_gateway = subnet(router, &vpc, "10.96.0.0/20", "a", case).await;
    let routed_by_nat = subnet(router, &vpc, "10.96.16.0/20", "b", case).await;
    let routed_by_private_nat = subnet(router, &vpc, "10.96.32.0/20", "c", case).await;
    let isolated = subnet(router, &vpc, "10.96.48.0/20", "a", case).await;

    // "To use an internet gateway, your subnet's route table must contain a route that
    // directs internet-bound traffic to the internet gateway" (Amazon VPC User Guide).
    let gateway = ec2_element(
        &ec2(router, "CreateInternetGateway", &[], case).await,
        "internetGatewayId",
        case,
    );
    ec2(
        router,
        "AttachInternetGateway",
        &[("InternetGatewayId", &gateway), ("VpcId", &vpc)],
        case,
    )
    .await;
    let gateway_table = route_table(router, &vpc, &routed_by_gateway, case).await;
    default_route(router, &gateway_table, ("GatewayId", &gateway), case).await;

    // "With a public NAT gateway, internet-bound traffic from a private subnet can be
    // routed to the NAT gateway" — a public one needs an Elastic IP address.
    let address = ec2_element(
        &ec2(router, "AllocateAddress", &[("Domain", "vpc")], case).await,
        "allocationId",
        case,
    );
    let public_nat = ec2_element(
        &ec2(
            router,
            "CreateNatGateway",
            &[
                ("SubnetId", &routed_by_gateway),
                ("AllocationId", &address),
            ],
            case,
        )
        .await,
        "natGatewayId",
        case,
    );
    let nat_table = route_table(router, &vpc, &routed_by_nat, case).await;
    default_route(router, &nat_table, ("NatGatewayId", &public_nat), case).await;

    // "With a private NAT gateway, internet-bound traffic from a private subnet can't be
    // routed to the gateway" (`CreateNatGateway`).
    let private_nat = ec2_element(
        &ec2(
            router,
            "CreateNatGateway",
            &[
                ("SubnetId", &routed_by_gateway),
                ("ConnectivityType", "private"),
            ],
            case,
        )
        .await,
        "natGatewayId",
        case,
    );
    let private_nat_table = route_table(router, &vpc, &routed_by_private_nat, case).await;
    default_route(
        router,
        &private_nat_table,
        ("NatGatewayId", &private_nat),
        case,
    )
    .await;

    // The isolated subnet gets a route table of its own and no route in it, which is what
    // makes it isolated: every route table comes with the local route of its VPC alone.
    route_table(router, &vpc, &isolated, case).await;

    // A second VPC whose subnet is associated with no route table at all — so the VPC's
    // MAIN table applies, which has no route off the VPC either — holding the three
    // endpoints Amazon ECR is reached through (Amazon ECR User Guide, "Amazon ECR
    // interface VPC endpoints": interface endpoints for `ecr.api` and `ecr.dkr`, and a
    // gateway endpoint for Amazon S3, which the image layers come from).
    let endpoint_vpc = ec2_element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.97.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let reached_by_endpoints = subnet(router, &endpoint_vpc, "10.97.0.0/20", "a", case).await;
    for (service, endpoint_type) in [
        ("ecr.api", "Interface"),
        ("ecr.dkr", "Interface"),
        ("s3", "Gateway"),
    ] {
        vpc_endpoint(
            router,
            &endpoint_vpc,
            &reached_by_endpoints,
            service,
            endpoint_type,
            case,
        )
        .await;
    }

    Network {
        routed_by_gateway,
        routed_by_nat,
        routed_by_private_nat,
        isolated,
        reached_by_endpoints,
    }
}

/// Register one Fargate-compatible `awsvpc` revision of `family` running `image`.
async fn register(router: &MultiServiceRouter, family: &str, image: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [container("app", image)],
        }),
        case,
    )
    .await;
}

/// The `networkConfiguration` a launch into `subnet_id` carries.
fn placement(subnet_id: &str, assign_public_ip: &str) -> Value {
    json!({
        "awsvpcConfiguration": {
            "subnets": [subnet_id],
            "securityGroups": ["sg-0123456789abcdef0"],
            "assignPublicIp": assign_public_ip,
        }
    })
}

/// Run one task of `family` into `subnet_id` and answer its ARN.
async fn run(
    router: &MultiServiceRouter,
    family: &str,
    subnet_id: &str,
    assign_public_ip: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "launchType": "FARGATE",
            "networkConfiguration": placement(subnet_id, assign_public_ip),
        }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    let task = tasks
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected RunTask to place one task, got {body}"));
    string(task, "taskArn", case)
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that launched it.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// Poll one task until it SETTLES — running, or stopped — and answer the record it
/// settled as.
async fn settled(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let task = describe(router, task_arn, case).await;
        let status = string(&task, "lastStatus", case);
        if status == RUNNING || status == STOPPED {
            return task;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to settle at {RUNNING} or {STOPPED}, it was \
             still {status:?}: {task}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert that `task` is the terminal stop a failed image fetch of `image` earns.
fn assert_failed_to_start(task: &Value, image: &str, case: &str) {
    assert_eq!(
        string(task, "lastStatus", case),
        STOPPED,
        "case {case:?}: expected a task that cannot fetch {image} to be {STOPPED}, got {task}"
    );
    assert_eq!(
        string(task, "stopCode", case),
        TASK_FAILED_TO_START,
        "case {case:?}: expected stopCode {TASK_FAILED_TO_START}, got {task}"
    );
    let reason = string(task, "stoppedReason", case);
    assert!(
        reason.contains(CANNOT_PULL),
        "case {case:?}: expected the stoppedReason to report {CANNOT_PULL}, got {reason:?}"
    );
    assert!(
        reason.contains(image),
        "case {case:?}: expected the stoppedReason to name the image {image}, got {reason:?}"
    );
    // "The Unix timestamp for the time when the task started (the task transitioned from
    // the PENDING state to the RUNNING state)" — a task that never ran has none, and its
    // containers never exited, so they report no exit code either.
    assert!(
        task["startedAt"].is_null(),
        "case {case:?}: expected no startedAt on a task that never ran, got {task}"
    );
    for reported in list(task, "containers", case) {
        assert_eq!(
            string(&reported, "lastStatus", case),
            STOPPED,
            "case {case:?}: expected every container to be {STOPPED}, got {reported}"
        );
        assert!(
            reported["exitCode"].is_null(),
            "case {case:?}: expected no exitCode on a container that never ran, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_task_is_stopped_when_its_placement_cannot_reach_the_image_registry() {
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, "fetch").await;
    push_ecr_image(&router, "fetch").await;
    let network = network(&router).await;

    let ecr_reference = ecr_image();
    for (family, image) in [
        ("publicimage", PUBLIC_IMAGE),
        ("hubimage", HUB_IMAGE),
        ("ecrimage", ecr_reference.as_str()),
    ] {
        register(&router, family, image, "fetch").await;
    }

    // The rows: the placement a task is launched into, the image it runs, and whether the
    // image can be fetched from there. `assignPublicIp` matters only where the route out
    // is an internet gateway, which is itself one of the rows. Every row places into a
    // subnet this account HOLDS: a placement naming one Amazon VPC has no subnet for is
    // refused outright rather than launched ([`crate::task_listing_filters`] pins that), so
    // there is no fetch to decide for it.
    let rows: [(&str, &str, &str, &str, bool); 9] = [
        (
            "isolated subnet, public registry",
            "publicimage",
            &network.isolated,
            "DISABLED",
            false,
        ),
        (
            "isolated subnet, public address asked for but no route out",
            "publicimage",
            &network.isolated,
            "ENABLED",
            false,
        ),
        (
            "isolated subnet, Amazon ECR image but no endpoints in the VPC",
            "ecrimage",
            &network.isolated,
            "DISABLED",
            false,
        ),
        (
            "internet gateway with a public address",
            "publicimage",
            &network.routed_by_gateway,
            "ENABLED",
            true,
        ),
        (
            "internet gateway without a public address",
            "publicimage",
            &network.routed_by_gateway,
            "DISABLED",
            false,
        ),
        (
            "public NAT gateway",
            "publicimage",
            &network.routed_by_nat,
            "DISABLED",
            true,
        ),
        (
            "private NAT gateway",
            "publicimage",
            &network.routed_by_private_nat,
            "DISABLED",
            false,
        ),
        (
            "Amazon ECR endpoints, Amazon ECR image",
            "ecrimage",
            &network.reached_by_endpoints,
            "DISABLED",
            true,
        ),
        (
            "Amazon ECR endpoints, image resolved against Docker Hub",
            "hubimage",
            &network.reached_by_endpoints,
            "DISABLED",
            false,
        ),
    ];

    // Every task is launched first and polled afterwards: each settles on its own clock,
    // so the whole table costs one start rather than one per row.
    let mut launched = Vec::new();
    for (case, family, subnet_id, assign_public_ip, fetchable) in rows {
        let task_arn = run(&router, family, subnet_id, assign_public_ip, case).await;
        launched.push((case, family, task_arn, fetchable));
    }

    for (case, family, task_arn, fetchable) in launched {
        let image = match family {
            "publicimage" => PUBLIC_IMAGE.to_string(),
            "hubimage" => HUB_IMAGE.to_string(),
            _ => ecr_reference.clone(),
        };
        let task = settled(&router, &task_arn, case).await;
        if fetchable {
            assert_eq!(
                string(&task, "lastStatus", case),
                RUNNING,
                "case {case:?}: expected a task that can fetch {image} to reach {RUNNING}, \
                 got {task}"
            );
            assert!(
                task["stopCode"].is_null(),
                "case {case:?}: expected no stopCode on a running task, got {task}"
            );
        } else {
            assert_failed_to_start(&task, &image, case);
        }
    }
}

#[tokio::test]
async fn scaling_a_service_up_into_an_isolated_placement_stops_its_task() {
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, "service").await;
    let network = network(&router).await;
    register(&router, "servicefamily", PUBLIC_IMAGE, "service").await;

    // The two sides of the comparison: the same revision placed in a subnet with no route
    // to the registry, and in one routed to an internet gateway with a public address.
    let rows: [(&str, &str, &str, bool); 2] = [
        ("isolated service", &network.isolated, "DISABLED", false),
        (
            "routed service",
            &network.routed_by_gateway,
            "ENABLED",
            true,
        ),
    ];

    let mut scaled = Vec::new();
    for (case, subnet_id, assign_public_ip, fetchable) in rows {
        let name = format!("svc{}", scaled.len());
        // Created with no tasks at all, so the launch is the SCALE that follows.
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": name,
                "taskDefinition": "servicefamily",
                "desiredCount": 0,
                "launchType": "FARGATE",
                "networkConfiguration": placement(subnet_id, assign_public_ip),
            }),
            case,
        )
        .await;
        ok(
            &router,
            "UpdateService",
            json!({ "cluster": CLUSTER, "service": name, "desiredCount": 1 }),
            case,
        )
        .await;
        scaled.push((case, name, fetchable));
    }

    for (case, name, fetchable) in scaled {
        // The task the scheduler placed, read back through the listing that reports the
        // tasks of a service.
        let listed = ok(
            &router,
            "ListTasks",
            json!({ "cluster": CLUSTER, "serviceName": name }),
            case,
        )
        .await;
        let arns = list(&listed, "taskArns", case);
        let task_arn = arns
            .first()
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the scale to place one task, got {listed}")
            })
            .to_string();
        let task = settled(&router, &task_arn, case).await;
        if fetchable {
            assert_eq!(
                string(&task, "lastStatus", case),
                RUNNING,
                "case {case:?}: expected the routed service's task to reach {RUNNING}, got \
                 {task}"
            );
        } else {
            assert_failed_to_start(&task, PUBLIC_IMAGE, case);
            // A task that failed to start is on its way out, so the listing narrowed to
            // stopped tasks is the one that reports it — and the service holds none
            // running.
            let stopped = ok(
                &router,
                "ListTasks",
                json!({
                    "cluster": CLUSTER,
                    "serviceName": name,
                    "desiredStatus": STOPPED,
                }),
                case,
            )
            .await;
            assert!(
                list(&stopped, "taskArns", case)
                    .iter()
                    .any(|arn| arn.as_str() == Some(task_arn.as_str())),
                "case {case:?}: expected the stopped task {task_arn} in {stopped}"
            );
            let described = ok(
                &router,
                "DescribeServices",
                json!({ "cluster": CLUSTER, "services": [name] }),
                case,
            )
            .await;
            let service = list(&described, "services", case)
                .into_iter()
                .next()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the service back, got {described}")
                });
            assert_eq!(
                service["runningCount"].as_i64().unwrap_or_default(),
                0,
                "case {case:?}: expected a service whose task failed to start to run none, \
                 got {service}"
            );
        }
    }
}
