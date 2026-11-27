// SPDX-License-Identifier: CC0-1.0

//! Whether a launched task's containers can be STARTED at all, driven end to end over the
//! wire with Amazon ECR and IAM mounted beside Amazon ECS on one engine — so every
//! repository, blob, image, policy and role is created through the OWNING service's own API.
//!
//! The rules under test:
//!
//! * a container runs an argument vector, and the vector is its entry point followed by its
//!   command: `ContainerDefinition$entryPoint` is "the entry point that's passed to the
//!   container" and `ContainerDefinition$command` "the command that's passed to the
//!   container", which "overrides the default command from the Docker image" — so a
//!   container whose definition (and whose launch's `containerOverrides`) names neither and
//!   whose IMAGE declares neither has no process for the runtime to start. Such a task never
//!   runs: it reaches the terminal `STOPPED` status with `Task$stopCode`
//!   `TaskFailedToStart` ("the stop code indicating why a task was stopped") and the
//!   container runtime's `CannotStartContainerError`, reported as the task's `stoppedReason`
//!   and as the `Container$reason` of the container that could not be created, with no
//!   `startedAt` and no `exitCode` — while the image it DID fetch is still reported, by the
//!   digest the registry resolved (`Container$imageDigest`);
//! * a container that HAS a program runs: one whose definition names a command, one whose
//!   launch overrides the command, and one whose image's own configuration declares an entry
//!   point or a command are each started, so none of those tasks is stopped as one that
//!   failed to start;
//! * and nothing is claimed about an image nothing states a program for: a reference whose
//!   stored document is not an image manifest at all declares neither a program nor the
//!   absence of one, so a container of it is left exactly as it was.
//!
//! What an image declares is the REGISTRY's answer, read off the manifest the push stored and
//! the configuration blob it references, so the rows differ along what that rule generalises
//! over: which of the two configuration members declares the program, whether the
//! configuration declares neither, whether the manifest references a configuration at all,
//! and — for the contrast rows — which of the three places a program can be named names one.
//!
//! The second suite pins the stop a task earns when the image cannot be FETCHED, over the
//! three predicates a client's negative tests for an Amazon ECR pull exercise: a repository
//! policy that DENIES the pull to the task's execution role, a tag the repository holds no
//! image under, and a reference into another account's registry. Each is the same
//! `TaskFailedToStart`, with the runtime's `CannotPullContainerError` rather than the
//! start failure above.
//!
//! Nothing here reaches into either state struct: every value asserted is read back through
//! `DescribeTasks`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    ecr, emulator_with_ecr_and_iam, iam, list, ok, poll_until, register_instance, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster every task below runs in.
const CLUSTER: &str = "startability";

/// The EC2 instance id the cluster's one container instance is registered under: "The EC2
/// launch type runs your tasks on Amazon EC2 instances registered to your cluster", so a
/// cluster with none has nowhere to place a task.
const CAPACITY_INSTANCE_ID: &str = "i-0c0ffee0c0ffee0d2";

/// `Task$lastStatus` of a task that has stopped.
const STOPPED: &str = "STOPPED";

/// `Task$lastStatus` of a task that is running.
const RUNNING: &str = "RUNNING";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated). Spelled out here rather than imported, so a wrong
/// constant in the implementation cannot make a test pass.
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// `Task$stopCode` of a task whose essential container exited — the end of a task that ran.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// The error class a container runtime reports a container it could not CREATE under, which
/// is the fixed part of such a `stoppedReason`.
const CANNOT_START: &str = "CannotStartContainerError";

/// The error class it reports a failed image FETCH under, which is the fixed part of the
/// other stop below.
const CANNOT_PULL: &str = "CannotPullContainerError";

/// A program that ends at once, for a row that names one itself.
const PROGRAM_THAT_ENDS: [&str; 3] = ["sh", "-c", "exit 0"];

/// The digest of `content` in the `sha256:<hex>` form a registry addresses a blob by,
/// computed here rather than taken from the registry's answer.
fn blob_digest(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// Put `content` into `repository` as a blob, through the documented push sequence —
/// `InitiateLayerUpload`, `UploadLayerPart`, `CompleteLayerUpload` — and answer the digest the
/// registry registered it under, which is the digest a manifest references it by.
///
/// A manifest cannot be pushed before the blobs it references are in the repository
/// (`PutImage`'s `LayersNotFoundException`), so a row that pushes an image whose
/// configuration states something pushes that configuration through here first.
async fn push_blob(
    router: &MultiServiceRouter,
    repository: &str,
    content: &[u8],
    case: &str,
) -> String {
    let opened = ecr(
        router,
        "InitiateLayerUpload",
        json!({ "repositoryName": repository }),
        case,
    )
    .await;
    let upload_id = string(&opened, "uploadId", case);
    let last = i64::try_from(content.len()).expect("a test blob fits in an i64") - 1;
    ecr(
        router,
        "UploadLayerPart",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "partFirstByte": 0,
            "partLastByte": last,
            "layerPartBlob": aws_smithy_types::base64::encode(content),
        }),
        case,
    )
    .await;
    let completed = ecr(
        router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [blob_digest(content)],
        }),
        case,
    )
    .await;
    string(&completed, "layerDigest", case)
}

/// Create `repository` and push ONE image into it under the tag `1`: the image's
/// CONFIGURATION is `configuration` when a row states one — pushed as the blob its manifest
/// references, so the registry really holds what the image declares — and the manifest
/// references none at all when it does not.
///
/// Answers the image reference a container definition names it by: the registry URI of this
/// account's registry in the Region the requests are signed for (Amazon ECR User Guide, the
/// registry URI a pull uses).
async fn push_image(
    router: &MultiServiceRouter,
    repository: &str,
    configuration: Option<&str>,
    case: &str,
) -> String {
    ecr(
        router,
        "CreateRepository",
        json!({ "repositoryName": repository }),
        case,
    )
    .await;
    let mut manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "layers": [],
    });
    if let Some(configuration) = configuration {
        let digest = push_blob(router, repository, configuration.as_bytes(), case).await;
        manifest["config"] = json!({
            "mediaType": "application/vnd.docker.container.image.v1+json",
            "digest": digest,
            "size": configuration.len(),
        });
    }
    ecr(
        router,
        "PutImage",
        json!({
            "repositoryName": repository,
            "imageManifest": manifest.to_string(),
            "imageTag": IMAGE_TAG,
        }),
        case,
    )
    .await;
    ecr_image(&format!("{repository}:{IMAGE_TAG}"))
}

/// The tag every image below is pushed under.
const IMAGE_TAG: &str = "1";

/// An image reference naming the path `path` of this account's registry in the Region the
/// requests are signed for: `<account>.dkr.ecr.<region>.amazonaws.com/<path>`.
fn ecr_image(path: &str) -> String {
    format!("{ACCOUNT}.dkr.ecr.{SIGNED_REGION}.amazonaws.com/{path}")
}

/// Create the execution role Amazon ECS pulls as, through IAM's own API, carrying an inline
/// document that allows the two calls a pull of a private image makes — and answer its ARN.
async fn execution_role(router: &MultiServiceRouter, name: &str) -> String {
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
    let grant = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [
                "ecr:GetAuthorizationToken",
                "ecr:BatchGetImage",
                "ecr:GetDownloadUrlForLayer",
            ],
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name.to_string()),
            ("PolicyName", "pull".to_string()),
            ("PolicyDocument", grant),
        ],
        name,
    )
    .await;
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// Create the cluster the tasks below run in, with ONE container instance registered into it.
async fn cluster(router: &MultiServiceRouter, case: &str) {
    ok(router, "CreateCluster", json!({ "clusterName": CLUSTER }), case).await;
    register_instance(router, CLUSTER, CAPACITY_INSTANCE_ID, case).await;
}

/// Register one revision of `family` running `image` as `execution_role` — with `command` as
/// its container's own command where a row names one — and run one task of it, overriding the
/// container's command with `override_command` where a row states that instead. Answers the
/// task's ARN.
async fn run(
    router: &MultiServiceRouter,
    family: &str,
    image: &str,
    execution_role: &str,
    command: Option<&[&str]>,
    override_command: Option<&[&str]>,
    case: &str,
) -> String {
    let mut app = container("app", image);
    if let Some(command) = command {
        app["command"] = json!(command);
    }
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [app],
            "executionRoleArn": execution_role,
        }),
        case,
    )
    .await;
    let mut request = json!({ "cluster": CLUSTER, "taskDefinition": family });
    if let Some(command) = override_command {
        request["overrides"] = json!({
            "containerOverrides": [{ "name": "app", "command": command }],
        });
    }
    let body = ok(router, "RunTask", request, case).await;
    let placed = list(&body, "tasks", case);
    let task = placed
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected RunTask to place one task, got {body}"));
    string(task, "taskArn", case)
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the call
/// that launched it.
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

/// Poll one task until it has SETTLED at a status its launch decided — `RUNNING` for a task
/// that was started, `STOPPED` for one that was not — and answer the record it settled as.
async fn settled(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    poll_until(
        case,
        &format!("the task to settle at {RUNNING} or {STOPPED}"),
        || describe(router, task_arn, case),
        |task| {
            let status = task["lastStatus"].as_str().unwrap_or_default();
            status == RUNNING || status == STOPPED
        },
    )
    .await
}

/// Poll one task until it reaches the terminal `STOPPED` status, and answer the record it
/// stopped as.
async fn stopped(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    poll_until(
        case,
        &format!("the task to reach {STOPPED}"),
        || describe(router, task_arn, case),
        |task| task["lastStatus"].as_str() == Some(STOPPED),
    )
    .await
}

/// The one container of `task`.
fn only_container(task: &Value, case: &str) -> Value {
    let containers = list(task, "containers", case);
    containers
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task's one container, got {task}"))
}

/// An image CONFIGURATION declaring neither of the two members that name a program: the
/// document a minimal image pushed by a client carries.
const CONFIGURATION_WITHOUT_A_PROGRAM: &str =
    r#"{"architecture":"amd64","os":"linux","config":{},"rootfs":{"type":"layers","diff_ids":[]}}"#;

/// One declaring a COMMAND, which is the program a container of it runs.
const CONFIGURATION_WITH_A_COMMAND: &str = concat!(
    r#"{"architecture":"amd64","os":"linux","#,
    r#""config":{"Cmd":["sh","-c","exit 0"]},"rootfs":{"type":"layers","diff_ids":[]}}"#
);

/// One declaring an ENTRY POINT instead.
const CONFIGURATION_WITH_AN_ENTRY_POINT: &str = concat!(
    r#"{"architecture":"amd64","os":"linux","#,
    r#""config":{"Entrypoint":["/usr/bin/app"]},"rootfs":{"type":"layers","diff_ids":[]}}"#
);

/// What a row expects of the task it launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Nothing named a program for the container and its image declares none, so Amazon ECS
    /// could not create the container and the task never ran.
    FailedToStart,
    /// The container HAS a program, so the task was started. Whether this emulator performs
    /// that program is another question (a program it performs none of reports no outcome, so
    /// the task goes on running): what this row is about is that the task was not failed.
    Started,
    /// The container's own program ran and ENDED, so the task is stopped for that exit.
    Ran,
}

/// One row of the table: a repository holding an image whose configuration is `configuration`,
/// a container whose definition names `command` and whose launch overrides it with
/// `override_command`, and the outcome the task it places must reach.
struct Row {
    case: &'static str,
    family: &'static str,
    /// The image's configuration, and `None` for a row whose manifest references none at all.
    configuration: Option<&'static str>,
    command: Option<&'static [&'static str]>,
    override_command: Option<&'static [&'static str]>,
    expected: Outcome,
}

const ROWS: &[Row] = &[
    Row {
        case: "an image whose configuration declares neither member, and nothing else names a \
               program",
        family: "noprogram",
        configuration: Some(CONFIGURATION_WITHOUT_A_PROGRAM),
        command: None,
        override_command: None,
        expected: Outcome::FailedToStart,
    },
    Row {
        case: "the same image, with the container's own definition naming a command",
        family: "definedcommand",
        configuration: Some(CONFIGURATION_WITHOUT_A_PROGRAM),
        command: Some(&PROGRAM_THAT_ENDS),
        override_command: None,
        expected: Outcome::Ran,
    },
    Row {
        case: "the same image, with the launch's own override naming the command",
        family: "overriddencommand",
        configuration: Some(CONFIGURATION_WITHOUT_A_PROGRAM),
        command: None,
        override_command: Some(&PROGRAM_THAT_ENDS),
        expected: Outcome::Ran,
    },
    Row {
        case: "an image whose configuration declares a command of its own",
        family: "imagecommand",
        configuration: Some(CONFIGURATION_WITH_A_COMMAND),
        command: None,
        override_command: None,
        expected: Outcome::Started,
    },
    Row {
        case: "an image whose configuration declares an entry point instead",
        family: "imageentrypoint",
        configuration: Some(CONFIGURATION_WITH_AN_ENTRY_POINT),
        command: None,
        override_command: None,
        expected: Outcome::Started,
    },
    Row {
        case: "an image whose stored document references no configuration at all, so nothing \
               states what it declares",
        family: "noconfiguration",
        configuration: None,
        command: None,
        override_command: None,
        expected: Outcome::Started,
    },
];

/// RULE: a container has a program only where something names one — its definition, its
/// launch's override, or the image's own configuration — and a container that has none cannot
/// be created, so its task reaches `STOPPED` with `TaskFailedToStart` and the runtime's
/// `CannotStartContainerError` having never run.
#[tokio::test]
async fn a_container_with_no_program_at_all_fails_to_start_its_task() {
    let router = emulator_with_ecr_and_iam();
    cluster(&router, "the cluster").await;
    let role = execution_role(&router, "startabilityrole").await;

    // Every row's image pushed first, so one pass of launches is placed and read back.
    let mut launched: Vec<(&Row, String, String)> = Vec::new();
    for row in ROWS {
        let image = push_image(&router, row.family, row.configuration, row.case).await;
        let task_arn = run(
            &router,
            row.family,
            &image,
            &role,
            row.command,
            row.override_command,
            row.case,
        )
        .await;
        launched.push((row, image, task_arn));
    }

    for (row, image, task_arn) in &launched {
        let case = row.case;
        match row.expected {
            Outcome::FailedToStart => {
                let task = stopped(&router, task_arn, case).await;
                assert_eq!(
                    string(&task, "stopCode", case),
                    TASK_FAILED_TO_START,
                    "case {case:?}: expected the stop code of a task that never started, got \
                     {task}"
                );
                let reason = string(&task, "stoppedReason", case);
                assert!(
                    reason.contains(CANNOT_START),
                    "case {case:?}: expected the stoppedReason to report {CANNOT_START}, got \
                     {reason:?}"
                );
                assert!(
                    !reason.contains(CANNOT_PULL),
                    "case {case:?}: the image was fetched, so the stop is not a fetch failure, \
                     got {reason:?}"
                );
                // "The Unix timestamp for the time when the task started (the task
                // transitioned from the PENDING state to the RUNNING state)" — a task that
                // never ran has none.
                assert_eq!(
                    task.get("startedAt"),
                    None,
                    "case {case:?}: expected no startedAt on a task that never ran, got {task}"
                );
                let reported = only_container(&task, case);
                assert_eq!(
                    string(&reported, "reason", case),
                    reason,
                    "case {case:?}: expected the container that could not be created to report \
                     the same detail, got {reported}"
                );
                assert_eq!(
                    reported.get("exitCode"),
                    None,
                    "case {case:?}: expected no exitCode on a container that never ran, got \
                     {reported}"
                );
                // The FETCH happened: the digest the registry resolved the reference to is
                // reported for the image the container was going to run.
                assert!(
                    reported["imageDigest"].as_str().is_some_and(|digest| {
                        digest.starts_with("sha256:")
                    }),
                    "case {case:?}: expected the digest the registry resolved {image} to, got \
                     {reported}"
                );
            }
            Outcome::Started => {
                let task = settled(&router, task_arn, case).await;
                assert_eq!(
                    string(&task, "lastStatus", case),
                    RUNNING,
                    "case {case:?}: expected a task whose container has a program to run, got \
                     {task}"
                );
                for member in ["stopCode", "stoppedReason"] {
                    assert_eq!(
                        task.get(member),
                        None,
                        "case {case:?}: expected no {member} on a task that was started, got \
                         {task}"
                    );
                }
            }
            Outcome::Ran => {
                let task = stopped(&router, task_arn, case).await;
                assert_eq!(
                    string(&task, "stopCode", case),
                    ESSENTIAL_CONTAINER_EXITED,
                    "case {case:?}: expected the stop code of a task whose container exited, \
                     got {task}"
                );
                let reported = only_container(&task, case);
                assert_eq!(
                    reported["exitCode"].as_i64(),
                    Some(0),
                    "case {case:?}: expected the status the container's program ended with, \
                     got {reported}"
                );
            }
        }
    }
}

/// One row of the fetch table: why the registry would not serve the image, and the reference
/// the container names.
struct FetchRow {
    case: &'static str,
    family: &'static str,
    /// The repository to create and push the image into, and `None` for a row whose reference
    /// names no repository of this registry at all.
    repository: Option<&'static str>,
    /// A clause the task's `stoppedReason` must carry, which is what the registry answered.
    says: &'static str,
}

/// The repository whose own policy denies the pull.
const DENIED_REPOSITORY: &str = "deniedpull";

/// The repository that holds an image, pulled by a tag it holds none under.
const TAGGED_REPOSITORY: &str = "taggedpull";

/// The repository the FOREIGN reference below names, in another account's registry.
const FOREIGN_REPOSITORY: &str = "foreignpull";

/// Another account's registry id: twelve digits that are not this emulator's account.
const OTHER_ACCOUNT: &str = "210987654321";

/// A tag no image of the repository was pushed under.
const UNHELD_TAG: &str = "does-not-exist-tag";

const FETCH_ROWS: &[FetchRow] = &[
    FetchRow {
        case: "a repository whose own policy denies the pull to the execution role",
        family: "deniedfamily",
        repository: Some(DENIED_REPOSITORY),
        says: "403 Forbidden",
    },
    FetchRow {
        case: "a tag the repository holds no image under",
        family: "untaggedfamily",
        repository: Some(TAGGED_REPOSITORY),
        says: "not found",
    },
    FetchRow {
        case: "a reference into another account's registry",
        family: "foreignfamily",
        repository: None,
        // A registry's contents are shared only through its OWN resource-based policy, so a
        // reference into another account's is a pull nothing permits: the manifest request is
        // answered with the status a registry refuses an unauthorized one with.
        says: "403 Forbidden",
    },
];

/// RULE: a task whose image the registry will not serve never runs either — the same
/// `TaskFailedToStart`, with the runtime's `CannotPullContainerError` naming the reference it
/// could not fetch — whichever of the registry's answers refused the fetch.
#[tokio::test]
async fn a_task_whose_image_the_registry_refuses_stops_with_the_fetch_failure() {
    let router = emulator_with_ecr_and_iam();
    cluster(&router, "the cluster").await;
    let role = execution_role(&router, "fetchrefusalrole").await;

    // The registry's records: one repository the policy below denies the pull of, and one
    // holding an image under its own tag. Both hold a real image, so what each row reads is
    // the registry's answer about the FETCH rather than an empty repository.
    let denied = push_image(
        &router,
        DENIED_REPOSITORY,
        Some(CONFIGURATION_WITH_A_COMMAND),
        "the denied repository",
    )
    .await;
    push_image(
        &router,
        TAGGED_REPOSITORY,
        Some(CONFIGURATION_WITH_A_COMMAND),
        "the tagged repository",
    )
    .await;
    // "Even if multiple statements allow the operation on the resource, if only one statement
    // denies that operation, then the explicit deny overrides any allow": the repository
    // shares itself with nobody and refuses the role the two calls a pull makes. A repository
    // policy applies to the repository it is attached to, so it names no resource of its own.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": { "AWS": role },
            "Action": ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer"],
        }],
    })
    .to_string();
    ecr(
        &router,
        "SetRepositoryPolicy",
        json!({ "repositoryName": DENIED_REPOSITORY, "policyText": policy }),
        "the deny policy",
    )
    .await;

    let mut launched: Vec<(&FetchRow, String, String)> = Vec::new();
    for row in FETCH_ROWS {
        let image = match row.repository {
            Some(DENIED_REPOSITORY) => denied.clone(),
            Some(repository) => ecr_image(&format!("{repository}:{UNHELD_TAG}")),
            None => format!(
                "{OTHER_ACCOUNT}.dkr.ecr.{SIGNED_REGION}.amazonaws.com/{FOREIGN_REPOSITORY}:{IMAGE_TAG}"
            ),
        };
        let task_arn = run(
            &router,
            row.family,
            &image,
            &role,
            Some(&PROGRAM_THAT_ENDS),
            None,
            row.case,
        )
        .await;
        launched.push((row, image, task_arn));
    }

    for (row, image, task_arn) in &launched {
        let case = row.case;
        let task = stopped(&router, task_arn, case).await;
        assert_eq!(
            string(&task, "stopCode", case),
            TASK_FAILED_TO_START,
            "case {case:?}: expected the stop code of a task that never started, got {task}"
        );
        let reason = string(&task, "stoppedReason", case);
        assert!(
            reason.contains(CANNOT_PULL),
            "case {case:?}: expected the stoppedReason to report {CANNOT_PULL}, got {reason:?}"
        );
        assert!(
            reason.contains(image.as_str()),
            "case {case:?}: expected the stoppedReason to name the reference {image}, got \
             {reason:?}"
        );
        assert!(
            reason.contains(row.says),
            "case {case:?}: expected the stoppedReason to carry what the registry answered \
             ({:?}), got {reason:?}",
            row.says
        );
        assert_eq!(
            task.get("startedAt"),
            None,
            "case {case:?}: expected no startedAt on a task that never ran, got {task}"
        );
        let reported = only_container(&task, case);
        assert_eq!(
            string(&reported, "reason", case),
            reason,
            "case {case:?}: expected the container whose image it was to report the same \
             detail, got {reported}"
        );
        assert_eq!(
            reported.get("imageDigest"),
            None,
            "case {case:?}: expected no imageDigest on an image no registry resolved, got \
             {reported}"
        );
    }
}
