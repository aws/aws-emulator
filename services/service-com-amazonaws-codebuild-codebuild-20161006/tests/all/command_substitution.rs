// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a COMMAND SUBSTITUTION does in a build, driven over the wire
//! the router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! RULE under test: `$(…)` is a command whose OUTPUT is a word of another command, so the
//! build performs it — in its own session, over exactly the commands that session performs —
//! and substitutes what it wrote with its trailing newlines removed. It is a subshell: what
//! it sets, and an `exit` it performs, do not reach the session that substituted it. The
//! status of the command carrying it is that command's own, except for a command with no
//! command name at all, whose status is the substitution's.
//!
//! Each row below differs along what that rule generalises over: whether the substituted
//! command is one the environment performs, how it is spelled, whether it sets anything,
//! and whether the command carrying it has a name of its own.

use serde_json::{json, Value};

use super::project_build_lifecycle::{create_project, emulator, settled_build};

/// A `CreateProject` source carrying `buildspec` inline.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose build phase runs `commands`, with every name in `exported` reported
/// back by the build.
fn buildspec_running(commands: &[&str], exported: &[&str]) -> String {
    let listed = commands
        .iter()
        .map(|command| format!("      - {command}\n"))
        .collect::<String>();
    let env = if exported.is_empty() {
        String::new()
    } else {
        format!(
            "env:\n  exported-variables:\n{}",
            exported
                .iter()
                .map(|name| format!("    - {name}\n"))
                .collect::<String>()
        )
    };
    format!("version: 0.2\n{env}phases:\n  build:\n    commands:\n{listed}")
}

/// The value a settled build reports for the exported variable `name`.
fn exported_value(build: &Value, name: &str) -> Option<String> {
    build["exportedEnvironmentVariables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|variable| variable["name"] == json!(name))
        .and_then(|variable| variable["value"].as_str().map(str::to_string))
}

/// RULE: the substituted command is performed by the build's own session, so the commands it
/// may name are the ones that session performs — and the status the line answers is the
/// status of the command that CARRIES the substitution, except where that command has no
/// name of its own.
#[tokio::test]
async fn a_substituted_command_is_held_to_what_the_session_performs() {
    let router = emulator();
    // (case, the command line the build phase runs, the build's status)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a substituted utility the session performs",
            "echo got-$(echo one)".to_string(),
            "SUCCEEDED",
        ),
        (
            "the same substitution written with backquotes",
            "echo got-`echo one`".to_string(),
            "SUCCEEDED",
        ),
        (
            // POSIX: the status of a command substitution is not the status of the command
            // carrying it, so `echo` still answers for itself.
            "a substituted program the environment has none for",
            "echo got-$(date)".to_string(),
            "SUCCEEDED",
        ),
        (
            // "If there is no command name … the exit status shall be that of the last
            // command substitution performed."
            "an assignment whose only substituted command succeeded",
            "SUBSTITUTED=$(true)".to_string(),
            "SUCCEEDED",
        ),
        (
            "an assignment whose only substituted command answered failure",
            "SUBSTITUTED=$(false)".to_string(),
            "FAILED",
        ),
        (
            "an assignment whose substituted command the environment has none for",
            "SUBSTITUTED=$(date)".to_string(),
            "FAILED",
        ),
        (
            "a word nesting more substitutions than one word may",
            // Past the bound the word has no expansion at all, so the command carrying it
            // is not run: the status a shell answers for a command it could not run, which
            // this line reads back as the status of a command with no name of its own.
            format!("SUBSTITUTED={}true{}", "$(".repeat(12), ")".repeat(12)),
            "FAILED",
        ),
    ];

    for (index, (case, command, expected)) in rows.iter().enumerate() {
        let project = format!("substitution-proj-{index}");
        create_project(
            &router,
            &project,
            source_with(&buildspec_running(&[command], &[])),
        )
        .await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!(expected),
            "case {case:?}: expected buildStatus {expected}, got {build}"
        );
    }
}

/// RULE: what the substituted command WROTE is the word, with its trailing newlines
/// removed — and the substitution is a subshell, so the variables it sets and an `exit` it
/// performs do not reach the session that substituted it.
#[tokio::test]
async fn what_a_substituted_command_wrote_is_the_word() {
    let router = emulator();
    // (case, the commands the build phase runs, the value the build must report for
    // SUBSTITUTED)
    let rows: [(&str, &[&str], &str); 5] = [
        (
            "one line of output",
            &["export SUBSTITUTED=$(echo one)"],
            "one",
        ),
        (
            "two lines of output, which the word keeps both of",
            &["export SUBSTITUTED=\"$(echo one; echo two)\""],
            "one\ntwo",
        ),
        (
            "a substitution nested inside a substitution",
            &["export SUBSTITUTED=$(echo $(echo deep))"],
            "deep",
        ),
        (
            "a substituted command that wrote nothing at all",
            &["export SUBSTITUTED=$(true)"],
            "",
        ),
        (
            // The substituted command sets a variable and ends its own session; neither
            // reaches the session that substituted it, so what the build reports is what
            // the line before it set.
            "a substitution that sets a variable and exits",
            &[
                "export SUBSTITUTED=kept",
                "IGNORED=$(export SUBSTITUTED=changed; exit 0)",
                "echo after the substitution",
            ],
            "kept",
        ),
    ];

    for (index, (case, commands, expected)) in rows.iter().enumerate() {
        let project = format!("substituted-value-proj-{index}");
        create_project(
            &router,
            &project,
            source_with(&buildspec_running(commands, &["SUBSTITUTED"])),
        )
        .await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("SUCCEEDED"),
            "case {case:?}: every command of the row exits zero, got {build}"
        );
        assert_eq!(
            exported_value(&build, "SUBSTITUTED").as_deref(),
            Some(*expected),
            "case {case:?}: expected the build to report {expected:?}, got {build}"
        );
    }
}
