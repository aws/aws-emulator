// SPDX-License-Identifier: CC0-1.0

//! The ECS SERVICE lifecycle, driven end to end over HTTP: `CreateService`,
//! `DescribeServices` and `UpdateService`.
//!
//! The rule under test: an ECS service is a persisted, cluster-scoped record, and the ECS
//! service scheduler keeps `desiredCount` tasks of the resolved task-definition revision
//! running for it, so every number a response carries (`runningCount`, `pendingCount`,
//! the deployment's `rolloutState`, the cluster's `activeServicesCount`) is COUNTED off
//! the records that exist rather than echoed from the request.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, count, list, ok, refused, register_capacity, service, string,
    ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// Long enough for one clock-owed task transition (`state::TRANSITION_SPAN` is 500 ms)
/// to fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// Long enough for every clock-owed transition the START of a task here takes: the
/// services below all run `awsvpc` revisions, so each of their tasks is placed only once
/// the interface it was allocated is attached ([`crate::ecs_wire::AWSVPC_START`]).
const TASK_START: Duration = crate::ecs_wire::AWSVPC_START;

/// How a case names the task definition its service should run: the three forms the
/// `taskDefinition` doc allows ("The family and revision (family:revision) or full ARN of
/// the task definition").
#[derive(Debug, Clone, Copy)]
enum Reference {
    Family,
    FamilyRevision,
    Arn,
}

/// One row of the create table.
#[derive(Debug)]
struct CreateCase {
    name: &'static str,
    cluster: &'static str,
    service_name: &'static str,
    desired: i64,
    reference: Reference,
    launch_type: Option<&'static str>,
    capacity_provider: Option<&'static str>,
    /// The `launchType` the stored service must report back, if any.
    expect_launch_type: Option<&'static str>,
    /// The capacity provider the stored service must report in its strategy, if any.
    expect_capacity_provider: Option<&'static str>,
}

/// Register one revision of `family` with a single container, Fargate-compatible
/// (`awsvpc` plus task-level cpu/memory) so a case may run it under either launch type.
/// Returns `(taskDefinitionArn, revision)`.
async fn register(router: &MultiServiceRouter, family: &str, case: &str) -> (String, i64) {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
    let definition = &body["taskDefinition"];
    (
        string(definition, "taskDefinitionArn", case),
        count(definition, "revision", case),
    )
}

/// The ARNs `ListTasks` reports for the tasks of one service — the tasks its scheduler
/// actually placed and still wants.
async fn service_task_arns(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "ListTasks",
        json!({ "cluster": cluster, "serviceName": service_name }),
        case,
    )
    .await;
    list(&body, "taskArns", case)
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Describe one service and return it.
async fn describe(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    include_tags: bool,
    case: &str,
) -> Value {
    let mut request = json!({ "cluster": cluster, "services": [service_name] });
    if include_tags {
        request["include"] = json!(["TAGS"]);
    }
    let body = ok(router, "DescribeServices", request, case).await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected no failures describing a stored service, got {body}"
    );
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly one described service, got {body}"
    );
    services[0].clone()
}

/// Re-read the task set of one service until it holds `wanted` tasks, NONE of which is
/// one of `previous`, and answer it.
///
/// What a rolling update leaves behind: the replacements are started first and the tasks
/// they replace are retired once they are up, so the inherited set is replaced over
/// several scheduler steps rather than in the instant the update answers.
async fn poll_replaced_task_set(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
    previous: &[String],
    wanted: usize,
) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let tasks = service_task_arns(router, cluster, service_name, case).await;
        if tasks.len() == wanted && tasks.iter().all(|arn| !previous.contains(arn)) {
            return tasks;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the rollout to replace the task set {previous:?} with \
             {wanted} new tasks, got {tasks:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn service_create_reads_back_with_the_task_set_its_scheduler_placed() {
    let router = service();

    // One cluster whose default capacity-provider strategy is set, and one without, so
    // the table can span both sides of "If no capacityProviderStrategy or launchType is
    // specified, the defaultCapacityProviderStrategy for the cluster is used".
    ok(
        &router,
        "CreateCluster",
        json!({
            "clusterName": "withdefault",
            "capacityProviders": ["FARGATE"],
            "defaultCapacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }],
        }),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "nodefault" }),
        "setup",
    )
    .await;
    // "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
    // cluster", so the rows that place EC2 tasks need capacity registered to run on.
    register_capacity(&router, "nodefault", "setup").await;
    let (arn_default, revision_default) = register(&router, "withdefaultfamily", "setup").await;
    let (arn_plain, revision_plain) = register(&router, "nodefaultfamily", "setup").await;

    // The rule generalises over the task COUNT the scheduler keeps running and over the
    // form the task definition and the capacity are named in, so the rows differ along
    // exactly those.
    let cases = [
        CreateCase {
            name: "zero tasks, explicit launch type",
            cluster: "nodefault",
            service_name: "zero",
            desired: 0,
            reference: Reference::Family,
            launch_type: Some("EC2"),
            capacity_provider: None,
            expect_launch_type: Some("EC2"),
            expect_capacity_provider: None,
        },
        CreateCase {
            name: "one task, family:revision",
            cluster: "nodefault",
            service_name: "one",
            desired: 1,
            reference: Reference::FamilyRevision,
            launch_type: Some("EC2"),
            capacity_provider: None,
            expect_launch_type: Some("EC2"),
            expect_capacity_provider: None,
        },
        CreateCase {
            name: "two tasks, full task-definition ARN",
            cluster: "nodefault",
            service_name: "two",
            desired: 2,
            reference: Reference::Arn,
            launch_type: Some("FARGATE"),
            capacity_provider: None,
            expect_launch_type: Some("FARGATE"),
            expect_capacity_provider: None,
        },
        CreateCase {
            name: "no launch type and no cluster default falls back to EC2",
            cluster: "nodefault",
            service_name: "fallback",
            desired: 3,
            reference: Reference::Family,
            launch_type: None,
            capacity_provider: None,
            expect_launch_type: Some("EC2"),
            expect_capacity_provider: None,
        },
        CreateCase {
            name: "explicit capacity provider strategy",
            cluster: "withdefault",
            service_name: "strategy",
            desired: 4,
            reference: Reference::Family,
            launch_type: None,
            capacity_provider: Some("FARGATE"),
            expect_launch_type: None,
            expect_capacity_provider: Some("FARGATE"),
        },
        CreateCase {
            name: "neither names capacity, so the cluster default is used",
            cluster: "withdefault",
            service_name: "clusterdefault",
            desired: 5,
            reference: Reference::Family,
            launch_type: None,
            capacity_provider: None,
            expect_launch_type: None,
            expect_capacity_provider: Some("FARGATE"),
        },
    ];

    // `(case, cluster, service, desiredCount)` for every row, settled in ONE span below.
    let mut settling: Vec<(&str, &str, &str, i64)> = Vec::new();
    for test in cases {
        let case = test.name;
        let (family, definition_arn, revision) = if test.cluster == "withdefault" {
            ("withdefaultfamily", arn_default.as_str(), revision_default)
        } else {
            ("nodefaultfamily", arn_plain.as_str(), revision_plain)
        };
        let reference = match test.reference {
            Reference::Family => family.to_string(),
            Reference::FamilyRevision => format!("{family}:{revision}"),
            Reference::Arn => definition_arn.to_string(),
        };

        let mut request = json!({
            "cluster": test.cluster,
            "serviceName": test.service_name,
            "taskDefinition": reference,
            "desiredCount": test.desired,
            "networkConfiguration": awsvpc_network_configuration(),
            "tags": [{ "key": "env", "value": "test" }],
        });
        if let Some(launch_type) = test.launch_type {
            request["launchType"] = json!(launch_type);
        }
        if let Some(provider) = test.capacity_provider {
            request["capacityProviderStrategy"] =
                json!([{ "capacityProvider": provider, "weight": 1 }]);
        }

        let created = ok(&router, "CreateService", request, case).await;
        let created = &created["service"];
        assert_eq!(
            string(created, "serviceArn", case),
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{}/{}",
                test.cluster, test.service_name
            ),
            "case {case:?}: unexpected serviceArn in {created}"
        );
        assert_eq!(
            string(created, "serviceName", case),
            test.service_name,
            "case {case:?}: unexpected serviceName in {created}"
        );
        assert_eq!(
            string(created, "clusterArn", case),
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{}", test.cluster),
            "case {case:?}: unexpected clusterArn in {created}"
        );
        assert_eq!(
            string(created, "status", case),
            "ACTIVE",
            "case {case:?}: expected a new service to be ACTIVE, got {created}"
        );
        assert_eq!(
            string(created, "taskDefinition", case),
            definition_arn,
            "case {case:?}: expected the resolved revision ARN, got {created}"
        );
        assert_eq!(
            count(created, "desiredCount", case),
            test.desired,
            "case {case:?}: expected desiredCount {}, got {created}",
            test.desired
        );
        // The scheduler has just placed the task set, so every task it wants is PENDING
        // and none is RUNNING yet: the clock, not the create, starts them.
        assert_eq!(
            count(created, "pendingCount", case),
            test.desired,
            "case {case:?}: expected pendingCount {}, got {created}",
            test.desired
        );
        assert_eq!(
            count(created, "runningCount", case),
            0,
            "case {case:?}: expected runningCount 0 on a just-created service, got {created}"
        );
        assert_eq!(
            created["launchType"].as_str(),
            test.expect_launch_type,
            "case {case:?}: unexpected launchType in {created}"
        );
        let strategy = list(created, "capacityProviderStrategy", case);
        match test.expect_capacity_provider {
            Some(provider) => assert_eq!(
                strategy
                    .first()
                    .and_then(|item| item["capacityProvider"].as_str()),
                Some(provider),
                "case {case:?}: unexpected capacityProviderStrategy in {created}"
            ),
            None => assert!(
                strategy.is_empty(),
                "case {case:?}: a service with a launchType reports no \
                 capacityProviderStrategy, got {created}"
            ),
        }
        // The documented defaults the response always carries.
        assert_eq!(
            string(created, "schedulingStrategy", case),
            "REPLICA",
            "case {case:?}: unexpected schedulingStrategy in {created}"
        );
        assert_eq!(
            string(created, "propagateTags", case),
            "NONE",
            "case {case:?}: unexpected propagateTags in {created}"
        );
        // A create that names no `availabilityZoneRebalancing` resolves the member's
        // documented create default, wherever the service's tasks are placed.
        assert_eq!(
            string(created, "availabilityZoneRebalancing", case),
            "ENABLED",
            "case {case:?}: expected availabilityZoneRebalancing ENABLED on a create that \
             named none, got {created}"
        );
        let configuration = &created["deploymentConfiguration"];
        assert_eq!(
            count(configuration, "maximumPercent", case),
            200,
            "case {case:?}: unexpected maximumPercent in {created}"
        );
        assert_eq!(
            count(configuration, "minimumHealthyPercent", case),
            100,
            "case {case:?}: unexpected minimumHealthyPercent in {created}"
        );

        // The ECS deployment controller owns one PRIMARY deployment, whose rollout state
        // follows the counts rather than being asserted.
        let deployments = list(created, "deployments", case);
        assert_eq!(
            deployments.len(),
            1,
            "case {case:?}: expected one deployment on an ECS-controller service, got {created}"
        );
        let deployment = &deployments[0];
        let deployment_id = string(deployment, "id", case);
        assert!(
            deployment_id.starts_with("ecs-svc/"),
            "case {case:?}: expected an ecs-svc/ deployment id, got {deployment}"
        );
        assert_eq!(
            string(deployment, "status", case),
            "PRIMARY",
            "case {case:?}: unexpected deployment status in {deployment}"
        );
        assert_eq!(
            string(deployment, "rolloutState", case),
            "IN_PROGRESS",
            "case {case:?}: a deployment the create just started is IN_PROGRESS until \
             the clock settles it, got {deployment}"
        );

        // The scheduler placed REAL task records: ListTasks reports one per wanted task.
        let placed = service_task_arns(&router, test.cluster, test.service_name, case).await;
        assert_eq!(
            placed.len() as i64,
            test.desired,
            "case {case:?}: expected {} placed tasks, got {placed:?}",
            test.desired
        );
        if let Some(arn) = placed.first() {
            let described = ok(
                &router,
                "DescribeTasks",
                json!({ "cluster": test.cluster, "tasks": [arn] }),
                case,
            )
            .await;
            let task = &list(&described, "tasks", case)[0];
            assert_eq!(
                string(task, "group", case),
                format!("service:{}", test.service_name),
                "case {case:?}: a service's task belongs to the service group, got {task}"
            );
            assert_eq!(
                string(task, "startedBy", case),
                deployment_id,
                "case {case:?}: a service's task is started by its deployment, got {task}"
            );
            assert_eq!(
                string(task, "taskDefinitionArn", case),
                definition_arn,
                "case {case:?}: unexpected task definition on a placed task, got {task}"
            );
        }

        // Read the service back through DescribeServices: the stored record, not the
        // request. `include` gates the tags.
        let read = describe(&router, test.cluster, test.service_name, true, case).await;
        assert_eq!(
            string(&read, "serviceArn", case),
            string(created, "serviceArn", case),
            "case {case:?}: describe returned a different ARN than create: {read}"
        );
        assert_eq!(
            count(&read, "desiredCount", case),
            test.desired,
            "case {case:?}: describe must report the stored desiredCount, got {read}"
        );
        assert_eq!(
            list(&read, "tags", case),
            vec![json!({ "key": "env", "value": "test" })],
            "case {case:?}: expected the stored tags back, got {read}"
        );
        let without_tags = describe(&router, test.cluster, test.service_name, false, case).await;
        assert!(
            without_tags["tags"].is_null(),
            "case {case:?}: tags are reported only when include names TAGS, got {without_tags}"
        );

        settling.push((case, test.cluster, test.service_name, test.desired));
    }

    // The clock, not a reader, is what starts the tasks the scheduler placed and confirms
    // the rollout once they have STAYED up, so each row is polled for its own condition
    // through the shared wait rather than read after a span.
    for (case, cluster, service_name, desired) in settling {
        let settled = crate::ecs_wire::poll_until(
            case,
            "every placed task RUNNING and the rollout COMPLETED",
            || describe(&router, cluster, service_name, false, case),
            |view| {
                count(view, "runningCount", case) == desired
                    && count(view, "pendingCount", case) == 0
                    && string(&list(view, "deployments", case)[0], "rolloutState", case)
                        == "COMPLETED"
            },
        )
        .await;
        assert_eq!(
            count(&settled, "runningCount", case),
            desired,
            "case {case:?}: expected every placed task RUNNING, got {settled}"
        );
        assert_eq!(
            count(&settled, "pendingCount", case),
            0,
            "case {case:?}: expected no PENDING task, got {settled}"
        );
        assert_eq!(
            string(&list(&settled, "deployments", case)[0], "rolloutState", case),
            "COMPLETED",
            "case {case:?}: a service at its desired count has COMPLETED its rollout, \
             got {settled}"
        );
    }
}

#[tokio::test]
async fn update_service_moves_the_task_count_in_both_directions() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "scaling" }),
        "setup",
    )
    .await;
    register_capacity(&router, "scaling", "setup").await;
    let (_, _) = register(&router, "scalingfamily", "setup").await;
    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": "scaling",
            "serviceName": "web",
            "taskDefinition": "scalingfamily",
            "desiredCount": 2,
            "launchType": "EC2",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        "setup",
    )
    .await;
    let first_deployment = string(&list(&created["service"], "deployments", "setup")[0], "id", "setup");
    tokio::time::sleep(ONE_SPAN).await;

    // The rule generalises over the target count and the direction of the move, so the
    // rows span scaling out, scaling in, down to zero and back out again.
    let cases: [(&str, i64); 5] = [
        ("scale out", 5),
        ("scale in", 3),
        ("scale in to zero", 0),
        ("scale out from zero", 4),
        ("set the count it already has", 4),
    ];

    for (case, target) in cases {
        let updated = ok(
            &router,
            "UpdateService",
            json!({ "cluster": "scaling", "service": "web", "desiredCount": target }),
            case,
        )
        .await;
        let updated = &updated["service"];
        assert_eq!(
            count(updated, "desiredCount", case),
            target,
            "case {case:?}: expected desiredCount {target}, got {updated}"
        );
        // "This parameter doesn't trigger a new service deployment": moving the count
        // alone keeps the deployment the service already had.
        assert_eq!(
            string(&list(updated, "deployments", case)[0], "id", case),
            first_deployment,
            "case {case:?}: desiredCount alone must not start a new deployment, got {updated}"
        );

        // The scheduler brought the task set in line: exactly `target` tasks are wanted.
        let placed = service_task_arns(&router, "scaling", "web", case).await;
        assert_eq!(
            placed.len() as i64,
            target,
            "case {case:?}: expected {target} wanted tasks, got {placed:?}"
        );

        tokio::time::sleep(TASK_START).await;
        let settled = describe(&router, "scaling", "web", false, case).await;
        assert_eq!(
            count(&settled, "runningCount", case),
            target,
            "case {case:?}: expected runningCount {target} once the clock settles, got {settled}"
        );
        assert_eq!(
            count(&settled, "pendingCount", case),
            0,
            "case {case:?}: expected no PENDING task once the clock settles, got {settled}"
        );
    }

    // A full service ARN names the cluster that hosts it, so it addresses the same
    // service with no `cluster` member at all — and naming a different cluster beside it
    // names no service.
    let case = "addressed by its service ARN";
    let service_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/scaling/web");
    let updated = ok(
        &router,
        "UpdateService",
        json!({ "service": service_arn, "desiredCount": 1 }),
        case,
    )
    .await;
    assert_eq!(
        count(&updated["service"], "desiredCount", case),
        1,
        "case {case:?}: an ARN-addressed update must reach the same service, got {updated}"
    );
    let placed = service_task_arns(&router, "scaling", "web", case).await;
    assert_eq!(
        placed.len(),
        1,
        "case {case:?}: expected one wanted task, got {placed:?}"
    );
}

#[tokio::test]
async fn a_new_deployment_replaces_the_task_set_it_inherited() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "deploying" }),
        "setup",
    )
    .await;
    register_capacity(&router, "deploying", "setup").await;
    let (first_arn, _) = register(&router, "deployingfamily", "setup").await;
    let (second_arn, second_revision) = register(&router, "deployingfamily", "setup").await;
    assert_ne!(
        first_arn, second_arn,
        "setup: a second RegisterTaskDefinition must mint a new revision"
    );

    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": "deploying",
            "serviceName": "web",
            "taskDefinition": format!("deployingfamily:{}", second_revision - 1),
            "desiredCount": 2,
            "launchType": "EC2",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        "setup",
    )
    .await;
    let mut deployment = string(&list(&created["service"], "deployments", "setup")[0], "id", "setup");
    let mut tasks = service_task_arns(&router, "deploying", "web", "setup").await;

    // Both of the things that start a deployment, and the member that does not.
    let cases: [(&str, Value, bool); 3] = [
        (
            "a changed task definition starts a deployment",
            json!({ "taskDefinition": second_arn.clone() }),
            true,
        ),
        (
            "forceNewDeployment starts one with no definition change",
            json!({ "forceNewDeployment": true }),
            true,
        ),
        (
            "a deployment configuration change does not",
            json!({ "deploymentConfiguration": { "maximumPercent": 150 } }),
            false,
        ),
    ];

    for (case, patch, expect_new) in cases {
        let mut request = json!({ "cluster": "deploying", "service": "web" });
        for (member, value) in patch.as_object().expect("the patch is an object") {
            request[member] = value.clone();
        }
        let updated = ok(&router, "UpdateService", request, case).await;
        let updated = &updated["service"];
        let now_deployment = string(&list(updated, "deployments", case)[0], "id", case);
        if expect_new {
            assert_ne!(
                now_deployment, deployment,
                "case {case:?}: expected a new deployment id, got {updated}"
            );
        } else {
            assert_eq!(
                now_deployment, deployment,
                "case {case:?}: expected the deployment to be kept, got {updated}"
            );
        }

        if expect_new {
            // A rolling update REPLACES the task set it inherited: the replacements are
            // started first and the tasks they replace are retired once they are up, so
            // the set the rollout leaves behind is polled for rather than read in the
            // instant the update answered.
            let now_tasks = poll_replaced_task_set(
                &router,
                "deploying",
                "web",
                case,
                &tasks,
                2,
            )
            .await;
            assert!(
                now_tasks.iter().all(|arn| !tasks.contains(arn)),
                "case {case:?}: a new deployment replaces the task set it inherited; \
                 before {tasks:?}, after {now_tasks:?}"
            );
            tasks = now_tasks;
        } else {
            let now_tasks = service_task_arns(&router, "deploying", "web", case).await;
            assert_eq!(
                now_tasks.len(),
                2,
                "case {case:?}: the service still wants two tasks, got {now_tasks:?}"
            );
            assert_eq!(
                now_tasks, tasks,
                "case {case:?}: an update that starts no deployment keeps the task set"
            );
            tasks = now_tasks;
        }

        deployment = now_deployment;
    }

    // The service now runs the second revision, and says so.
    let read = describe(&router, "deploying", "web", false, "after the deployments").await;
    assert_eq!(
        string(&read, "taskDefinition", "after the deployments"),
        second_arn,
        "the stored service must report the revision it was updated to, got {read}"
    );
    assert_eq!(
        count(&read["deploymentConfiguration"], "maximumPercent", "after the deployments"),
        150,
        "an updated deployment configuration must be stored, got {read}"
    );
}

#[tokio::test]
async fn the_cluster_counts_the_services_and_tasks_it_holds() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "counted" }),
        "setup",
    )
    .await;
    register_capacity(&router, "counted", "setup").await;
    register(&router, "countedfamily", "setup").await;

    // Each row adds one service, and the cluster's counters must follow.
    let cases: [(&str, &str, i64, &str); 3] = [
        ("first EC2 service", "alpha", 1, "EC2"),
        ("second EC2 service", "beta", 2, "EC2"),
        ("a Fargate service", "gamma", 3, "FARGATE"),
    ];

    let mut expected_services = 0;
    let mut expected_tasks = 0;
    let mut expected_ec2 = 0;
    let mut expected_fargate = 0;
    for (case, name, desired, launch_type) in cases {
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": "counted",
                "serviceName": name,
                "taskDefinition": "countedfamily",
                "desiredCount": desired,
                "launchType": launch_type,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            case,
        )
        .await;
        expected_services += 1;
        expected_tasks += desired;
        if launch_type == "EC2" {
            expected_ec2 += 1;
        } else {
            expected_fargate += 1;
        }

        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": ["counted"], "include": ["STATISTICS"] }),
            case,
        )
        .await;
        let cluster = &list(&described, "clusters", case)[0];
        assert_eq!(
            count(cluster, "activeServicesCount", case),
            expected_services,
            "case {case:?}: expected activeServicesCount {expected_services}, got {cluster}"
        );
        // The tasks have just been placed, so the cluster counts them as PENDING.
        assert_eq!(
            count(cluster, "pendingTasksCount", case),
            expected_tasks,
            "case {case:?}: expected pendingTasksCount {expected_tasks}, got {cluster}"
        );
        let statistics = list(cluster, "statistics", case);
        let statistic = |name: &str| -> String {
            statistics
                .iter()
                .find(|pair| pair["name"].as_str() == Some(name))
                .and_then(|pair| pair["value"].as_str())
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(
            statistic("activeEC2ServiceCount"),
            expected_ec2.to_string(),
            "case {case:?}: unexpected activeEC2ServiceCount in {cluster}"
        );
        assert_eq!(
            statistic("activeFargateServiceCount"),
            expected_fargate.to_string(),
            "case {case:?}: unexpected activeFargateServiceCount in {cluster}"
        );
    }

    // One wait covers every row: the clock starts the tasks of all three services, and
    // the cluster's running count is the sum of what their schedulers placed.
    let case = "every service settled";
    tokio::time::sleep(TASK_START).await;
    let described = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": ["counted"] }),
        case,
    )
    .await;
    let cluster = &list(&described, "clusters", case)[0];
    assert_eq!(
        count(cluster, "runningTasksCount", case),
        expected_tasks,
        "case {case:?}: expected runningTasksCount {expected_tasks}, got {cluster}"
    );
    assert_eq!(
        count(cluster, "pendingTasksCount", case),
        0,
        "case {case:?}: expected no PENDING task once the clock settles, got {cluster}"
    );

    // "You can't delete a cluster that contains services."
    refused(
        &router,
        "DeleteCluster",
        json!({ "cluster": "counted" }),
        "ClusterContainsServicesException",
        "a cluster that still holds services",
    )
    .await;
}

#[tokio::test]
async fn a_service_request_that_cannot_be_satisfied_is_refused() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "refusing" }),
        "setup",
    )
    .await;
    register(&router, "refusingfamily", "setup").await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": "refusing",
            "serviceName": "web",
            "taskDefinition": "refusingfamily",
            "desiredCount": 1,
            "launchType": "EC2",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        "setup",
    )
    .await;

    // One row per rule the handler enforces, each with the modelled error it answers.
    let cases: [(&str, &str, Value, &str); 11] = [
        (
            "a service name outside the documented character set",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "not a service name",
                "taskDefinition": "refusingfamily",
                "desiredCount": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            "InvalidParameterException",
        ),
        (
            "a service name longer than 255 characters",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "s".repeat(256),
                "taskDefinition": "refusingfamily",
                "desiredCount": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            "InvalidParameterException",
        ),
        (
            "a cluster that does not exist",
            "CreateService",
            json!({
                "cluster": "nosuchcluster",
                "serviceName": "web",
                "taskDefinition": "refusingfamily",
                "desiredCount": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            "ClusterNotFoundException",
        ),
        (
            "a task definition that was never registered",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "other",
                "taskDefinition": "nosuchfamily",
                "desiredCount": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            "ClientException",
        ),
        (
            "no desiredCount for a REPLICA service",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "other",
                "taskDefinition": "refusingfamily",
            }),
            "InvalidParameterException",
        ),
        (
            "a negative desiredCount",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "other",
                "taskDefinition": "refusingfamily",
                "desiredCount": -1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            "InvalidParameterException",
        ),
        (
            "both a launch type and a capacity provider strategy",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "other",
                "taskDefinition": "refusingfamily",
                "desiredCount": 1,
                "launchType": "EC2",
                "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }],
            }),
            "InvalidParameterException",
        ),
        (
            "a name another service in the cluster already has",
            "CreateService",
            json!({
                "cluster": "refusing",
                "serviceName": "web",
                "taskDefinition": "refusingfamily",
                "desiredCount": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            "InvalidParameterException",
        ),
        (
            "updating a service that does not exist",
            "UpdateService",
            json!({ "cluster": "refusing", "service": "nosuchservice", "desiredCount": 2 }),
            "ServiceNotFoundException",
        ),
        (
            "a service ARN whose cluster is not the one the request names",
            "UpdateService",
            json!({
                "cluster": "refusing",
                "service": format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/othercluster/web"
                ),
                "desiredCount": 2,
            }),
            "InvalidParameterException",
        ),
        (
            "describing services in a cluster that does not exist",
            "DescribeServices",
            json!({ "cluster": "nosuchcluster", "services": ["web"] }),
            "ClusterNotFoundException",
        ),
    ];

    for (case, operation, request, expected) in cases {
        refused(&router, operation, request, expected, case).await;
    }

    // A name that matches nothing is a FAILURE entry, not a refusal: "MISSING — the
    // described resource could not be found".
    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": "refusing", "services": ["web", "nosuchservice"] }),
        "a mix of known and unknown names",
    )
    .await;
    let case = "a mix of known and unknown names";
    assert_eq!(
        list(&described, "services", case).len(),
        1,
        "case {case:?}: only the stored service is described, got {described}"
    );
    let failures = list(&described, "failures", case);
    assert_eq!(
        failures,
        vec![json!({
            "arn": format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/refusing/nosuchservice"
            ),
            "reason": "MISSING",
        })],
        "case {case:?}: unexpected failures in {described}"
    );

    // "You may specify up to 10 services to describe in a single operation."
    let eleven: Vec<String> = (0..11).map(|index| format!("service{index}")).collect();
    refused(
        &router,
        "DescribeServices",
        json!({ "cluster": "refusing", "services": eleven }),
        "InvalidParameterException",
        "more than ten services in one describe",
    )
    .await;
}

#[tokio::test]
async fn repeating_a_create_with_the_same_client_token_is_the_same_service() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "idempotent" }),
        "setup",
    )
    .await;
    register_capacity(&router, "idempotent", "setup").await;
    register(&router, "idempotentfamily", "setup").await;

    let request = json!({
        "cluster": "idempotent",
        "serviceName": "web",
        "taskDefinition": "idempotentfamily",
        "desiredCount": 2,
        "launchType": "EC2",
        "networkConfiguration": awsvpc_network_configuration(),
        "clientToken": "token-0001",
    });
    let case = "the same clientToken twice";
    let first = ok(&router, "CreateService", request.clone(), case).await;
    let second = ok(&router, "CreateService", request, case).await;
    assert_eq!(
        string(&first["service"], "serviceArn", case),
        string(&second["service"], "serviceArn", case),
        "case {case:?}: the repeat must report the same service"
    );
    assert_eq!(
        string(&list(&first["service"], "deployments", case)[0], "id", case),
        string(&list(&second["service"], "deployments", case)[0], "id", case),
        "case {case:?}: the repeat must not start a second deployment"
    );
    let placed = service_task_arns(&router, "idempotent", "web", case).await;
    assert_eq!(
        placed.len(),
        2,
        "case {case:?}: the repeat must not place a second task set, got {placed:?}"
    );

    // A clientToken outside "up to 36 ASCII characters in the range of 33-126" is
    // refused, and so is a DIFFERENT token on a name that is already taken.
    refused(
        &router,
        "CreateService",
        json!({
            "cluster": "idempotent",
            "serviceName": "other",
            "taskDefinition": "idempotentfamily",
            "desiredCount": 1,
            "networkConfiguration": awsvpc_network_configuration(),
            "clientToken": "t".repeat(37),
        }),
        "InvalidParameterException",
        "a clientToken longer than 36 characters",
    )
    .await;
    refused(
        &router,
        "CreateService",
        json!({
            "cluster": "idempotent",
            "serviceName": "web",
            "taskDefinition": "idempotentfamily",
            "desiredCount": 2,
            "networkConfiguration": awsvpc_network_configuration(),
            "clientToken": "token-0002",
        }),
        "InvalidParameterException",
        "a different clientToken on a name that is taken",
    )
    .await;
}
