// SPDX-License-Identifier: CC0-1.0

//! What a pod's containers DO on the node the cluster scheduled them onto, read over
//! the address the cluster itself publishes.
//!
//! The RULE under test: **a pod's `status.phase` and its container statuses report what
//! its containers really did. `Container.command` is the "entrypoint array" and
//! `Container.args` the "arguments to the entrypoint", so the two are the argument
//! vector a container runs — and once the node's kubelet has pulled the images it
//! creates and starts those containers, so the pod reports `Running` while a container
//! of it is running, each running container carries `state.running`, and a container
//! whose program ended carries `state.terminated` with the status that program exited
//! with beside the reason the runtime words for it (`Completed` for the status a program
//! that completed its work exits with, `Error` for any other). The terminal phase is the
//! one those statuses and the pod's `restartPolicy` decide: `Succeeded` where "all
//! containers in the pod have terminated in success, and will not be restarted",
//! `Failed` where "at least one container has terminated in failure", and NEITHER under
//! a policy that starts a terminated container again. A container nothing was performed
//! for — one whose program is the one inside its image, which this emulator starts none
//! of — claims no exit status at all: it reports the runtime's own reason for a
//! container it could not start, and the pod ends in the phase a pod with such a
//! container reports.**
//!
//! The rows differ along what the rule generalises over: the PROGRAM the container
//! declares (a shell program, an argument vector exec'd with no shell, a program that
//! computes its own status, one that reads an environment variable the pod declares, a
//! program this emulator provides none of, and no program at all), the STATUS it exits
//! with (zero and several non-zero ones, which is what tells a reported status from a
//! constant), and the `restartPolicy` the pod was admitted under (all three Kubernetes
//! defines).
//!
//! The client is the raw, kubeconfig-built one `kubernetes_api.rs` dials with, and the
//! node the pods are scheduled onto is the one `kubernetes_cluster_resources.rs`
//! registers on a node group of the cluster. Every wait is for a CONDITION, re-read
//! until it holds or a generous deadline passes; nothing here measures elapsed time.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::kubernetes_api::{dial, ApiAnswer, Kubeconfig};
use crate::kubernetes_cluster_resources::{
    active_nodegroup, instance, published_cluster, register_nodes,
};

/// How long a polled condition is given before the case fails. Generous: the spans
/// these cases wait out are the service's own pod-start and container-run spans, and
/// nothing here measures elapsed time.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often a polled condition is looked at again. Nothing inside the emulator sleeps
/// for a due instant, so the condition is polled rather than waited for.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The image every pod of this suite runs: a reference no registry this emulator holds
/// answers for, so nothing is decided about the pull and what is under test is what the
/// container's own program did.
const IMAGE: &str = "registry.example.com/probe:1";

/// The container every pod of this suite declares.
const CONTAINER: &str = "app";

/// What a pod's one container is expected to have come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// Its program ran and exited with this status, so the container terminated and the
    /// pod reached the phase its status and the policy decide.
    Exited(i32),
    /// Nothing was performed for it: no exit status is claimed and the container reports
    /// the runtime's reason for one it could not start.
    NeverStarted,
    /// Its program ran and the pod's policy starts a terminated container again, so the
    /// pod reaches no terminal phase and the container keeps reporting that it runs.
    KeptRunning,
}

/// One row: the pod, the program its container declares, the policy it was admitted
/// under, and what it must have come to.
struct Row {
    case: &'static str,
    pod: &'static str,
    /// `spec.containers[0].command`, and `None` for a container that declares none.
    command: Option<&'static [&'static str]>,
    /// `spec.containers[0].args`.
    args: Option<&'static [&'static str]>,
    /// `spec.restartPolicy`.
    restart_policy: &'static str,
    ended: Ended,
    /// The terminal `status.phase` the pod must reach, and `None` for a pod that reaches
    /// none.
    phase: Option<&'static str>,
}

/// The rows. The programs are written out of the catalog the shared session performs
/// (`service_common::posix_shell_session::BUILT_IN_COMMANDS` and `VOLUME_COMMANDS`),
/// and the statuses they exit with are the shell's own.
const ROWS: &[Row] = &[
    Row {
        case: "a one-shot shell program that writes a line and exits zero",
        pod: "run-echo",
        command: Some(&["sh", "-c", "echo pulled-ok"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(0),
        phase: Some("Succeeded"),
    },
    Row {
        case: "an argument vector exec'd with no shell at all",
        pod: "run-argv",
        command: Some(&["/bin/echo", "pulled-ok"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(0),
        phase: Some("Succeeded"),
    },
    Row {
        case: "a vector whose operands arrive as the entrypoint's args",
        pod: "run-args",
        command: Some(&["sh", "-c"]),
        args: Some(&["echo from-args"]),
        restart_policy: "Never",
        ended: Ended::Exited(0),
        phase: Some("Succeeded"),
    },
    Row {
        case: "a program that exits with a status of its own choosing",
        pod: "run-exit-7",
        command: Some(&["sh", "-c", "echo before; exit 7"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(7),
        phase: Some("Failed"),
    },
    Row {
        case: "a program whose last command decides the status",
        pod: "run-false",
        command: Some(&["/bin/false"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(1),
        phase: Some("Failed"),
    },
    Row {
        case: "a program that branches on the environment the pod gave it",
        pod: "run-environment",
        command: Some(&["sh", "-c", "test \"$PROBE\" = ok"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(0),
        phase: Some("Succeeded"),
    },
    Row {
        case: "the same program reading a variable the pod declares none of",
        pod: "run-environment-absent",
        command: Some(&["sh", "-c", "test \"$ABSENT\" = ok"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(1),
        phase: Some("Failed"),
    },
    Row {
        case: "a program whose status is what an earlier command of it counted",
        pod: "run-pipeline",
        command: Some(&["sh", "-c", "exit $(seq 3 | wc -l)"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::Exited(3),
        phase: Some("Failed"),
    },
    Row {
        case: "a failing program under a policy that starts it again",
        pod: "run-on-failure",
        command: Some(&["sh", "-c", "exit 3"]),
        args: None,
        restart_policy: "OnFailure",
        ended: Ended::KeptRunning,
        phase: None,
    },
    Row {
        case: "a succeeding program under the policy that always starts it again",
        pod: "run-always",
        command: Some(&["sh", "-c", "echo served"]),
        args: None,
        restart_policy: "Always",
        ended: Ended::KeptRunning,
        phase: None,
    },
    Row {
        case: "a succeeding program under a policy that starts only failures again",
        pod: "run-on-failure-ok",
        command: Some(&["sh", "-c", "exit 0"]),
        args: None,
        restart_policy: "OnFailure",
        ended: Ended::Exited(0),
        phase: Some("Succeeded"),
    },
    Row {
        case: "a program this emulator provides none of",
        pod: "run-unknown",
        command: Some(&["/usr/local/bin/probe-agent", "--serve"]),
        args: None,
        restart_policy: "Never",
        ended: Ended::NeverStarted,
        phase: Some("Failed"),
    },
    Row {
        case: "a container that declares no program at all",
        pod: "run-image-entrypoint",
        command: None,
        args: None,
        restart_policy: "Never",
        ended: Ended::NeverStarted,
        phase: Some("Failed"),
    },
];

/// The value the `run-environment` row's program reads, declared on the container the
/// way `Container.env` declares one.
const PROBE_VARIABLE: (&str, &str) = ("PROBE", "ok");

/// The manifest `row` is created from.
fn manifest(row: &Row) -> Value {
    let mut container = json!({
        "name": CONTAINER,
        "image": IMAGE,
        "env": [{ "name": PROBE_VARIABLE.0, "value": PROBE_VARIABLE.1 }],
    });
    if let Some(command) = row.command {
        container["command"] = json!(command);
    }
    if let Some(args) = row.args {
        container["args"] = json!(args);
    }
    json!({
        "metadata": { "name": row.pod },
        "spec": {
            "restartPolicy": row.restart_policy,
            "containers": [container],
        },
    })
}

/// Read the pod `pod` back.
async fn read_pod(config: &Kubeconfig, case: &str, pod: &str) -> ApiAnswer {
    dial(
        config,
        case,
        "GET",
        &format!("/api/v1/namespaces/default/pods/{pod}"),
        None,
    )
    .await
}

/// Poll the pod `pod` until `holds` answers true, and answer the last read; fail naming
/// `what` when the deadline passes first.
async fn pod_until(
    config: &Kubeconfig,
    case: &str,
    pod: &str,
    what: &str,
    holds: impl Fn(&ApiAnswer) -> bool,
) -> ApiAnswer {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let read = read_pod(config, case, pod).await;
        if holds(&read) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {what}; the last answer was {read:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The one container status of the pod `read` reports.
fn container_status(case: &str, read: &ApiAnswer) -> Value {
    let statuses = read.body["status"]["containerStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        statuses.len(),
        1,
        "case {case:?}: one status per container, got {read:?}"
    );
    statuses[0].clone()
}

#[tokio::test]
async fn a_pods_container_runs_the_program_it_declares_and_the_pod_reports_what_it_did() {
    let cluster = "container-run-cluster";
    let (router, engine, config) = published_cluster(cluster).await;
    active_nodegroup(&router, cluster, "runners").await;
    register_nodes(
        &engine,
        cluster,
        "runners",
        std::slice::from_ref(&instance("i-0aaa1111aaa1111aa", "us-west-2a", "t3.medium", "10.0.1.11")),
    );

    for row in ROWS {
        let Row {
            case,
            pod,
            restart_policy,
            ended,
            phase,
            ..
        } = row;
        let created = dial(
            &config,
            case,
            "POST",
            "/api/v1/namespaces/default/pods",
            Some(&manifest(row)),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the pod must be created, got {created:?}"
        );
        assert_eq!(
            created.body["spec"]["restartPolicy"],
            json!(restart_policy),
            "case {case:?}: the pod is admitted under the policy it named, got {created:?}"
        );
        assert_eq!(
            created.body["status"]["phase"],
            json!("Pending"),
            "case {case:?}: a pod the node has not taken has not started, got {created:?}"
        );

        match (ended, phase) {
            (Ended::Exited(status), Some(expected)) => {
                let concluded = pod_until(&config, case, pod, "the pod to terminate", |read| {
                    read.body["status"]["phase"] == json!("Succeeded")
                        || read.body["status"]["phase"] == json!("Failed")
                })
                .await;
                assert_eq!(
                    concluded.body["status"]["phase"],
                    json!(expected),
                    "case {case:?}: the phase the container's status and the policy decide, \
                     got {concluded:?}"
                );
                let held = container_status(case, &concluded);
                let terminated = held["state"]["terminated"].clone();
                assert_eq!(
                    terminated["exitCode"],
                    json!(status),
                    "case {case:?}: expected the status the program exited with, got {held}"
                );
                assert_eq!(
                    terminated["reason"],
                    json!(if *status == 0 { "Completed" } else { "Error" }),
                    "case {case:?}: expected the reason the runtime words that status with, \
                     got {held}"
                );
                assert!(
                    terminated["startedAt"].is_string() && terminated["finishedAt"].is_string(),
                    "case {case:?}: a container that ran reports when it ran, got {held}"
                );
                assert_eq!(
                    (held["started"].clone(), held["ready"].clone()),
                    (json!(true), json!(false)),
                    "case {case:?}: a container that ran and ended was started and is no \
                     longer ready, got {held}"
                );
                // NOTHING an unperformed container reports is claimed for a container
                // that really ran: the pod names no condition of its own.
                assert_eq!(
                    concluded.body["status"]["reason"],
                    Value::Null,
                    "case {case:?}: a pod whose containers ran names no pod-level reason, \
                     got {concluded:?}"
                );
            }
            (Ended::NeverStarted, Some(expected)) => {
                let concluded = pod_until(&config, case, pod, "the pod to terminate", |read| {
                    read.body["status"]["phase"] == json!("Succeeded")
                        || read.body["status"]["phase"] == json!("Failed")
                })
                .await;
                assert_eq!(
                    concluded.body["status"]["phase"],
                    json!(expected),
                    "case {case:?}: a pod with a container nothing started reaches the phase \
                     such a container reports, got {concluded:?}"
                );
                let held = container_status(case, &concluded);
                assert_eq!(
                    held["state"]["terminated"]["reason"],
                    json!("StartError"),
                    "case {case:?}: the runtime's reason for a container it could not start, \
                     got {held}"
                );
                assert_eq!(
                    (held["started"].clone(), held["ready"].clone()),
                    (json!(false), json!(false)),
                    "case {case:?}: a container that never ran was never started, got {held}"
                );
                assert_eq!(
                    concluded.body["status"]["reason"],
                    json!("StartError"),
                    "case {case:?}: the pod names what stopped it, got {concluded:?}"
                );
            }
            (Ended::KeptRunning, None) => {
                let running = pod_until(&config, case, pod, "the pod to run", |read| {
                    read.body["status"]["phase"] == json!("Running")
                })
                .await;
                let held = container_status(case, &running);
                assert!(
                    held["state"]["running"]["startedAt"].is_string(),
                    "case {case:?}: a running container reports when it started, got {held}"
                );
                assert_eq!(
                    (held["started"].clone(), held["ready"].clone()),
                    (json!(true), json!(true)),
                    "case {case:?}: a running container with no readiness probe is ready, \
                     got {held}"
                );
                // It reaches NO terminal phase: the policy starts a terminated container
                // again, so every later read answers the same running pod.
                for look in 0..4 {
                    let read = read_pod(&config, case, pod).await;
                    assert_eq!(
                        read.body["status"]["phase"],
                        json!("Running"),
                        "case {case:?}: read {look}: a pod whose containers are started again \
                         reaches no terminal phase, got {read:?}"
                    );
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
            (ended, phase) => panic!("case {case:?}: the row states {ended:?} with {phase:?}"),
        }
    }
}

/// A pod whose container runs a program that states a time of its own is RUNNING for
/// that time: the phase a pod whose containers are running reports is observable, and
/// the terminal phase follows it.
#[tokio::test]
async fn a_pod_reports_running_while_its_containers_program_runs_and_then_terminates() {
    let cluster = "container-running-cluster";
    let (router, engine, config) = published_cluster(cluster).await;
    active_nodegroup(&router, cluster, "runners").await;
    register_nodes(
        &engine,
        cluster,
        "runners",
        std::slice::from_ref(&instance("i-0bbb2222bbb2222bb", "us-west-2b", "t3.medium", "10.0.2.22")),
    );

    // The rows differ along the time the program states it takes: one that states a
    // span of its own, and one that states none at all. Both must be observed RUNNING
    // and both must terminate in success.
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "a program that states the time it takes",
            "running-sleep",
            &["sh", "-c", "echo starting; sleep 2; echo done"],
        ),
        (
            "a program that states none",
            "running-instant",
            &["sh", "-c", "echo done"],
        ),
    ];

    for (case, pod, command) in rows {
        let created = dial(
            &config,
            case,
            "POST",
            "/api/v1/namespaces/default/pods",
            Some(&json!({
                "metadata": { "name": pod },
                "spec": {
                    "restartPolicy": "Never",
                    "containers": [{ "name": CONTAINER, "image": IMAGE, "command": command }],
                },
            })),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the pod must be created, got {created:?}"
        );
        // The phase a pod whose containers were started is in, observed before the
        // terminal one — the span the move out of it is owed is non-zero.
        let running = pod_until(&config, case, pod, "the pod to leave Pending", |read| {
            read.body["status"]["phase"] != json!("Pending")
        })
        .await;
        assert_eq!(
            running.body["status"]["phase"],
            json!("Running"),
            "case {case:?}: a pod whose containers were started runs, got {running:?}"
        );
        assert!(
            running.body["status"]["startTime"].is_string(),
            "case {case:?}: a pod the kubelet acknowledged reports when, got {running:?}"
        );
        let concluded = pod_until(&config, case, pod, "the pod to terminate", |read| {
            read.body["status"]["phase"] == json!("Succeeded")
                || read.body["status"]["phase"] == json!("Failed")
        })
        .await;
        assert_eq!(
            concluded.body["status"]["phase"],
            json!("Succeeded"),
            "case {case:?}: every container exited in success, got {concluded:?}"
        );
        let held = container_status(case, &concluded);
        assert_eq!(
            held["state"]["terminated"]["exitCode"],
            json!(0),
            "case {case:?}: the status the program exited with, got {held}"
        );
    }
}
