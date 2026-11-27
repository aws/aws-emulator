// SPDX-License-Identifier: CC0-1.0

//! The build LOG as the agent's own account of the run, read back through CloudWatch Logs.
//!
//! RULE under test: a build's log is what the build AGENT wrote while running the buildspec,
//! so before each command it runs the agent writes the line naming that command — the command
//! LINE as the buildspec wrote it, unexpanded — and the lines that command itself wrote on
//! either of its streams follow it. That is what makes the log the documented way to read why
//! a build ended as it did: a client sees which command produced which output, and sees the
//! command a build ended on even when that command wrote nothing at all.
//!
//! Two services have to agree for that to hold — CloudWatch Logs holds the events, IAM holds
//! the role whose grants let the build deliver them — so this suite drives the harness
//! `build_log_delivery.rs` already mounts those two beside CodeBuild with, rather than a
//! second copy of it.
//!
//! The rows differ along what the rule generalises over: how many commands the phase runs,
//! whether a command writes anything at all, whether its line carries a variable reference
//! the shell expands, and whether the command succeeds or is one the environment has no
//! program for.

use serde_json::json;

use super::build_log_delivery::{
    buildspec, codebuild, delivered_messages, emulator, project_body, role_stating, settled_build,
    LOG_ACTIONS,
};
use super::project_build_lifecycle::REGION;

/// What the agent writes in front of each command it runs, as this service words it.
const RUNNING_COMMAND: &str = "Running command ";

/// One row: the case, the commands the build's phase runs, the status the build settles at,
/// and the lines its log must carry IN THAT ORDER.
struct Row {
    case: &'static str,
    commands: &'static [&'static str],
    status: &'static str,
    expected: Vec<String>,
}

/// The line the agent writes for `command`.
fn running(command: &str) -> String {
    format!("{RUNNING_COMMAND}{command}")
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one command that writes a line",
            commands: &["echo first"],
            status: "SUCCEEDED",
            expected: vec![running("echo first"), "first".to_string()],
        },
        Row {
            case: "two commands, each followed by what it wrote",
            commands: &["echo one", "echo two"],
            status: "SUCCEEDED",
            expected: vec![
                running("echo one"),
                "one".to_string(),
                running("echo two"),
                "two".to_string(),
            ],
        },
        Row {
            case: "a command that writes nothing at all",
            commands: &["true", "echo after"],
            status: "SUCCEEDED",
            expected: vec![
                running("true"),
                running("echo after"),
                "after".to_string(),
            ],
        },
        Row {
            // The line the agent names is the one the BUILDSPEC wrote, so the variable
            // reference is in the log as written and its VALUE is in the output below it.
            case: "a command whose line carries a variable the shell expands",
            commands: &["echo $AWS_REGION"],
            status: "SUCCEEDED",
            expected: vec![running("echo $AWS_REGION"), REGION.to_string()],
        },
        Row {
            case: "the command a build ends on, which the environment has no program for",
            commands: &["pdq-not-a-program", "echo never"],
            status: "FAILED",
            expected: vec![running("pdq-not-a-program")],
        },
    ]
}

/// RULE: see the module header.
#[tokio::test]
async fn a_builds_log_names_each_command_before_what_that_command_wrote() {
    let router = emulator();
    let role = role_stating(
        &router,
        "codebuild-agent-log-role",
        json!([{"Effect": "Allow", "Action": LOG_ACTIONS, "Resource": "*"}]),
    )
    .await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let project = format!("agent-log-proj-{index}");
        codebuild(
            &router,
            case,
            "CreateProject",
            &project_body(&project, &role, &buildspec(row.commands), json!({})),
        )
        .await;
        let build = settled_build(&router, case, &project).await;
        assert_eq!(
            build["buildStatus"],
            json!(row.status),
            "case {case:?}: expected the build to settle {}, got {build}",
            row.status
        );
        let group = build["logs"]["groupName"].as_str().unwrap_or_default();
        let stream = build["logs"]["streamName"].as_str().unwrap_or_default();
        let messages = delivered_messages(&router, group, stream)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: CloudWatch Logs must hold {group}/{stream}, got {build}")
            });

        // Every line the row names is in the log, and in the order the run produced it.
        let mut reached = 0usize;
        for line in &row.expected {
            let at = messages
                .iter()
                .skip(reached)
                .position(|message| message == line)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected {line:?} among the events of {group}/{stream} \
                         after the {reached} line(s) before it, got {messages:?}"
                    )
                });
            reached += at + 1;
        }

        // A command the build never reached is not in the log: the agent names the commands it
        // RAN, and a phase stops at its first failing command.
        if row.status == "FAILED" {
            let never = running("echo never");
            assert!(
                !messages.contains(&never),
                "case {case:?}: no line may name a command the build never ran, got {messages:?}"
            );
        }
    }
}

/// RULE: the agent's account covers EVERY phase the build ran, so a command of the install
/// phase is named in the log beside one of the build phase — and the phase lines the agent
/// writes are there too.
#[tokio::test]
async fn the_log_names_the_commands_of_every_phase_the_build_ran() {
    let router = emulator();
    let role = role_stating(
        &router,
        "codebuild-agent-phases-role",
        json!([{"Effect": "Allow", "Action": LOG_ACTIONS, "Resource": "*"}]),
    )
    .await;
    let case = "a command in each of three phases";
    let spec = concat!(
        "version: 0.2\n",
        "phases:\n",
        "  install:\n",
        "    commands:\n",
        "      - 'echo installing'\n",
        "  pre_build:\n",
        "    commands:\n",
        "      - 'echo preparing'\n",
        "  build:\n",
        "    commands:\n",
        "      - 'echo building'\n",
    );
    codebuild(
        &router,
        case,
        "CreateProject",
        &project_body("agent-log-phases", &role, spec, json!({})),
    )
    .await;
    let build = settled_build(&router, case, "agent-log-phases").await;
    assert_eq!(
        build["buildStatus"],
        json!("SUCCEEDED"),
        "case {case:?}: every command runs, so the build succeeds, got {build}"
    );
    let group = build["logs"]["groupName"].as_str().unwrap_or_default();
    let stream = build["logs"]["streamName"].as_str().unwrap_or_default();
    let messages = delivered_messages(&router, group, stream)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: CloudWatch Logs must hold {group}/{stream}"));
    for command in ["echo installing", "echo preparing", "echo building"] {
        assert!(
            messages.contains(&running(command)),
            "case {case:?}: expected {:?} among the events, got {messages:?}",
            running(command)
        );
    }
    // The phase lines the agent writes about what it ran are in the same log.
    assert!(
        messages
            .iter()
            .any(|message| message == "Phase complete: BUILD State: SUCCEEDED"),
        "case {case:?}: expected the agent's phase line among the events, got {messages:?}"
    );
}
