// SPDX-License-Identifier: CC0-1.0

//! WHICH `UpdateService` calls RELEASE a service revision, driven end to end over the
//! shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: a service deployment is the release of a service REVISION, so
//! `UpdateService` records one — and mints a revision for it — exactly when the call leaves
//! the service with a workload configuration it was not already running; a call that only
//! re-states the configuration the service holds, scales it, or forces its tasks to be
//! replaced rolls tasks (a new `Deployment` of the revision already deployed) and records
//! no service deployment at all.
//!
//! The docs the rows are read off:
//!
//! * `ServiceRevision`: "A service revision contains a record of the workload configuration
//!   Amazon ECS is attempting to deploy. Whenever you create or deploy a service, Amazon ECS
//!   automatically creates and captures the configuration that you're trying to deploy in
//!   the service revision."
//! * `ListServiceDeployments`: "A service deployment happens when you release a software
//!   update for the service."
//! * `UpdateService$forceNewDeployment`: "Determines whether to force a new deployment of
//!   the service. By default, deployments aren't forced. You can use this option to start a
//!   new deployment with no service definition changes. For example, you can update a
//!   service's tasks to use a newer Docker image with the same image/tag combination
//!   (`my_image:latest`) or to roll Fargate tasks onto a newer platform version."
//! * `UpdateService$taskDefinition`: "If a revision isn't specified, the latest `ACTIVE`
//!   revision is used."
//! * `UpdateService$desiredCount`, `$deploymentConfiguration`: members whose own docs do
//!   not name a deployment as what stating them starts.
//! * `Service$currentServiceDeployment`: "The ARN of the current service deployment."
//! * `UpdateService$platformVersion`: "The platform version that your tasks in the service
//!   run on", whose only published Linux value beside the `LATEST` alias is the latest one
//!   (smithy_query query="doc" shape="UpdateServiceRequest" member="platformVersion").
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{create_cluster, list, ok, service, string, AWSVPC_SUBNET};

/// The cluster every service below is created in.
const CLUSTER: &str = "releases";

/// The Fargate platform version a Linux revision can be PINNED to: the one `LATEST`
/// resolves to for a Linux task, which is the only published Linux version a service may
/// name (the others are retired).
const PINNED_PLATFORM_VERSION: &str = "1.4.0";

/// A second subnet, so a row can move the service onto a network configuration that is not
/// the one it holds.
const OTHER_SUBNET: &str = "subnet-0123456789abcdef1";

/// What ONE `UpdateService` call did to the service's release history, read back through
/// the three places it shows: the deployments `ListServiceDeployments` reports, the ARN the
/// service reports as its current service deployment, and the revision it reports as
/// current.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Release {
    deployments: usize,
    current_deployment: Option<String>,
    current_revision: String,
    /// The id of the service's `PRIMARY` `Deployment`: a forced redeployment starts a new
    /// rolling update, so this moves even where nothing is released.
    primary_deployment_id: String,
}

/// One row of the release table: the members the update adds to the request, whether the
/// call RELEASES a revision (and so records a service deployment), and whether it starts a
/// new rolling update of the service's tasks.
struct Row {
    name: &'static str,
    patch: Value,
    releases: bool,
    rolls_tasks: bool,
}

/// Register one Fargate-capable Linux revision of `family` and return its ARN.
async fn register_revision(router: &MultiServiceRouter, family: &str, case: &str) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
            "runtimePlatform": {
                "cpuArchitecture": "X86_64",
                "operatingSystemFamily": "LINUX",
            },
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Create a Fargate service the ECS scheduler rolls out, and return its projection.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": definition,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": { "subnets": [AWSVPC_SUBNET] },
            },
        }),
        case,
    )
    .await;
    body["service"].clone()
}

/// The id of the one `PRIMARY` deployment of a reported service.
fn primary_deployment_id(reported: &Value, case: &str) -> String {
    let deployments = list(reported, "deployments", case);
    let primary: Vec<&Value> = deployments
        .iter()
        .filter(|deployment| string(deployment, "status", case) == "PRIMARY")
        .collect();
    assert_eq!(
        primary.len(),
        1,
        "case {case:?}: a service has exactly one PRIMARY deployment, got {deployments:?}"
    );
    string(primary[0], "id", case)
}

/// What the service's release history reads as NOW, read through `ListServiceDeployments`
/// and `DescribeServices`.
async fn release(router: &MultiServiceRouter, name: &str, case: &str) -> Release {
    let listed = ok(
        router,
        "ListServiceDeployments",
        json!({ "cluster": CLUSTER, "service": name }),
        case,
    )
    .await;
    let described = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    let services = list(&described, "services", case);
    let reported = services
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be described, got {described}"))
        .clone();
    Release {
        deployments: list(&listed, "serviceDeployments", case).len(),
        current_deployment: reported["currentServiceDeployment"]
            .as_str()
            .map(str::to_string),
        current_revision: string(&list(&reported, "currentServiceRevisions", case)[0], "arn", case),
        primary_deployment_id: primary_deployment_id(&reported, case),
    }
}

#[tokio::test]
async fn only_an_update_that_changes_the_workload_configuration_releases_a_revision() {
    let router = service();
    let case = "setup";
    create_cluster(&router, CLUSTER, case).await;
    let first_revision = register_revision(&router, "releasefamily", case).await;
    let second_revision = register_revision(&router, "releasefamily", case).await;

    // The rows differ along WHAT the update states: a member re-stated as the service
    // already holds it, the same member carrying another value, a member whose doc names no
    // deployment, and the force flag that starts one without any change at all.
    let rows: [Row; 10] = [
        Row {
            name: "an update that states nothing",
            patch: json!({}),
            releases: false,
            rolls_tasks: false,
        },
        Row {
            name: "the task definition revision the service already runs",
            patch: json!({ "taskDefinition": first_revision.clone() }),
            releases: false,
            rolls_tasks: false,
        },
        Row {
            name: "the network configuration the service already holds",
            patch: json!({ "networkConfiguration": {
                "awsvpcConfiguration": { "subnets": [AWSVPC_SUBNET] },
            } }),
            releases: false,
            rolls_tasks: false,
        },
        Row {
            name: "a desired count of its own",
            patch: json!({ "desiredCount": 2 }),
            releases: false,
            rolls_tasks: false,
        },
        Row {
            name: "a deployment configuration of its own",
            patch: json!({ "deploymentConfiguration": { "maximumPercent": 150 } }),
            releases: false,
            rolls_tasks: false,
        },
        Row {
            name: "forceNewDeployment with no change beside it",
            patch: json!({ "forceNewDeployment": true }),
            releases: false,
            rolls_tasks: true,
        },
        Row {
            name: "another task definition revision",
            patch: json!({ "taskDefinition": second_revision.clone() }),
            releases: true,
            rolls_tasks: true,
        },
        Row {
            name: "a platform version pinned off the LATEST alias",
            patch: json!({ "platformVersion": PINNED_PLATFORM_VERSION }),
            releases: true,
            rolls_tasks: true,
        },
        Row {
            name: "the platform version the service now holds, re-stated",
            patch: json!({ "platformVersion": PINNED_PLATFORM_VERSION }),
            releases: false,
            rolls_tasks: false,
        },
        Row {
            name: "another network configuration",
            patch: json!({ "networkConfiguration": {
                "awsvpcConfiguration": { "subnets": [OTHER_SUBNET] },
            } }),
            releases: true,
            rolls_tasks: true,
        },
    ];

    let created = create_service(&router, "web", &first_revision, case).await;
    // The create released the service's first revision, so it is the service deployment the
    // service starts out on.
    let case = "the create's own release";
    let mut before = release(&router, "web", case).await;
    assert_eq!(
        before.deployments, 1,
        "case {case:?}: expected the create to record one service deployment, got {before:?}"
    );
    assert_eq!(
        before.current_deployment.as_deref(),
        Some(string(&created, "currentServiceDeployment", case).as_str()),
        "case {case:?}: the listed deployment is the one the create reported, got {before:?}"
    );

    for row in &rows {
        let case = row.name;
        let mut request = json!({ "cluster": CLUSTER, "service": "web" });
        for (member, value) in row.patch.as_object().expect("the patch is an object") {
            request[member] = value.clone();
        }
        let updated = ok(&router, "UpdateService", request, case).await;
        let after = release(&router, "web", case).await;

        assert_eq!(
            after.deployments,
            before.deployments + usize::from(row.releases),
            "case {case:?}: expected {} service deployment(s), got {after:?}",
            before.deployments + usize::from(row.releases)
        );
        if row.releases {
            assert_ne!(
                after.current_revision, before.current_revision,
                "case {case:?}: a released revision is a new one, got {after:?}"
            );
            assert_ne!(
                after.current_deployment, before.current_deployment,
                "case {case:?}: a release is recorded under a deployment of its own, got \
                 {after:?}"
            );
        } else {
            assert_eq!(
                after.current_revision, before.current_revision,
                "case {case:?}: a call that releases nothing deploys the revision already \
                 deployed, got {after:?}"
            );
            assert_eq!(
                after.current_deployment, before.current_deployment,
                "case {case:?}: a call that releases nothing leaves the service on the \
                 service deployment it was on, got {after:?}"
            );
        }
        assert!(
            after.current_deployment.is_some(),
            "case {case:?}: a service the scheduler rolls out is on a service deployment, \
             got {after:?}"
        );
        // Whatever it released, the update's own answer reports the same two ARNs the reads
        // report, so a caller acting on the response and a caller polling the reads agree.
        assert_eq!(
            (
                updated["service"]["currentServiceDeployment"]
                    .as_str()
                    .map(str::to_string),
                string(
                    &list(&updated["service"], "currentServiceRevisions", case)[0],
                    "arn",
                    case
                ),
            ),
            (
                after.current_deployment.clone(),
                after.current_revision.clone()
            ),
            "case {case:?}: UpdateService answers with the release the reads report, got \
             {updated}"
        );
        assert_eq!(
            after.primary_deployment_id != before.primary_deployment_id,
            row.rolls_tasks,
            "case {case:?}: expected rolls_tasks={}, got {after:?} against {before:?}",
            row.rolls_tasks
        );
        before = after;
    }
}
