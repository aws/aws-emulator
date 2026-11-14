// SPDX-License-Identifier: CC0-1.0

//! The image PULL a launched task's containers need, served by Amazon ECR, and the
//! terminal status a task that ran reaches — driven end to end over the wire, with Amazon
//! ECR and IAM mounted beside Amazon ECS on one engine so every repository, image, pull
//! through cache rule, role and policy is created through the OWNING service's own API
//! rather than written into a store by hand.
//!
//! The rules under test:
//!
//! * a container's image is fetched before the container starts, and for a reference that
//!   names a repository of this account's Amazon ECR registry the fetch is the registry's
//!   to serve: Amazon ECS pulls as the task's EXECUTION ROLE
//!   (`TaskDefinition$executionRoleArn`), so the registry decides the two calls a pull
//!   makes (`ecr:GetAuthorizationToken` on the registry and `ecr:BatchGetImage` on the
//!   repository) over the policies in force on that role and the repository's own policy,
//!   resolves the reference, and caches the upstream image where a pull through cache rule
//!   covers the repository — and a task whose image it will not serve never runs: it stops
//!   with `Task$stopCode` `TaskFailedToStart` and a `CannotPullContainerError`
//!   `stoppedReason`, with no `startedAt`;
//! * a task that DID pull records the instants the fetch took (`Task$pullStartedAt` and
//!   `pullStoppedAt`) and runs, and a task placed on its own then reaches the terminal
//!   `STOPPED` status with the stop code for an essential container that exited — while a
//!   task a service's scheduler placed is kept running by that scheduler.
//!
//! The rows differ along what the rule generalises over: whether the registry holds the
//! image, whether a cache rule covers the repository, which half of the pull the role is
//! granted, which document grants it, and whether the reference names this registry at
//! all. Nothing here reaches into either state struct: every value asserted is read back
//! through `DescribeTasks` and Amazon ECR's `DescribeImages`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    ecr, emulator_with_ecr_and_iam, iam, ok, register_instance, string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster every task below runs in.
const CLUSTER: &str = "registrypull";

/// The EC2 instance id the cluster's one container instance is registered under: a task
/// placed with the default `EC2` launch type runs on the capacity registered to its
/// cluster, so every cluster below has one.
const CAPACITY_INSTANCE_ID: &str = "i-0c0ffee0c0ffee0c1";

/// How often a poll re-reads a task.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll keeps trying before it gives up: the whole run of a standalone task —
/// its start, the work it was placed to do (`state::CONTAINER_WORK_SPAN`) and the stop that
/// follows — with margin for a loaded test machine. Built by `Duration` arithmetic, so no
/// figure here is a cast.
const POLL_BUDGET: Duration =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::CONTAINER_WORK_SPAN
        .saturating_add(
            service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
                .saturating_mul(4),
        )
        .saturating_add(Duration::from_secs(4));

/// `Task$lastStatus` of a task that has stopped.
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of
/// (`smithy_query(query="shape",shape="TaskStopCode")`).
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// `Task$stopCode` of a task whose essential container exited — the end of a task that
/// ran.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// The error class a container runtime reports a failed image fetch under, which is the
/// fixed part of such a `stoppedReason`.
const CANNOT_PULL: &str = "CannotPullContainerError";

/// A Docker image manifest V2 schema 2 referencing no blob: what is under test is who may
/// pull an image and whether the registry holds one, not which blobs an image is built
/// from.
const MANIFEST: &str = concat!(
    r#"{"schemaVersion":2,"#,
    r#""mediaType":"application/vnd.docker.distribution.manifest.v2+json"}"#
);

/// Create the IAM role `name`, trusted by Amazon ECS, carrying `policy` as its one inline
/// policy when a document is given — through IAM's own API — and answer the role's ARN.
async fn role(router: &MultiServiceRouter, name: &str, policy: Option<&str>) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "ecs-tasks.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }]
    })
    .to_string();
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
        name,
    )
    .await;
    if let Some(policy) = policy {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
            name,
        )
        .await;
    }
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// Create the cluster the tasks below run in, with ONE container instance registered into
/// it: "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
/// cluster", so a cluster with none has nowhere to place a task.
async fn cluster(router: &MultiServiceRouter) {
    ok(router, "CreateCluster", json!({ "clusterName": CLUSTER }), "cluster").await;
    register_instance(router, CLUSTER, CAPACITY_INSTANCE_ID, "capacity").await;
}

/// Register one revision of `family` running `image` as `execution_role`, and run one task
/// of it — answering the task's ARN.
async fn run(
    router: &MultiServiceRouter,
    family: &str,
    image: &str,
    execution_role: Option<&str>,
    case: &str,
) -> String {
    let mut definition = json!({
        "family": family,
        "containerDefinitions": [container("app", image)],
    });
    if let Some(role) = execution_role {
        definition["executionRoleArn"] = json!(role);
    }
    ok(router, "RegisterTaskDefinition", definition, case).await;
    let body = ok(
        router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": family }),
        case,
    )
    .await;
    let tasks = body["tasks"].as_array().cloned().unwrap_or_default();
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
    body["tasks"]
        .as_array()
        .and_then(|tasks| tasks.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// Poll one task until it reaches the terminal `STOPPED` status, and answer the record it
/// stopped as.
async fn stopped(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let task = describe(router, task_arn, case).await;
        if string(&task, "lastStatus", case) == STOPPED {
            return task;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to reach {STOPPED}, it was still {:?}: {task}",
            string(&task, "lastStatus", case)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// An image reference naming the repository `path` of this account's registry in the Region
/// the requests are signed for: `<account>.dkr.ecr.<region>.amazonaws.com/<path>` (Amazon
/// ECR User Guide, the registry URI a pull uses).
fn ecr_image(path: &str) -> String {
    format!("{ACCOUNT}.dkr.ecr.{SIGNED_REGION}.amazonaws.com/{path}")
}

/// An identity-based document allowing `actions` on every resource.
fn grant(actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }],
    })
    .to_string()
}

/// What a row expects of the task it launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The registry served the image, so the task ran and then ended.
    Ran,
    /// The registry would not serve it, so the task never started.
    FailedToStart,
}

/// One row of the table: a revision running `image` as `execution_role`, and the terminal
/// record the task it places must reach.
struct Row {
    case: &'static str,
    family: &'static str,
    image: String,
    execution_role: Option<String>,
    expected: Outcome,
    /// A clause the task's `stoppedReason` must carry, for a row whose refusal says WHY the
    /// registry would not serve the pull.
    says: Option<String>,
}

/// One task launched by the table, waiting to be read back.
struct Launched {
    case: &'static str,
    image: String,
    task_arn: String,
    expected: Outcome,
    says: Option<String>,
}

#[tokio::test]
async fn a_task_pulls_its_ecr_image_as_its_execution_role_and_then_settles() {
    let router = emulator_with_ecr_and_iam();
    cluster(&router).await;
    let case = "the registry's records";

    // The registry's own records: one repository holding one tagged image, and one pull
    // through cache rule whose prefix nothing has been cached under yet.
    ecr(
        &router,
        "CreateRepository",
        json!({ "repositoryName": "app" }),
        case,
    )
    .await;
    ecr(
        &router,
        "PutImage",
        json!({
            "repositoryName": "app",
            "imageManifest": MANIFEST,
            "imageTag": "1",
        }),
        case,
    )
    .await;
    ecr(
        &router,
        "CreatePullThroughCacheRule",
        json!({
            "ecrRepositoryPrefix": "hub",
            "upstreamRegistryUrl": "registry-1.docker.io",
        }),
        case,
    )
    .await;
    // A repository with a policy that shares its images with ONE role, which is the other
    // document a pull can be allowed by.
    ecr(
        &router,
        "CreateRepository",
        json!({ "repositoryName": "shared" }),
        case,
    )
    .await;
    ecr(
        &router,
        "PutImage",
        json!({
            "repositoryName": "shared",
            "imageManifest": MANIFEST,
            "imageTag": "1",
        }),
        case,
    )
    .await;

    // The roles, each carrying a different part of what a pull takes.
    let full = role(
        &router,
        "pullall",
        Some(&grant(&["ecr:GetAuthorizationToken", "ecr:BatchGetImage"])),
    )
    .await;
    let none = role(&router, "pullnone", Some(&grant(&["ecr:ListImages"]))).await;
    let token_only = role(
        &router,
        "pulltoken",
        Some(&grant(&["ecr:GetAuthorizationToken"])),
    )
    .await;
    let unknown = format!("arn:aws:iam::{ACCOUNT}:role/nosuchrole");
    ecr(
        &router,
        "SetRepositoryPolicy",
        json!({
            "repositoryName": "shared",
            "policyText": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": token_only },
                    "Action": "ecr:BatchGetImage",
                    // A repository policy is attached to the repository its statements
                    // apply to, so it names no resource of its own — Amazon ECR refuses a
                    // repository policy that does.
                }],
            }).to_string(),
        }),
        case,
    )
    .await;
    // A third repository, holding the same image, whose policy DENIES the pull outright: an
    // explicit deny on either side refuses whatever the identity documents grant.
    ecr(
        &router,
        "CreateRepository",
        json!({ "repositoryName": "denied" }),
        case,
    )
    .await;
    ecr(
        &router,
        "PutImage",
        json!({
            "repositoryName": "denied",
            "imageManifest": MANIFEST,
            "imageTag": "1",
        }),
        case,
    )
    .await;
    ecr(
        &router,
        "SetRepositoryPolicy",
        json!({
            "repositoryName": "denied",
            "policyText": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "ecr:BatchGetImage",
                }],
            }).to_string(),
        }),
        case,
    )
    .await;

    let row = |case: &'static str,
               family: &'static str,
               image: String,
               execution_role: Option<&String>,
               expected: Outcome| Row {
        case,
        family,
        image,
        execution_role: execution_role.cloned(),
        expected,
        says: None,
    };
    /// The clause a refusal carries when nothing grants a call: IAM's default deny, naming
    /// the action (`service_common::iam_authorization::no_identity_policy_allows`).
    fn nothing_allows(action: &str) -> String {
        format!("because no identity-based policy allows the {action} action")
    }
    /// The clause a refusal carries when the REPOSITORY's own policy denies the call.
    const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";
    let rows = vec![
        row(
            "an image the registry holds, pulled by a granted role",
            "held",
            ecr_image("app:1"),
            Some(&full),
            Outcome::Ran,
        ),
        row(
            "a repository a pull through cache rule covers, cached on this pull",
            "cached",
            ecr_image("hub/library/busybox:latest"),
            Some(&full),
            Outcome::Ran,
        ),
        row(
            "a repository the registry does not hold and no rule covers",
            "missingrepo",
            ecr_image("absent:1"),
            Some(&full),
            Outcome::FailedToStart,
        ),
        row(
            "a tag the repository does not hold",
            "missingtag",
            ecr_image("app:2"),
            Some(&full),
            Outcome::FailedToStart,
        ),
        Row {
            says: Some(nothing_allows("ecr:GetAuthorizationToken")),
            ..row(
                "a role no document grants an ECR pull to",
                "nogrant",
                ecr_image("app:1"),
                Some(&none),
                Outcome::FailedToStart,
            )
        },
        Row {
            says: Some(nothing_allows("ecr:BatchGetImage")),
            ..row(
                "a role granted the registry token but not the manifest",
                "tokenonly",
                ecr_image("app:1"),
                Some(&token_only),
                Outcome::FailedToStart,
            )
        },
        row(
            "a role the repository's own policy shares the image with",
            "sharedrepo",
            ecr_image("shared:1"),
            Some(&token_only),
            Outcome::Ran,
        ),
        Row {
            says: Some(RESOURCE_DENY.to_string()),
            ..row(
                "a repository whose own policy denies the pull a role is granted",
                "deniedrepo",
                ecr_image("denied:1"),
                Some(&full),
                Outcome::FailedToStart,
            )
        },
        row(
            "a role this account's IAM holds no record for",
            "unknownrole",
            ecr_image("app:1"),
            Some(&unknown),
            Outcome::Ran,
        ),
        row(
            "no execution role at all",
            "norole",
            ecr_image("app:1"),
            None,
            Outcome::Ran,
        ),
        row(
            "a reference naming another registry altogether",
            "publicimage",
            "public.ecr.aws/nginx/nginx:latest".to_string(),
            Some(&none),
            Outcome::Ran,
        ),
    ];

    // Every task is launched first and polled afterwards: each settles on its own clock,
    // so the whole table costs one run rather than one per row.
    let mut launched = Vec::new();
    for row in &rows {
        let task_arn = run(
            &router,
            row.family,
            &row.image,
            row.execution_role.as_deref(),
            row.case,
        )
        .await;
        launched.push(Launched {
            case: row.case,
            image: row.image.clone(),
            task_arn,
            expected: row.expected,
            says: row.says.clone(),
        });
    }

    for Launched {
        case,
        image,
        task_arn,
        expected,
        says,
    } in launched
    {
        let task = stopped(&router, &task_arn, case).await;
        let stop_code = string(&task, "stopCode", case);
        // The pull is ATTEMPTED either way, so both instants are reported either way.
        for member in ["pullStartedAt", "pullStoppedAt"] {
            assert!(
                !task[member].is_null(),
                "case {case:?}: expected the attempted pull's {member}, got {task}"
            );
        }
        match expected {
            Outcome::Ran => {
                assert_eq!(
                    stop_code, ESSENTIAL_CONTAINER_EXITED,
                    "case {case:?}: expected a task that pulled {image} to end with \
                     {ESSENTIAL_CONTAINER_EXITED}, got {task}"
                );
                assert!(
                    !task["startedAt"].is_null(),
                    "case {case:?}: expected a startedAt on a task that ran, got {task}"
                );
            }
            Outcome::FailedToStart => {
                assert_eq!(
                    stop_code, TASK_FAILED_TO_START,
                    "case {case:?}: expected a task that could not pull {image} to stop with \
                     {TASK_FAILED_TO_START}, got {task}"
                );
                let reason = string(&task, "stoppedReason", case);
                assert!(
                    reason.contains(CANNOT_PULL),
                    "case {case:?}: expected the stoppedReason to report {CANNOT_PULL}, got \
                     {reason:?}"
                );
                assert!(
                    reason.contains(&image),
                    "case {case:?}: expected the stoppedReason to name the image {image}, got \
                     {reason:?}"
                );
                // A row that is about WHY the registry withheld the pull asserts that
                // clause too: an explicit deny in the repository's own policy is not the
                // same answer as a grant nothing wrote.
                if let Some(says) = says.as_deref() {
                    assert!(
                        reason.contains(says),
                        "case {case:?}: expected the stoppedReason to carry {says:?}, got \
                         {reason:?}"
                    );
                }
                assert!(
                    task["startedAt"].is_null(),
                    "case {case:?}: expected no startedAt on a task that never ran, got {task}"
                );
            }
        }
        // Whichever way the task ended, no container reports an `exitCode`: this emulator
        // starts no container process, so nothing observed an exit status — only the
        // agent's own report carries one.
        for container in task["containers"].as_array().cloned().unwrap_or_default() {
            assert!(
                container["exitCode"].is_null(),
                "case {case:?}: expected no exitCode on a container nothing ran, got \
                 {container}"
            );
        }
    }

    // The import the cached row made, read back through the REGISTRY's own report of what
    // the cache repository holds: the upstream image is in it under the tag that was
    // pulled, with a digest of the manifest the registry stored.
    let case = "the cached image";
    let described = ecr(
        &router,
        "DescribeImages",
        json!({ "repositoryName": "hub/library/busybox" }),
        case,
    )
    .await;
    let details = described["imageDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let detail = details
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the cached image, got {described}"));
    assert!(
        detail["imageTags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|tag| tag.as_str() == Some("latest"))),
        "case {case:?}: expected the pulled tag on the cached image, got {detail}"
    );
    assert!(
        string(detail, "imageDigest", case).starts_with("sha256:"),
        "case {case:?}: expected a content digest on the cached image, got {detail}"
    );

    // And nothing was cached under a prefix no rule covers: the repository the missing-row
    // reference named is still absent from the registry.
    let listed = ecr(&router, "DescribeRepositories", json!({}), case).await;
    let names: Vec<String> = listed["repositories"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|repository| string(repository, "repositoryName", case))
        .collect();
    assert!(
        !names.iter().any(|name| name == "absent"),
        "case \"a repository no rule covers\": expected no repository to be created for it, \
         got {names:?}"
    );
}

#[tokio::test]
async fn a_service_keeps_a_running_task_past_the_span_a_standalone_task_ends_in() {
    let router = emulator_with_ecr_and_iam();
    cluster(&router).await;
    let case = "a service's task";
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "servicefamily",
            "containerDefinitions": [container("app", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "web",
            "taskDefinition": "servicefamily",
            "desiredCount": 1,
        }),
        case,
    )
    .await;
    // A task placed ON ITS OWN in the same cluster, at the same instant, is the clock this
    // row is read against: once IT has stopped, the span a standalone task's work takes has
    // passed — so nothing here sleeps for a fixed stretch.
    let standalone = run(
        &router,
        "standalonefamily",
        "public.ecr.aws/nginx/nginx:latest",
        None,
        "a standalone task",
    )
    .await;
    stopped(&router, &standalone, "a standalone task").await;

    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": ["web"] }),
        case,
    )
    .await;
    let service = described["services"]
        .as_array()
        .and_then(|services| services.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {described}"));
    assert_eq!(
        service["runningCount"].as_i64().unwrap_or_default(),
        1,
        "case {case:?}: expected the scheduler to keep one task running, got {service}"
    );
    let listed = ok(
        &router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": "web" }),
        case,
    )
    .await;
    let task_arn = listed["taskArns"]
        .as_array()
        .and_then(|arns| arns.first().cloned())
        .and_then(|arn| arn.as_str().map(ToString::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: expected the service's task, got {listed}"));
    let task = describe(&router, &task_arn, case).await;
    assert_eq!(
        string(&task, "lastStatus", case),
        "RUNNING",
        "case {case:?}: expected a service's task to still be running, got {task}"
    );
    assert!(
        task["stopCode"].is_null(),
        "case {case:?}: expected no stopCode on a running task, got {task}"
    );
}
