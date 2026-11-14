// SPDX-License-Identifier: CC0-1.0

//! The DEPLOYMENT CIRCUIT BREAKER: how a rollout whose tasks cannot be launched reaches a
//! TERMINAL rollout state, and where it leaves the service — driven end to end over the
//! shared wire harness in [`crate::ecs_wire`], with the Amazon EC2 service mounted beside
//! this one on the same engine so the placement every task is launched into is created
//! through EC2's OWN API.
//!
//! The rule under test, `Deployment$rolloutState`'s own contract: "When a service deployment
//! is started, it begins in an `IN_PROGRESS` state. When the service reaches a steady state,
//! the deployment transitions to a `COMPLETED` state. If the service fails to reach a steady
//! state and circuit breaker is turned on, the deployment transitions to a `FAILED` state. A
//! deployment in `FAILED` state doesn't launch any new tasks." So a rollout whose launches
//! keep failing leaves `IN_PROGRESS` once the breaker has counted as many failures as its
//! threshold tolerates — and `DeploymentCircuitBreaker$rollback` decides where that leaves
//! the service: "If rollback is on, when a service deployment fails, the service is rolled
//! back to the last deployment that completed successfully."
//!
//! The rows differ along what the rule generalises over:
//!
//! * the THRESHOLD the breaker computes, over every `ThresholdConfiguration$type` the model
//!   names (`smithy_query(query="shape", shape="ThresholdType")` -> BOUNDED_PERCENT, COUNT,
//!   UNBOUNDED_PERCENT): the documented default, which clamps to a minimum of 3; a count
//!   used directly; and an unbounded percentage of the desired count;
//! * how many failures it takes to reach that threshold — one service of three tasks, whose
//!   launches fail together, and one service of a single task, whose replacement launches
//!   have to fail one after another;
//! * whether `rollback` is on, which is the difference between a service taken back to the
//!   revision that completed and one left on the revision that failed;
//! * whether the breaker is turned on AT ALL — the row that pins the rule to the breaker
//!   rather than to the failure: "Determines whether to use the deployment circuit breaker
//!   logic for the service."
//!
//! The two tests after the table are the other side of the same rule: a REPLACEMENT launch is
//! decided against the placement as it then stands, so a caller who opens the path to the
//! registry while the breaker is still tolerating the failures has the rollout recover; and a
//! launch lost AFTER a rollout reached its steady state is not that rollout's failure, since
//! the breaker is what decides a service that fails to reach one.
//!
//! Every launch is decided by the placement it was made into, as
//! [`crate::image_pull_placement`] pins: the services below run in a subnet with no route
//! off its VPC and the three endpoints Amazon ECR publishes, so the FIRST revision's image
//! (a repository in Amazon ECR) can be fetched there and the SECOND's (a reference a
//! container runtime resolves against Docker Hub) cannot.
//!
//! Nothing here reaches into either state struct: every value asserted is read back through
//! `DescribeServices` and `DescribeServiceDeployments`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    count, create_cluster, ec2, ec2_element, ecr, emulator_with_ec2, list, ok, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster every service below runs in.
const CLUSTER: &str = "breaker";

/// The task-definition family every service below rolls out: revision 1 can be launched
/// where the services run, revision 2 cannot.
const FAMILY: &str = "breakerapp";

/// How often a poll re-reads a service.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll keeps trying before it gives up.
///
/// A failing rollout takes the whole start of an `awsvpc` task per attempt (the interface it
/// is allocated is attached one documented step at a time, and only then is the launch
/// attempted), the slowest row needs two of them, and the rollback that follows starts one
/// more task — with wide margin for a loaded test machine, since every row stops polling the
/// instant its own outcome is reached.
const POLL_BUDGET: Duration = Duration::from_secs(120);

/// `Deployment$rolloutState` of a rollout still placing tasks.
const IN_PROGRESS: &str = "IN_PROGRESS";

/// `Deployment$rolloutState` of a rollout that reached the steady state it was rolling out.
const COMPLETED: &str = "COMPLETED";

/// `Deployment$rolloutState` of a rollout the circuit breaker failed.
const FAILED: &str = "FAILED";

/// The `rolloutStateReason` such a rollout carries — the wording Amazon ECS words a
/// circuit-breaker failure in, which names the breaker that failed it.
const FAILURE_REASON: &str = "ECS deployment circuit breaker: tasks failed to start.";

/// The `statusReason` the service deployment recording a rollout the breaker ROLLED BACK
/// reports, and the `Rollback$reason` that rollback carries ("The reason the rollback
/// happened. For example, the circuit breaker initiated the rollback operation").
const ROLLED_BACK_REASON: &str =
    "Service deployment rolled back because the circuit breaker threshold was exceeded.";

/// The same for a rollout the breaker failed with no rollback configured.
const STOPPED_REASON: &str =
    "Service deployment stopped because the circuit breaker threshold was exceeded.";

/// An image reference that names no registry at all, which a container runtime resolves
/// against Docker Hub — a public registry, which publishes no VPC endpoint, so it cannot be
/// reached from a subnet with no route off its VPC.
const UNFETCHABLE_IMAGE: &str = "nginx:1.27";

/// An image in a repository of this account's Amazon ECR registry, whose registry URI is
/// `<account>.dkr.ecr.<region>.amazonaws.com` (Amazon ECR User Guide) — reachable through
/// the endpoints that service publishes, and SERVED by it once the repository holds the tag
/// ([`push_ecr_image`]).
fn fetchable_image() -> String {
    format!("{ACCOUNT}.dkr.ecr.{SIGNED_REGION}.amazonaws.com/{ECR_REPOSITORY}:{ECR_TAG}")
}

/// The repository of this account's registry the services below pull from.
const ECR_REPOSITORY: &str = "app";

/// The tag that repository's one image carries.
const ECR_TAG: &str = "1";

/// A Docker image manifest V2 schema 2 referencing no blob: what these rows are about is the
/// deployment the pull decides, not which blobs an image is built from.
const ECR_MANIFEST: &str = concat!(
    r#"{"schemaVersion":2,"#,
    r#""mediaType":"application/vnd.docker.distribution.manifest.v2+json"}"#
);

/// Create [`ECR_REPOSITORY`] and push its one tagged image, through Amazon ECR's own API — so
/// the reference [`fetchable_image`] names resolves to an image that registry actually holds.
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

/// Where the circuit breaker leaves the service, for one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The breaker failed the rollout and rolled the service back to the revision that
    /// completed, which the scheduler then runs again.
    RolledBack,
    /// The breaker failed the rollout and left the service on the revision that failed.
    Failed,
    /// No breaker is watching, so the rollout never leaves `IN_PROGRESS`.
    StillRolling,
}

/// One row: a service, the breaker it is created with, and what that breaker owes it.
struct Row {
    case: &'static str,
    service: &'static str,
    /// `Service$desiredCount` the service is created with.
    desired: i64,
    /// The `deploymentCircuitBreaker` the create configures, or `None` for a service that
    /// configures none at all.
    breaker: Option<Value>,
    /// The failure threshold that breaker computes for this desired count, as
    /// `ThresholdConfiguration$type` documents it: the percentage types multiply `value` by
    /// the desired count, `BOUNDED_PERCENT` clamping to "a minimum of 3 and a maximum of
    /// 200", and `COUNT` "uses `value` directly as the threshold".
    threshold: i64,
    outcome: Outcome,
}

/// The rows, each its own service in the one cluster.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "default threshold, rollback on, three launches that fail together",
            service: "defaultbounded",
            desired: 3,
            // "By default, the threshold configuration uses a type of BOUNDED_PERCENT with a
            // value of 50", whose product with a desired count of 3 clamps up to 3.
            breaker: Some(json!({ "enable": true, "rollback": true })),
            threshold: 3,
            outcome: Outcome::RolledBack,
        },
        Row {
            case: "count threshold of one, no rollback",
            service: "countnorollback",
            desired: 1,
            breaker: Some(json!({
                "enable": true,
                "rollback": false,
                "thresholdConfiguration": { "type": "COUNT", "value": 1 },
            })),
            threshold: 1,
            outcome: Outcome::Failed,
        },
        Row {
            case: "count threshold of two, reached by a replacement launch, rollback on",
            service: "countrollback",
            desired: 1,
            breaker: Some(json!({
                "enable": true,
                "rollback": true,
                "thresholdConfiguration": { "type": "COUNT", "value": 2 },
            })),
            threshold: 2,
            outcome: Outcome::RolledBack,
        },
        Row {
            case: "unbounded percentage of the desired count, rollback on",
            service: "unbounded",
            desired: 1,
            breaker: Some(json!({
                "enable": true,
                "rollback": true,
                "thresholdConfiguration": { "type": "UNBOUNDED_PERCENT", "value": 100 },
            })),
            threshold: 1,
            outcome: Outcome::RolledBack,
        },
        Row {
            case: "no circuit breaker configured",
            service: "nobreaker",
            desired: 1,
            breaker: None,
            threshold: 0,
            outcome: Outcome::StillRolling,
        },
    ]
}

/// A VPC and one subnet of it with no route off the VPC at all and no endpoint in it —
/// a placement no registry is reachable from — created through Amazon EC2's own API.
async fn isolated_placement(router: &MultiServiceRouter) -> (String, String) {
    let case = "network";
    let vpc = ec2_element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.98.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let subnet = ec2_element(
        &ec2(
            router,
            "CreateSubnet",
            &[
                ("VpcId", &vpc),
                ("CidrBlock", "10.98.0.0/20"),
                ("AvailabilityZone", &format!("{SIGNED_REGION}a")),
            ],
            case,
        )
        .await,
        "subnetId",
        case,
    );
    (vpc, subnet)
}

/// Open the path from `vpc` to Amazon ECR: the three endpoints that service publishes, whose
/// ids are answered so a test can close the path again.
///
/// Amazon ECR User Guide, "Amazon ECR interface VPC endpoints": interface endpoints for
/// `ecr.api` and `ecr.dkr`, and a gateway endpoint for Amazon S3, which the image layers come
/// from.
async fn open_ecr_path(router: &MultiServiceRouter, vpc: &str, subnet: &str) -> Vec<String> {
    let case = "network";
    let mut endpoints = Vec::new();
    for (service, endpoint_type) in [
        ("ecr.api", "Interface"),
        ("ecr.dkr", "Interface"),
        ("s3", "Gateway"),
    ] {
        let service_name = format!("com.amazonaws.{SIGNED_REGION}.{service}");
        let mut params: Vec<(&str, &str)> = vec![
            ("VpcId", vpc),
            ("ServiceName", &service_name),
            ("VpcEndpointType", endpoint_type),
        ];
        if endpoint_type == "Interface" {
            params.push(("SubnetId.1", subnet));
            // A VPC you create has DNS hostnames off, which a private hosted zone needs;
            // these endpoints are only what an image pull is reached through, so they ask
            // for none.
            params.push(("PrivateDnsEnabled", "false"));
        }
        let body = ec2(router, "CreateVpcEndpoint", &params, case).await;
        endpoints.push(ec2_element(&body, "vpcEndpointId", case));
    }
    endpoints
}

/// Close that path again, through Amazon EC2's own API.
async fn close_ecr_path(router: &MultiServiceRouter, endpoints: &[String]) {
    let case = "network";
    let params: Vec<(&str, &str)> = endpoints
        .iter()
        .enumerate()
        .map(|(index, id)| (ENDPOINT_PARAMETERS[index], id.as_str()))
        .collect();
    ec2(router, "DeleteVpcEndpoints", &params, case).await;
}

/// The `VpcEndpointId.<n>` parameters a `DeleteVpcEndpoints` request names its endpoints with,
/// one per endpoint [`open_ecr_path`] creates.
const ENDPOINT_PARAMETERS: [&str; 3] = [
    "VpcEndpointId.1",
    "VpcEndpointId.2",
    "VpcEndpointId.3",
];

/// The subnet the table's services place their tasks in: an isolated one whose VPC holds the
/// Amazon ECR endpoints, so the FIRST revision's image can be fetched there and the SECOND's
/// cannot.
async fn placement_subnet(router: &MultiServiceRouter) -> String {
    let (vpc, subnet) = isolated_placement(router).await;
    open_ecr_path(router, &vpc, &subnet).await;
    subnet
}

/// Register one Fargate-compatible `awsvpc` revision of [`FAMILY`] running `image`, and
/// answer its ARN.
async fn register(router: &MultiServiceRouter, image: &str, case: &str) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [container("app", image)],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Read one service back through `DescribeServices`.
async fn describe(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {body}"))
}

/// The service's current (`PRIMARY`) deployment, which is the first of the deployments it
/// reports.
fn primary(service: &Value, case: &str) -> Value {
    list(service, "deployments", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected a deployment, got {service}"))
}

/// Read one service deployment back through `DescribeServiceDeployments`.
async fn describe_deployment(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [arn] }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the rollout back, got {body}"))
}

/// Poll one service until `settled` holds of it, and answer the service it settled as.
async fn poll_until(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
    what: &str,
    settled: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let service = describe(router, name, case).await;
        if settled(&service) {
            return service;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {what}, got {service}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_deployment_circuit_breaker_takes_a_failing_rollout_to_a_terminal_state() {
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, "breaker").await;
    push_ecr_image(&router, "breaker").await;
    let subnet = placement_subnet(&router).await;
    let first = register(&router, &fetchable_image(), "revisions").await;
    let second = register(&router, UNFETCHABLE_IMAGE, "revisions").await;

    let rows = rows();

    // Every service is created on the revision it CAN run, so each has a deployment that
    // completed successfully — the one a rollback puts back.
    for row in &rows {
        let mut configuration = json!({});
        if let Some(breaker) = row.breaker.clone() {
            configuration["deploymentCircuitBreaker"] = breaker;
        }
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": row.service,
                "taskDefinition": first,
                "desiredCount": row.desired,
                "launchType": "FARGATE",
                "deploymentController": { "type": "ECS" },
                "deploymentConfiguration": configuration,
                "networkConfiguration": {
                    "awsvpcConfiguration": {
                        "subnets": [subnet],
                        "securityGroups": ["sg-0123456789abcdef0"],
                    }
                },
            }),
            row.case,
        )
        .await;
    }

    // The steady state the first rollout reaches, which is what makes it "the last
    // deployment that completed successfully".
    let mut completed_revisions = Vec::new();
    for row in &rows {
        let desired = row.desired;
        let service = poll_until(
            &router,
            row.service,
            row.case,
            "the first rollout to complete with every task running",
            move |service| {
                let deployment = primary(service, "steady");
                string(&deployment, "rolloutState", "steady") == COMPLETED
                    && count(service, "runningCount", "steady") == desired
            },
        )
        .await;
        // `ServiceCurrentRevisionSummary$arn`: "The ARN of the current service revision" —
        // the revision a rollback of the next rollout has to put back.
        let revision = list(&service, "currentServiceRevisions", row.case)
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!("case {:?}: expected a current service revision, got {service}", row.case)
            });
        completed_revisions.push(string(&revision, "arn", row.case));
    }

    // The rollout that cannot be launched: the revision whose image no path from this
    // placement reaches.
    let mut failing = Vec::new();
    for row in &rows {
        let body = ok(
            &router,
            "UpdateService",
            json!({
                "cluster": CLUSTER,
                "service": row.service,
                "taskDefinition": second,
            }),
            row.case,
        )
        .await;
        let service = &body["service"];
        assert_eq!(
            string(service, "taskDefinition", row.case),
            second,
            "case {:?}: expected the update to move the service onto the revision it asked \
             for, got {service}",
            row.case
        );
        failing.push(string(service, "currentServiceDeployment", row.case));
    }

    // Each row reaches the end its own breaker owes it.
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let revision = completed_revisions[index].clone();
        let rollout = failing[index].clone();
        match row.outcome {
            Outcome::RolledBack => {
                let first_arn = first.clone();
                let desired = row.desired;
                let service = poll_until(
                    &router,
                    row.service,
                    case,
                    "the service to be rolled back to the revision that completed, running \
                     the task count it wants",
                    move |service| {
                        let deployment = primary(service, "rolled back");
                        string(service, "taskDefinition", "rolled back") == first_arn
                            && string(&deployment, "rolloutState", "rolled back") == COMPLETED
                            && count(service, "runningCount", "rolled back") == desired
                    },
                )
                .await;
                let deployment = primary(&service, case);
                assert_eq!(
                    string(&deployment, "taskDefinition", case),
                    first,
                    "case {case:?}: expected the PRIMARY deployment to run the revision that \
                     completed, got {deployment}"
                );
                assert_eq!(
                    count(&deployment, "runningCount", case),
                    row.desired,
                    "case {case:?}: expected the rolled-back deployment to converge on the \
                     desired count, got {deployment}"
                );
                // The rollout the breaker failed, as a resource of its own.
                let record = describe_deployment(&router, &rollout, case).await;
                assert_eq!(
                    string(&record, "status", case),
                    "ROLLBACK_SUCCESSFUL",
                    "case {case:?}: expected the failed rollout to end rolled back, got {record}"
                );
                assert_eq!(
                    string(&record, "statusReason", case),
                    ROLLED_BACK_REASON,
                    "case {case:?}: expected the status reason to report the rollback the \
                     breaker made, got {record}"
                );
                let rollback = &record["rollback"];
                assert_eq!(
                    string(rollback, "reason", case),
                    ROLLED_BACK_REASON,
                    "case {case:?}: expected the rollback to name the circuit breaker, got \
                     {record}"
                );
                assert_eq!(
                    string(rollback, "serviceRevisionArn", case),
                    revision,
                    "case {case:?}: expected the rollback to deploy the revision that \
                     completed, got {record}"
                );
                assert_eq!(
                    string(&record["deploymentCircuitBreaker"], "status", case),
                    "TRIGGERED",
                    "case {case:?}: expected the breaker to report that it fired, got \
                     {record}"
                );
                assert!(
                    count(&record["deploymentCircuitBreaker"], "failureCount", case)
                        >= row.threshold,
                    "case {case:?}: expected at least {} counted failures, got {record}",
                    row.threshold
                );
                assert_eq!(
                    count(&record["deploymentCircuitBreaker"], "threshold", case),
                    row.threshold,
                    "case {case:?}: expected the documented threshold, got {record}"
                );
            }
            Outcome::Failed => {
                let service = poll_until(
                    &router,
                    row.service,
                    case,
                    "the rollout to reach the terminal FAILED state",
                    |service| {
                        string(&primary(service, "failed"), "rolloutState", "failed") == FAILED
                    },
                )
                .await;
                let deployment = primary(&service, case);
                assert_eq!(
                    string(&deployment, "rolloutStateReason", case),
                    FAILURE_REASON,
                    "case {case:?}: expected the reason to name the circuit breaker, got \
                     {deployment}"
                );
                assert_eq!(
                    string(&deployment, "taskDefinition", case),
                    second,
                    "case {case:?}: expected a service with no rollback to keep the revision \
                     that failed, got {deployment}"
                );
                assert_eq!(
                    string(&service, "taskDefinition", case),
                    second,
                    "case {case:?}: expected the service itself to keep that revision, got \
                     {service}"
                );
                assert!(
                    count(&deployment, "failedTasks", case) >= row.threshold,
                    "case {case:?}: expected at least {} failed tasks counted, got {deployment}",
                    row.threshold
                );
                let record = describe_deployment(&router, &rollout, case).await;
                assert_eq!(
                    string(&record, "status", case),
                    "STOPPED",
                    "case {case:?}: expected the failed rollout to end stopped, got {record}"
                );
                assert_eq!(
                    string(&record, "statusReason", case),
                    STOPPED_REASON,
                    "case {case:?}: expected the status reason to name the circuit breaker, \
                     got {record}"
                );
                assert_eq!(
                    string(&record["deploymentCircuitBreaker"], "status", case),
                    "TRIGGERED",
                    "case {case:?}: expected the breaker to report that it fired, got \
                     {record}"
                );
                assert!(
                    record["rollback"].is_null(),
                    "case {case:?}: expected no rollback on a rollout nothing rolled back, \
                     got {record}"
                );
                assert_eq!(
                    count(&record["deploymentCircuitBreaker"], "threshold", case),
                    row.threshold,
                    "case {case:?}: expected the documented threshold, got {record}"
                );
            }
            Outcome::StillRolling => {
                // Read LAST, after every watched row above has reached its end — so the
                // rollout has had at least as long as a breaker needs and still carries no
                // terminal state, which is what "no circuit breaker" means.
                let service = describe(&router, row.service, case).await;
                let deployment = primary(&service, case);
                assert_eq!(
                    string(&deployment, "rolloutState", case),
                    IN_PROGRESS,
                    "case {case:?}: expected a rollout no breaker watches to stay rolling, \
                     got {deployment}"
                );
                assert_eq!(
                    string(&service, "taskDefinition", case),
                    second,
                    "case {case:?}: expected the service to keep the revision it was moved \
                     onto, got {service}"
                );
                let record = describe_deployment(&router, &rollout, case).await;
                assert_eq!(
                    string(&record, "status", case),
                    "IN_PROGRESS",
                    "case {case:?}: expected the rollout's own record to stay under way, got \
                     {record}"
                );
                assert_eq!(
                    string(&record["deploymentCircuitBreaker"], "status", case),
                    "DISABLED",
                    "case {case:?}: expected a breaker nothing turned on to be DISABLED, got \
                     {record}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_replacement_launch_is_decided_against_the_placement_as_it_then_stands() {
    let case = "replacement launch";
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, case).await;
    push_ecr_image(&router, case).await;
    // A placement no registry is reachable from, so the rollout's first launch fails.
    let (vpc, subnet) = isolated_placement(&router).await;
    let revision = register(&router, &fetchable_image(), case).await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "recovers",
            "taskDefinition": revision,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "deploymentConfiguration": {
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": true,
                    // Three failures tolerated, so the breaker is still watching when the
                    // path below is opened.
                    "thresholdConfiguration": { "type": "COUNT", "value": 3 },
                },
            },
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [subnet],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
        }),
        case,
    )
    .await;

    // The breaker counts the launch the rollout lost, and tolerates it.
    let service = poll_until(
        &router,
        "recovers",
        case,
        "the rollout's first launch to be counted as failed",
        |service| count(&primary(service, "counted"), "failedTasks", "counted") >= 1,
    )
    .await;
    let deployment = primary(&service, case);
    assert_eq!(
        string(&deployment, "rolloutState", case),
        IN_PROGRESS,
        "case {case:?}: expected one counted failure to leave the rollout under way, got \
         {deployment}"
    );

    // The path to the registry is opened WHILE the rollout is still being retried, so the
    // replacement launch is attempted over it: the rollout recovers instead of being failed,
    // and the service runs the task it wanted.
    open_ecr_path(&router, &vpc, &subnet).await;
    let service = poll_until(
        &router,
        "recovers",
        case,
        "the rollout to complete once its launches can fetch their image",
        |service| {
            string(&primary(service, "recovered"), "rolloutState", "recovered") == COMPLETED
                && count(service, "runningCount", "recovered") == 1
        },
    )
    .await;
    let deployment = primary(&service, case);
    assert_eq!(
        string(&deployment, "rolloutStateReason", case),
        format!(
            "ECS deployment {} completed.",
            string(&deployment, "id", case)
        ),
        "case {case:?}: expected the rollout that recovered to report its own completion, \
         got {deployment}"
    );
    assert_eq!(
        count(&deployment, "runningCount", case),
        1,
        "case {case:?}: expected the replacement launch to be running, got {deployment}"
    );
}

#[tokio::test]
async fn a_launch_lost_after_the_rollout_completed_does_not_fail_that_rollout() {
    let case = "launch lost after the rollout completed";
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, case).await;
    push_ecr_image(&router, case).await;
    let (vpc, subnet) = isolated_placement(&router).await;
    let endpoints = open_ecr_path(&router, &vpc, &subnet).await;
    let revision = register(&router, &fetchable_image(), case).await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "settled",
            "taskDefinition": revision,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "deploymentConfiguration": {
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": true,
                    "thresholdConfiguration": { "type": "COUNT", "value": 1 },
                },
            },
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [subnet],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
        }),
        case,
    )
    .await;
    let settled = poll_until(
        &router,
        "settled",
        case,
        "the rollout to reach the steady state it was rolling out",
        |service| {
            string(&primary(service, "settled"), "rolloutState", "settled") == COMPLETED
                && count(service, "runningCount", "settled") == 1
        },
    )
    .await;
    let deployment_id = string(&primary(&settled, case), "id", case);
    let rollout = string(&settled, "currentServiceDeployment", case);

    // The path to the registry is closed and the service is scaled out, so the launch the
    // scheduler makes for the SAME deployment fails — after that deployment had already
    // reached its steady state. The breaker fails a rollout that cannot reach one; it does
    // not take back the one this rollout did reach.
    close_ecr_path(&router, &endpoints).await;
    ok(
        &router,
        "UpdateService",
        json!({ "cluster": CLUSTER, "service": "settled", "desiredCount": 2 }),
        case,
    )
    .await;
    let service = poll_until(
        &router,
        "settled",
        case,
        "the launch the scale-out lost to be counted",
        |service| count(&primary(service, "lost"), "failedTasks", "lost") >= 1,
    )
    .await;
    let deployment = primary(&service, case);
    // Scaling a service that has settled puts the scheduler back to work, so the rollout is
    // under way again — what it must NOT be is failed: the breaker fails a rollout that
    // cannot reach a steady state, not one that lost a launch after reaching one.
    assert_eq!(
        string(&deployment, "rolloutState", case),
        IN_PROGRESS,
        "case {case:?}: expected the rollout that reached its steady state not to be failed, \
         got {deployment}"
    );
    assert_ne!(
        string(&deployment, "rolloutState", case),
        FAILED,
        "case {case:?}: expected the breaker to leave a settled rollout alone, got {deployment}"
    );
    assert_eq!(
        string(&deployment, "id", case),
        deployment_id,
        "case {case:?}: expected no new deployment, got {deployment}"
    );
    assert_eq!(
        string(&service, "taskDefinition", case),
        revision,
        "case {case:?}: expected the service to keep the revision it completed, got {service}"
    );
    let record = describe_deployment(&router, &rollout, case).await;
    assert_eq!(
        string(&record, "status", case),
        "SUCCESSFUL",
        "case {case:?}: expected the rollout's own record to stay successful, got {record}"
    );
    assert!(
        record["rollback"].is_null(),
        "case {case:?}: expected no rollback of a rollout that succeeded, got {record}"
    );
}
