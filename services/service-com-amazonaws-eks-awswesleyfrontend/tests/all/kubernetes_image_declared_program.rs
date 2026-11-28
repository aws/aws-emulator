// SPDX-License-Identifier: CC0-1.0

//! The program a container runs when its pod declares NONE: the one its IMAGE declares.
//!
//! The RULE under test: **a container's program is the argument vector its `command` and
//! `args` declare: `Container$command` is the "entrypoint array ... the container image's
//! ENTRYPOINT is used if this is not provided" and `Container$args` the "arguments to the
//! entrypoint. The container image's CMD is used if this is not provided" — so a container
//! that declares NEITHER runs the program its IMAGE declares, which the REGISTRY serving
//! that image is what states (the configuration a push stored for a private repository, and
//! the published image itself for one of the public gallery's). The pod then reports what
//! running that program did, exactly as it does for a program the manifest named: a
//! program that exits with the status a completed program exits with leaves the pod
//! `Succeeded` with `state.terminated.reason` `Completed`. A container whose image no
//! registry of this emulator states a program for, and one that declared PART of a vector
//! of its own (its `args`, which replace the image's command rather than its entry point),
//! are containers nothing was performed for: each reports the runtime's own reason for one
//! it could not start, and no exit status.**
//!
//! The rows differ along what the rule generalises over: whether the manifest declares a
//! command, arguments, both or neither; whether the image is one a registry of this
//! emulator states a program for; which program that image declares (one the shared session
//! performs and one it does not); and whether the release the reference names is one the
//! registry publishes.
//!
//! The client is the raw, kubeconfig-built one `kubernetes_api.rs` dials with, and the node
//! the pods are scheduled onto is the one `kubernetes_cluster_resources.rs` registers on a
//! node group of the cluster. Every wait is for a CONDITION, re-read until it holds or a
//! generous deadline passes; nothing here measures elapsed time.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::kubernetes_api::{dial, ApiAnswer, Kubeconfig};
use crate::kubernetes_cluster_resources::{
    active_nodegroup, instance, published_cluster, register_nodes,
};

/// How long a polled condition is given before the case fails, and how often it is looked
/// at again. Generous: the spans these cases wait out are the service's own pod-start and
/// container-run spans.
const DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The container every pod of this suite declares.
const CONTAINER: &str = "app";

/// What the pod a row created must have come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// Its container's program ran and exited with this status.
    Exited(i32),
    /// Nothing was performed for its container: no exit status is claimed.
    NeverStarted,
}

/// One row: the pod, the image its container runs, the command its manifest declares (if
/// any), and what the pod must have come to.
struct Row {
    case: &'static str,
    pod: &'static str,
    image: &'static str,
    /// `spec.containers[0].command`, and `None` for a container that declares none — the
    /// case the rule is about.
    command: Option<&'static [&'static str]>,
    /// `spec.containers[0].args`, which replace the image's COMMAND rather than its entry
    /// point — a container declaring them has part of a vector of its own.
    args: Option<&'static [&'static str]>,
    ended: Ended,
    phase: &'static str,
}

/// The rows. The images are references into the PUBLIC GALLERY this emulator serves, whose
/// published repositories state the program each of their images declares; the releases
/// named are ones that gallery publishes, except where a row says otherwise.
const ROWS: &[Row] = &[
    Row {
        case: "no command at all, on an image whose own program is a shell",
        pod: "image-shell",
        image: "public.ecr.aws/docker/library/busybox:1.37",
        command: None,
        args: None,
        // A shell invoked with no program text reads its commands from a standard input
        // this container has none of, so it runs nothing and exits as a shell that
        // executed no command does.
        ended: Ended::Exited(0),
        phase: "Succeeded",
    },
    Row {
        case: "the same image under another release the gallery publishes",
        pod: "image-shell-channel",
        image: "public.ecr.aws/docker/library/busybox:musl",
        command: None,
        args: None,
        ended: Ended::Exited(0),
        phase: "Succeeded",
    },
    Row {
        case: "a command of its own on that same image",
        pod: "image-overridden",
        image: "public.ecr.aws/docker/library/busybox:1.37",
        command: Some(&["sh", "-c", "exit 4"]),
        args: None,
        ended: Ended::Exited(4),
        phase: "Failed",
    },
    Row {
        case: "a command and arguments of its own on that image",
        pod: "image-overridden-args",
        image: "public.ecr.aws/docker/library/busybox:1.37",
        command: Some(&["sh", "-c"]),
        args: Some(&["exit 5"]),
        ended: Ended::Exited(5),
        phase: "Failed",
    },
    Row {
        // `args` replace the image's COMMAND and leave its entry point in place, which is
        // not what the registry answers here (one vector, the entry point and the command
        // together), so nothing is performed for such a container rather than the image's
        // own program being run with the words the manifest declared thrown away.
        case: "arguments of its own and no command, on that same image",
        pod: "image-args-only",
        image: "public.ecr.aws/docker/library/busybox:1.37",
        command: None,
        args: Some(&["-c", "exit 0"]),
        ended: Ended::NeverStarted,
        phase: "Failed",
    },
    Row {
        case: "no command, on an image whose declared program needs a server",
        pod: "image-server",
        image: "public.ecr.aws/docker/library/nginx:1.29",
        command: None,
        args: None,
        ended: Ended::NeverStarted,
        phase: "Failed",
    },
    Row {
        case: "no command, under a release that gallery publishes no image of",
        pod: "image-unpublished",
        image: "public.ecr.aws/docker/library/busybox:no-such-release",
        command: None,
        args: None,
        ended: Ended::NeverStarted,
        phase: "Failed",
    },
    Row {
        case: "no command, on an image no registry of this emulator states a program for",
        pod: "image-foreign",
        image: "registry.example.com/probe:1",
        command: None,
        args: None,
        ended: Ended::NeverStarted,
        phase: "Failed",
    },
];

/// The manifest `row` is created from.
fn manifest(row: &Row) -> Value {
    let mut container = json!({ "name": CONTAINER, "image": row.image });
    if let Some(command) = row.command {
        container["command"] = json!(command);
    }
    if let Some(args) = row.args {
        container["args"] = json!(args);
    }
    json!({
        "metadata": { "name": row.pod },
        "spec": {
            "restartPolicy": "Never",
            "containers": [container],
        },
    })
}

/// Poll the pod `pod` until it reaches a terminal phase, and answer the last read.
async fn concluded(config: &Kubeconfig, case: &str, pod: &str) -> ApiAnswer {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let read = dial(
            config,
            case,
            "GET",
            &format!("/api/v1/namespaces/default/pods/{pod}"),
            None,
        )
        .await;
        let phase = read.body["status"]["phase"].clone();
        if phase == json!("Succeeded") || phase == json!("Failed") {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the pod to terminate; the last answer was {read:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_container_that_declares_no_program_runs_the_one_its_image_declares() {
    let cluster = "image-program-cluster";
    let (router, engine, config) = published_cluster(cluster).await;
    active_nodegroup(&router, cluster, "runners").await;
    register_nodes(
        &engine,
        cluster,
        "runners",
        std::slice::from_ref(&instance(
            "i-0ccc3333ccc3333cc",
            "us-west-2a",
            "t3.medium",
            "10.0.3.33",
        )),
    );

    for row in ROWS {
        let created = dial(
            &config,
            row.case,
            "POST",
            "/api/v1/namespaces/default/pods",
            Some(&manifest(row)),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: the pod must be created, got {created:?}",
            row.case
        );
        // The manifest is stored as it was submitted: resolving the image's own program
        // is the node's work, and it puts no vector on the pod a client reads back.
        assert_eq!(
            (
                created.body["spec"]["containers"][0]["command"].clone(),
                created.body["spec"]["containers"][0]["args"].clone(),
            ),
            (
                row.command.map(|command| json!(command)).unwrap_or(Value::Null),
                row.args.map(|args| json!(args)).unwrap_or(Value::Null),
            ),
            "case {:?}: the admitted spec carries the vector the manifest declared, got \
             {created:?}",
            row.case
        );

        let read = concluded(&config, row.case, row.pod).await;
        assert_eq!(
            read.body["status"]["phase"],
            json!(row.phase),
            "case {:?}: the phase the container's own outcome decides, got {read:?}",
            row.case
        );
        let held = read.body["status"]["containerStatuses"][0].clone();
        let terminated = held["state"]["terminated"].clone();
        match row.ended {
            Ended::Exited(status) => {
                assert_eq!(
                    terminated["exitCode"],
                    json!(status),
                    "case {:?}: expected the status the program exited with, got {held}",
                    row.case
                );
                assert_eq!(
                    terminated["reason"],
                    json!(if status == 0 { "Completed" } else { "Error" }),
                    "case {:?}: expected the reason the runtime words that status with, got \
                     {held}",
                    row.case
                );
                assert_eq!(
                    held["started"],
                    json!(true),
                    "case {:?}: a container whose program ran was started, got {held}",
                    row.case
                );
                assert_eq!(
                    read.body["status"]["reason"],
                    Value::Null,
                    "case {:?}: a pod whose containers ran names no pod-level reason, got \
                     {read:?}",
                    row.case
                );
            }
            Ended::NeverStarted => {
                assert_eq!(
                    terminated["reason"],
                    json!("StartError"),
                    "case {:?}: the runtime's reason for a container it could not start, got \
                     {held}",
                    row.case
                );
                assert_eq!(
                    held["started"],
                    json!(false),
                    "case {:?}: a container that never ran was never started, got {held}",
                    row.case
                );
            }
        }
    }
}
