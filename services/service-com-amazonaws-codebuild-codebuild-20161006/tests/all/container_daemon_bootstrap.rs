// SPDX-License-Identifier: CC0-1.0

//! The command a CONTAINER build starts the Docker daemon with, driven over the wire the
//! router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: a build environment runs the Docker daemon only when it was started in
//! PRIVILEGED mode — `ProjectEnvironment$privilegedMode` "enables running the Docker daemon
//! inside a Docker container" — and the managed images carry the daemon's own program, so the
//! documented bootstrap a container build begins with (starting `dockerd` from its install
//! phase, in the background, with its output redirected) is a command such an environment
//! performs: it exits zero and the phase goes on to the commands after it. The same command
//! in an environment that is not privileged does not start a daemon: it exits non-zero, which
//! fails the phase it is in with the command-execution error the environment reports for a
//! command that answered failure, and the commands after it do not run.
//!
//! The rows differ along what the rule generalises over: whether the environment is
//! privileged, the FORM the bootstrap command is written in (the options AWS documents it
//! with, the short form, and a bare invocation), and whether the words it carries are an
//! invocation of the daemon at all.
//!
//! What the Docker CLI's own commands do once the daemon is up is `ecr_image_push.rs`'s
//! subject, and the forms this environment's shell still does not perform are
//! `shell_probe.rs`'s.

use serde_json::{json, Value};

use super::project_build_lifecycle::{create_project, emulator, phase_of, settled_build};

/// The environment members a row's project is created with: the managed container image, and
/// `privilegedMode` as the row states it.
fn environment(privileged: bool) -> Value {
    json!({
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
            "privilegedMode": privileged,
        },
    })
}

/// A buildspec whose install phase runs the one command `bootstrap`, which is the one-command
/// case of [`waiting_buildspec`].
fn buildspec(bootstrap: &str) -> String {
    waiting_buildspec(&[bootstrap])
}

/// The value a settled build reports for the exported variable `name`, or an empty string.
fn exported(build: &Value, name: &str) -> String {
    build["exportedEnvironmentVariables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|variable| variable["name"] == json!(name))
        .and_then(|variable| variable["value"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A buildspec whose install phase runs `bootstrap`, one command per line, and whose build
/// phase then publishes the marker a reader can only see when the phase before it ran to its
/// end.
fn waiting_buildspec(bootstrap: &[&str]) -> String {
    let commands: String = bootstrap
        .iter()
        .map(|command| {
            let quoted = command.replace('\\', "\\\\").replace('"', "\\\"");
            format!("      - \"{quoted}\"\n")
        })
        .collect();
    format!(
        "version: 0.2\nenv:\n  exported-variables:\n    - STAGE\nphases:\n  install:\n    \
         commands:\n{commands}      - \"export STAGE=installed\"\n  build:\n    \
         commands:\n      - \"export STAGE=built\"\n"
    )
}

/// RULE: the daemon a privileged build environment runs ANSWERS the command that asks it about
/// itself, which is what the documented bootstrap WAITS on before it builds anything — so that
/// wait exits zero there and the phase continues, and the same command in an environment that
/// may run no daemon exits non-zero and fails the phase.
#[tokio::test]
async fn the_daemon_a_privileged_environment_runs_answers_the_wait_for_it() {
    /// One row: the case, whether the environment is privileged, the install commands, the
    /// build's terminal status and the stage its commands got as far as.
    type Row = (
        &'static str,
        bool,
        &'static [&'static str],
        &'static str,
        &'static str,
    );

    let rows: Vec<Row> = vec![
        (
            // The form AWS documents for a container build: the daemon started in the
            // background, and then the wait that polls it until it answers.
            "the documented bootstrap and the wait that polls the daemon",
            true,
            &[
                "nohup dockerd --storage-driver=overlay2 > /tmp/dockerd.log 2>&1 &",
                "timeout 15 sh -c \"until docker info; do sleep 1; done\"",
            ],
            "SUCCEEDED",
            "built",
        ),
        (
            "the daemon asked about itself directly",
            true,
            &["dockerd &", "docker info"],
            "SUCCEEDED",
            "built",
        ),
        (
            // No daemon runs in an environment that was not started in privileged mode, so
            // there is none to answer: the command fails and ends its phase.
            "the daemon asked about itself where the environment is not privileged",
            false,
            &["docker info"],
            "FAILED",
            "",
        ),
        (
            // The wait is bounded by the duration it states: a daemon that never answers
            // leaves it reporting the duration it ran out of rather than succeeding.
            "the wait where no daemon can answer it",
            false,
            &["timeout 15 sh -c \"until docker info; do sleep 1; done\""],
            "FAILED",
            "",
        ),
        (
            // The command takes no operand at all, so words after it are no invocation of it.
            "words that are no invocation of the command",
            true,
            &["dockerd &", "docker info --everything-of-it"],
            "FAILED",
            "",
        ),
    ];

    let router = emulator();
    for (index, (case, privileged, bootstrap, status, stage)) in rows.iter().enumerate() {
        let project = format!("daemon-wait-{index}");
        create_project(
            &router,
            &project,
            json!({
                "source": {"type": "NO_SOURCE", "buildspec": waiting_buildspec(bootstrap)},
                "artifacts": {"type": "NO_ARTIFACTS"},
                "environment": environment(*privileged)["environment"].clone(),
            }),
        )
        .await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"].as_str(),
            Some(*status),
            "case {case:?}: expected buildStatus {status}, got {build}"
        );
        assert_eq!(
            exported(&build, "STAGE"),
            *stage,
            "case {case:?}: expected the commands to reach {stage:?}, got {build}"
        );
        if *status == "FAILED" {
            let install = phase_of(&build, "INSTALL").unwrap_or_default();
            assert_eq!(
                install["phaseStatus"].as_str(),
                Some("FAILED"),
                "case {case:?}: the phase that ran it must be the failing one, got {install}"
            );
        }
    }
}

/// RULE: the Docker daemon is a program a build environment runs only in privileged mode, so
/// the bootstrap command a container build starts it with succeeds there and the phase
/// continues, and fails in an environment that is not privileged.
#[tokio::test]
async fn the_daemon_bootstrap_succeeds_only_in_a_privileged_build_environment() {
    /// One row: the case, whether the environment is privileged, the bootstrap command, the
    /// build's terminal status and the stage its commands got as far as.
    type Row = (&'static str, bool, &'static str, &'static str, &'static str);

    let rows: Vec<Row> = vec![
        (
            // The form AWS documents for a container build: the daemon started in the
            // background, with the storage-driver option it names.
            "the documented bootstrap in a privileged environment",
            true,
            "nohup dockerd --storage-driver=overlay2 --debug > /tmp/dockerd.log 2>&1 &",
            "SUCCEEDED",
            "built",
        ),
        (
            "the same bootstrap where the environment is not privileged",
            false,
            "nohup dockerd --storage-driver=overlay2 --debug > /tmp/dockerd.log 2>&1 &",
            "FAILED",
            // The command that failed ends its phase, so the commands after it — the one
            // that publishes the stage — never run.
            "",
        ),
        (
            "the short bootstrap in a privileged environment",
            true,
            "nohup dockerd --storage-driver=overlay2 > /tmp/dockerd.log 2>&1 &",
            "SUCCEEDED",
            "built",
        ),
        (
            "the daemon started with no option at all",
            true,
            "dockerd &",
            "SUCCEEDED",
            "built",
        ),
        (
            "the daemon with an option written apart from its value",
            true,
            "dockerd --storage-driver overlay2 &",
            "SUCCEEDED",
            "built",
        ),
        (
            // Words that are no invocation of the daemon: it takes options and no operand,
            // so this is a command the environment has no program for.
            "words that are no invocation of the daemon",
            true,
            "dockerd start-the-daemon",
            "FAILED",
            "",
        ),
    ];

    let router = emulator();
    for (index, (case, privileged, bootstrap, status, stage)) in rows.iter().enumerate() {
        let project = format!("daemon-bootstrap-{index}");
        create_project(
            &router,
            &project,
            json!({
                "source": {"type": "NO_SOURCE", "buildspec": buildspec(bootstrap)},
                "artifacts": {"type": "NO_ARTIFACTS"},
                "environment": environment(*privileged)["environment"].clone(),
            }),
        )
        .await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"].as_str(),
            Some(*status),
            "case {case:?}: expected buildStatus {status}, got {build}"
        );
        assert_eq!(
            exported(&build, "STAGE"),
            *stage,
            "case {case:?}: expected the commands to reach {stage:?}, got {build}"
        );
        if *status == "FAILED" {
            // The phase that ran the command carries why it failed, naming the command and
            // the status it answered.
            let install = phase_of(&build, "INSTALL").unwrap_or_default();
            let reported = install["contexts"][0]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                reported.contains("dockerd"),
                "case {case:?}: expected the failing command to be named, got {install}"
            );
            assert_eq!(
                install["phaseStatus"].as_str(),
                Some("FAILED"),
                "case {case:?}: the phase that ran it must be the failing one, got {install}"
            );
        }
    }
}
