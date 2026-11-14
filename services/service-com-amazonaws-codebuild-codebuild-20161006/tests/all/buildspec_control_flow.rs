// SPDX-License-Identifier: CC0-1.0

//! How a build's COMMANDS are performed, driven over the wire the router serves: `POST /`
//! with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: a buildspec's commands are the program of a build environment that has
//! a working tree of its own, so the build performs them as that environment's shell does —
//! the grammar's control flow (`if`/`elif`/`else`, `while`, `until`, `for`, a brace group)
//! around the commands inside it, arithmetic expansion of the values they compute, and the
//! file utilities over that tree — and a phase's `finally` block runs after its `commands`
//! block whatever that block answered, while the build's terminal status is still FAILED
//! when any command of either block failed.
//!
//! The rows differ along what the rule generalises over: which control-flow form the
//! commands are written with, whether its condition holds, how many times a loop runs, what
//! the arithmetic computes, whether the `finally` block's own commands succeed, and whether
//! the commands block ended the program with `exit`.
//!
//! What a build's tree holds is `buildspec_file_tree.rs`'s subject, the forms this
//! environment still does not perform are `shell_probe.rs`'s, and where a build's declared
//! artifact goes is covered where an object store to put it in is mounted
//! (`core/router/tests/all/codebuild_s3_build_output.rs`).

use serde_json::{json, Value};

use super::project_build_lifecycle::{
    call, create_project, emulator, start_build, PAST_THE_SETTLE_SPAN,
};

/// A `CreateProject` source carrying `buildspec` inline, with no input source code of its
/// own — the form that lets a suite fix exactly what a build runs.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose `build` phase declares `commands`, and whose `env/exported-variables`
/// report what those commands left behind.
fn buildspec_for(commands: &[&str], exported: &[&str]) -> String {
    let listed = commands.iter().map(|command| listed_command(command)).collect::<String>();
    let names = exported
        .iter()
        .map(|name| format!("    - {name}\n"))
        .collect::<String>();
    format!(
        "version: 0.2\nenv:\n  exported-variables:\n{names}phases:\n  build:\n    \
         commands:\n{listed}"
    )
}

/// One command line as an entry of a buildspec's command list: a double-quoted YAML scalar,
/// so a line carrying any of YAML's own indicators (a brace, a bracket, a colon) is still
/// the command the row wrote.
fn listed_command(command: &str) -> String {
    let quoted = command.replace('\\', "\\\\").replace('"', "\\\"");
    format!("      - \"{quoted}\"\n")
}

/// Start a build of `project`, let the transition the clock owes it fall due, and hand back
/// the settled record.
async fn settled_build(
    router: &smithy_server_runtime::MultiServiceRouter,
    project: &str,
) -> Value {
    let build = start_build(router, json!({"projectName": project})).await;
    let id = build["id"].as_str().expect("a build id").to_string();
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
    let (status, read) = call(router, "BatchGetBuilds", json!({"ids": [id.clone()]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds {id}: expected 200, got {status} with {read}"
    );
    read["builds"][0].clone()
}

/// The value the settled build reports for the exported variable `name`, or an empty string
/// when it reports none.
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

/// RULE: the control flow a build's commands are written with is performed over this
/// environment's own catalog, and what it computes is what the build exports.
#[tokio::test]
async fn a_builds_commands_are_performed_as_a_script_of_its_environment() {
    let router = emulator();
    /// One row: the case, the commands, the names exported, the build's status, and the
    /// value expected for the first exported name.
    type Row = (&'static str, Vec<&'static str>, &'static str, &'static str);

    let rows: Vec<Row> = vec![
        (
            "an if whose condition holds",
            vec!["if true; then export STATE=then; else export STATE=else; fi"],
            "SUCCEEDED",
            "then",
        ),
        (
            "an if whose condition does not hold",
            vec!["if false; then export STATE=then; else export STATE=else; fi"],
            "SUCCEEDED",
            "else",
        ),
        (
            "an elif branch",
            vec![
                "if false; then export STATE=then; elif true; then export STATE=elif; else \
                 export STATE=else; fi",
            ],
            "SUCCEEDED",
            "elif",
        ),
        (
            "an if with no branch to run",
            vec!["export STATE=untouched", "if false; then export STATE=then; fi"],
            "SUCCEEDED",
            "untouched",
        ),
        (
            "an if asking about the build's own tree",
            vec![
                "echo built > marker.txt",
                "if [ -f marker.txt ]; then export STATE=present; else export STATE=absent; fi",
            ],
            "SUCCEEDED",
            "present",
        ),
        (
            "an if asking about a path the build never wrote",
            vec!["if [ -f marker.txt ]; then export STATE=present; else export STATE=absent; fi"],
            "SUCCEEDED",
            "absent",
        ),
        (
            "a while loop counting with arithmetic",
            vec![
                "export STATE=0",
                "while [ $STATE -lt 3 ]; do export STATE=\"$(( STATE + 1 ))\"; done",
            ],
            "SUCCEEDED",
            "3",
        ),
        (
            "an until loop",
            vec![
                "export STATE=0",
                "until [ $STATE -ge 2 ]; do export STATE=\"$(( STATE + 1 ))\"; done",
            ],
            "SUCCEEDED",
            "2",
        ),
        (
            "a for loop over the words it names",
            vec!["for WORD in a b c; do export STATE=\"$WORD\"; done"],
            "SUCCEEDED",
            "c",
        ),
        (
            "a brace group",
            vec!["{ export STATE=grouped; }"],
            "SUCCEEDED",
            "grouped",
        ),
        (
            "arithmetic the commands compute",
            vec!["export SEED=6", "export STATE=\"$(( SEED * 7 + 3 ))\""],
            "SUCCEEDED",
            "45",
        ),
        (
            "arithmetic inside a redirected command, read back",
            vec![
                "export SEED=4",
                "printf '%s' \"$(( SEED * 2 ))\" > computed.txt",
                "export STATE=\"$(cat computed.txt)\"",
            ],
            "SUCCEEDED",
            "8",
        ),
        (
            "a command inside an if that fails, which fails the phase",
            vec!["if true; then export STATE=entered; false; fi"],
            "FAILED",
            "entered",
        ),
    ];

    for (index, (case, commands, expected_status, expected_state)) in rows.iter().enumerate() {
        let project = format!("flow-proj-{index}");
        let buildspec = buildspec_for(commands, &["STATE"]);
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project).await;
        assert_eq!(
            build["buildStatus"],
            json!(expected_status),
            "case {case:?}: expected buildStatus {expected_status}, got {build}"
        );
        let state = exported(&build, "STATE");
        assert_eq!(
            state, *expected_state,
            "case {case:?}: expected the exported STATE {expected_state:?}, got {state:?}"
        );
    }
}

/// RULE: a phase's `finally` block runs after its `commands` block whatever that block
/// answered — an `exit` that ended it included — and the build is FAILED when a command of
/// either block failed.
#[tokio::test]
async fn a_phases_finally_block_runs_after_its_commands() {
    let router = emulator();
    /// One row: the case, the `commands` block, the `finally` block, the build's status, and
    /// the value the finally block left in STATE.
    type Row = (
        &'static str,
        Vec<&'static str>,
        Vec<&'static str>,
        &'static str,
        &'static str,
    );

    let rows: Vec<Row> = vec![
        (
            "after commands that succeeded",
            vec!["export STATE=commands"],
            vec!["export STATE=\"${STATE}-finally\""],
            "SUCCEEDED",
            "commands-finally",
        ),
        (
            "after a command that failed",
            vec!["export STATE=commands", "false", "export STATE=unreached"],
            vec!["export STATE=\"${STATE}-finally\""],
            "FAILED",
            "commands-finally",
        ),
        (
            "after a command that ended the program with a non-zero exit",
            vec!["export STATE=commands", "exit 7"],
            vec!["export STATE=\"${STATE}-finally\""],
            "FAILED",
            "commands-finally",
        ),
        (
            "after a command that ended the program with a zero exit",
            vec!["export STATE=commands", "exit 0"],
            vec!["export STATE=\"${STATE}-finally\""],
            "SUCCEEDED",
            "commands-finally",
        ),
        (
            "a finally block whose own command fails",
            vec!["export STATE=commands"],
            vec!["export STATE=\"${STATE}-finally\"", "false"],
            "FAILED",
            "commands-finally",
        ),
        (
            "a finally block that appends to a file the commands wrote",
            vec!["echo first > out.txt", "false"],
            vec![
                "echo second >> out.txt",
                "export STATE=\"$(cat out.txt)\"",
            ],
            "FAILED",
            "first\nsecond",
        ),
    ];

    for (index, (case, commands, finally, expected_status, expected_state)) in
        rows.iter().enumerate()
    {
        let project = format!("finally-proj-{index}");
        let listed = commands.iter().map(|command| listed_command(command)).collect::<String>();
        let after = finally.iter().map(|command| listed_command(command)).collect::<String>();
        let buildspec = format!(
            "version: 0.2\nenv:\n  exported-variables:\n    - STATE\nphases:\n  build:\n    \
             commands:\n{listed}    finally:\n{after}"
        );
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project).await;
        assert_eq!(
            build["buildStatus"],
            json!(expected_status),
            "case {case:?}: expected buildStatus {expected_status}, got {build}"
        );
        let state = exported(&build, "STATE");
        assert_eq!(
            state, *expected_state,
            "case {case:?}: expected the exported STATE {expected_state:?}, got {state:?}"
        );
        // Whatever the commands answered, the build did its own finishing work: a build
        // that ran its commands reports the phase that puts its output where its
        // artifacts say.
        let phases: Vec<String> = build["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|phase| phase["phaseType"].as_str().map(str::to_string))
            .collect();
        assert!(
            phases.contains(&"UPLOAD_ARTIFACTS".to_string())
                && phases.contains(&"FINALIZING".to_string()),
            "case {case:?}: expected the finishing phases to be reported, got {phases:?}"
        );
    }
}
