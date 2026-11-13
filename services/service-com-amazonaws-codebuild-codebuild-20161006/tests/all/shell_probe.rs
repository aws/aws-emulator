// SPDX-License-Identifier: CC0-1.0

//! What the build environment's shell answers for each COMMAND — the probe of which
//! commands it provides and which it has no program for, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: the build environment runs IN THIS PROCESS, so the commands it
//! provides are the ones whose whole effect is inside the build's own session, plus the
//! programs the runtimes the build installed provide, and every other command is answered
//! the way a shell answers a command it has no program for — status 127 — which fails the
//! phase that ran it rather than being reported as a command that succeeded. Nothing a
//! command does here starts a process or reaches a host.
//!
//! The catalog is the one `crate::state`'s build shell declares (`BUILT_IN_COMMANDS`);
//! each row below names a command of it or a program outside it, and asserts the status
//! the phase context reports. What an INSTALLED runtime's programs answer is the subject of
//! `runtime_versions`.

use serde_json::{json, Value};

use super::project_build_lifecycle::{call, create_project, emulator, start_build, PAST_THE_SETTLE_SPAN};

/// The failure context CodeBuild reports for a command that exited non-zero.
fn command_failure(command: &str, status: i32) -> Value {
    json!(format!(
        "Error while executing command: {command}. Reason: exit status {status}"
    ))
}

/// RULE: a command FORM this environment does not perform is not performed at all — it
/// answers the way a shell answers a command it cannot run, which fails the phase that
/// ran it. Nothing here reaches a process, a socket or the host's environment, and no file
/// of the HOST either: the files a build writes are its own session's
/// (`buildspec_file_tree.rs` covers those), so a redirection that READS a path is still a
/// form this environment does not perform.
///
/// One row per containment the shell states: a compound command, a function definition, an
/// input redirection, a redirection of another descriptor, an arithmetic expansion, a tilde
/// prefix and an appending assignment. (A command SUBSTITUTION is performed — in this
/// session, over the same catalog — and what it answers is `command_substitution.rs`'s
/// subject.)
#[tokio::test]
async fn a_command_form_this_environment_does_not_perform_is_refused() {
    let router = emulator();
    // (case, the command line the build phase runs)
    let rows = [
        ("a compound command", "if true; then echo yes; fi"),
        ("a loop", "while true; do echo again; done"),
        ("a function definition", "run() { echo hi; }"),
        ("a redirection reading a file", "echo marker < /etc/hostname"),
        ("a redirection of another descriptor", "echo marker 2> errors.txt"),
        ("an arithmetic expansion", "echo $((1 + 1))"),
        ("a tilde prefix", "echo ~/build"),
        ("an appending assignment", "PATH+=/opt/bin"),
        ("a subshell", "(echo inside)"),
    ];

    for (index, (case, command)) in rows.iter().enumerate() {
        let project = format!("refused-proj-{index}");
        let buildspec =
            format!("version: 0.2\nphases:\n  build:\n    commands:\n      - '{command}'\n");
        create_project(
            &router,
            &project,
            json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}}),
        )
        .await;
        let build = start_build(&router, json!({"projectName": project})).await;
        let id = build["id"].as_str().expect("a build id").to_string();
        tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        let (_, read) = call(&router, "BatchGetBuilds", json!({"ids": [id]})).await;
        let settled = read["builds"][0].clone();
        assert_eq!(
            settled["buildStatus"], json!("FAILED"),
            "case {case:?}: a command form this environment does not perform fails the \
             build, got {settled}"
        );
        let build_phase = settled["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|phase| phase["phaseType"] == json!("BUILD"))
            .unwrap_or_else(|| panic!("case {case:?}: a BUILD phase, got {settled}"));
        assert_eq!(
            build_phase["contexts"][0]["message"],
            command_failure(command, 127),
            "case {case:?}: expected the status a shell answers for a command it cannot \
             run, got {build_phase}"
        );
    }
}

/// RULE: the bound on what one command line can cost the grammar is a bound on the line,
/// not on how its words happen to be spelled — a reserved word glued to a separator nests
/// the grammar exactly as a spaced one does, so it counts the same.
#[tokio::test]
async fn a_separator_glued_nesting_chain_is_refused_before_it_is_parsed() {
    let router = emulator();
    // The same chain in both spellings: one spaced, one glued to the `;` separator. Both
    // open far more nested commands than one line may, so neither reaches the grammar.
    let rows = [
        ("spaced", "if true ; then ".repeat(600)),
        ("glued to the separator", ":;if ".repeat(600)),
    ];
    for (index, (case, prefix)) in rows.iter().enumerate() {
        let project = format!("glued-proj-{index}");
        let buildspec = format!(
            "version: 0.2\nphases:\n  build:\n    commands:\n      - '{prefix}true'\n"
        );
        create_project(
            &router,
            &project,
            json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}}),
        )
        .await;
        let build = start_build(&router, json!({"projectName": project})).await;
        let id = build["id"].as_str().expect("a build id").to_string();
        tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        let (_, read) = call(&router, "BatchGetBuilds", json!({"ids": [id]})).await;
        let settled = read["builds"][0].clone();
        assert_eq!(
            settled["buildStatus"], json!("FAILED"),
            "case {case:?}: a line past the nesting bound is not run, got {settled}"
        );
        assert_eq!(
            settled["buildComplete"], json!(true),
            "case {case:?}: the build must still settle, got {settled}"
        );
    }
}

/// RULE: each command the build environment provides answers its own status, and a
/// command it has no program for answers 127.
#[tokio::test]
async fn each_command_answers_the_status_the_shell_answers_for_it() {
    let router = emulator();
    // (case, the command line the build phase runs, the status the shell answers)
    let rows = [
        ("echo, which writes its arguments", "echo hello world", 0),
        ("the true utility", "true", 0),
        ("the null utility", ":", 0),
        ("the false utility", "false", 1),
        ("exit with a status of its own", "exit 3", 3),
        ("exit with no argument", "exit", 0),
        ("export setting a variable", "export BUILT=yes", 0),
        ("unset removing one", "unset BUILT", 0),
        ("an assignment on its own", "BUILT=yes", 0),
        ("a container runtime this environment has none for", "docker build .", 127),
        ("a command-line tool it has none for", "aws ecr get-login-password", 127),
        ("a coreutil it has none for", "ls -la", 127),
        ("a working-tree command it has none for", "cd /codebuild/output", 127),
        ("a command whose name an unset parameter expands to", "$MISSING_TOOL --version", 127),
    ];

    for (index, (case, command, status)) in rows.iter().enumerate() {
        let project = format!("shell-proj-{index}");
        let buildspec =
            format!("version: 0.2\nphases:\n  build:\n    commands:\n      - {command}\n");
        create_project(
            &router,
            &project,
            json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}}),
        )
        .await;
        let build = start_build(&router, json!({"projectName": project})).await;
        let id = build["id"].as_str().expect("a build id").to_string();
        tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        let (_, read) = call(&router, "BatchGetBuilds", json!({"ids": [id]})).await;
        let settled = read["builds"][0].clone();
        let expected_status = if *status == 0 { "SUCCEEDED" } else { "FAILED" };
        assert_eq!(
            settled["buildStatus"], json!(expected_status),
            "case {case:?}: expected buildStatus {expected_status}, got {settled}"
        );
        let build_phase = settled["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|phase| phase["phaseType"] == json!("BUILD"))
            .unwrap_or_else(|| panic!("case {case:?}: a BUILD phase, got {settled}"));
        if *status == 0 {
            assert_eq!(
                build_phase["phaseStatus"], json!("SUCCEEDED"),
                "case {case:?}: the phase its command succeeded in succeeds, got {build_phase}"
            );
        } else {
            assert_eq!(
                build_phase["contexts"][0]["message"],
                command_failure(command, *status),
                "case {case:?}: expected the command and its status, got {build_phase}"
            );
        }
    }
}
