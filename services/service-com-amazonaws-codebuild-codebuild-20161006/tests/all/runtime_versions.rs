// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the RUNTIMES a build installs, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: a buildspec's `phases.install.runtime-versions` selects which version
//! of a runtime the build installs, and what it may select is what the build environment
//! has — so a runtime the environment has none of, or a version of one it does not have,
//! fails the build in the phase that prepares it from its specification rather than running
//! anything, while a selection the environment can satisfy installs that runtime and its
//! programs then report the version that was installed to the build's own commands. A
//! runtime is installed for the build that selected it alone, so a `buildspecOverride`
//! selecting another version changes that build and nothing else.
//!
//! Each row below differs along what that rule generalises over: which version of a runtime
//! is selected, which runtime, whether the environment has it, how a selection it cannot
//! satisfy fails, which program of an installed runtime is invoked and how.

use serde_json::{json, Value};

use super::project_build_lifecycle::{
    call, create_project, emulator, phase_of, settled_build,
};

/// A `CreateProject` source carrying `buildspec` inline, with no input source code of its
/// own — the form that fixes exactly what a build installs and runs.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose install phase selects `version` of `runtime` and whose commands are
/// `commands`, with every name in `exported` reported back by the build.
fn buildspec_selecting(
    runtime: &str,
    version: &str,
    commands: &[&str],
    exported: &[&str],
) -> String {
    let listed = commands
        .iter()
        .map(|command| format!("      - {command}\n"))
        .collect::<String>();
    let names = exported
        .iter()
        .map(|name| format!("    - {name}\n"))
        .collect::<String>();
    let env = if exported.is_empty() {
        String::new()
    } else {
        format!("env:\n  exported-variables:\n{names}")
    };
    format!(
        "version: 0.2\n{env}phases:\n  install:\n    runtime-versions:\n      \
         {runtime}: {version}\n    commands:\n{listed}"
    )
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

/// RULE: a selection the environment can satisfy installs that runtime, and the version its
/// programs report to the build's own commands is the one the buildspec selected.
///
/// The majors below are the `nodejs` versions a buildspec may select (source: AWS's
/// buildspec reference for `runtime-versions`, whose `nodejs` entry lists the versions a
/// current standard image carries). Each row reads the version back through the build's own
/// `$(node -v)`, so the assertion is about what the build observed rather than about the
/// selection it was given.
#[tokio::test]
async fn an_installed_runtime_reports_the_version_the_buildspec_selected() {
    let router = emulator();
    for (index, major) in ["18", "20", "22", "24"].iter().enumerate() {
        let project = format!("nodejs-proj-{index}");
        let buildspec = buildspec_selecting(
            "nodejs",
            major,
            &["export NODE_V=$(node -v)", "node -v", "npm -v"],
            &["NODE_V"],
        );
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("SUCCEEDED"),
            "case nodejs {major}: a build whose commands exited zero succeeds, got {build}"
        );
        let install = phase_of(&build, "INSTALL")
            .unwrap_or_else(|| panic!("case nodejs {major}: an INSTALL phase, got {build}"));
        assert_eq!(
            install["phaseStatus"], json!("SUCCEEDED"),
            "case nodejs {major}: the install phase ran its commands, got {install}"
        );
        let reported = exported_value(&build, "NODE_V").unwrap_or_default();
        assert!(
            reported.starts_with(&format!("v{major}.")),
            "case nodejs {major}: expected `node -v` to report that major, got {reported:?}"
        );
    }
}

/// RULE: the programs an installed runtime carries are that runtime's own, so which program
/// answers, which argument it answers to, and what it reports all follow from the runtime
/// and version the build installed — and `latest` installs the newest version of a runtime
/// the environment has.
#[tokio::test]
async fn each_installed_runtimes_programs_report_that_runtimes_version() {
    let router = emulator();
    // (case, runtime, the version selected, the program invoked, the argument that asks it
    // for its version, the prefix its answer carries)
    let rows = [
        (
            "node reports the nodejs version",
            "nodejs",
            "20",
            "node",
            "-v",
            "v20.",
        ),
        (
            "npm reports the npm that nodejs release bundles",
            "nodejs",
            "20",
            "npm",
            "--version",
            "10.",
        ),
        (
            // `python -V` is the version query; `-v` asks that program for a verbose trace.
            "python reports the python version",
            "python",
            "3.12",
            "python",
            "-V",
            "Python 3.12.",
        ),
        (
            "python3 reports the same installation",
            "python",
            "3.12",
            "python3",
            "--version",
            "Python 3.12.",
        ),
        (
            "the dotnet driver reports its SDK version",
            "dotnet",
            "8.0",
            "dotnet",
            "--version",
            "8.0.",
        ),
        (
            "latest installs the newest nodejs the environment has",
            "nodejs",
            "latest",
            "node",
            "-v",
            "v24.",
        ),
    ];
    for (index, (case, runtime, version, program, query, prefix)) in rows.iter().enumerate() {
        let project = format!("program-proj-{index}");
        let buildspec = buildspec_selecting(
            runtime,
            version,
            &[&format!("export REPORTED=$({program} {query})")],
            &["REPORTED"],
        );
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("SUCCEEDED"),
            "case {case:?}: the version query exits zero, got {build}"
        );
        let reported = exported_value(&build, "REPORTED").unwrap_or_default();
        assert!(
            reported.starts_with(prefix),
            "case {case:?}: expected {program} to report a version starting {prefix:?}, got \
             {reported:?}"
        );
    }
}

/// RULE: a version of a provided runtime the environment does not have is an installation
/// that could not be performed, so the build fails in the phase that prepares it — with the
/// command-execution status code and CodeBuild's sentence naming the version and the
/// runtime — and no phase of the buildspec runs.
#[tokio::test]
async fn a_version_the_environment_does_not_have_fails_the_build() {
    let router = emulator();
    // (case, runtime, the version selected) — one dropped version, one between two
    // provided ones, one far past them, one written as a word the reference does not
    // document, and an unprovided version of another runtime.
    let rows = [
        ("a nodejs major no image carries any more", "nodejs", "14"),
        ("a nodejs major between two provided ones", "nodejs", "19"),
        ("a nodejs major far past the provided ones", "nodejs", "99"),
        ("a version written as a word", "nodejs", "newest"),
        ("a python version no image carries", "python", "2.7"),
        ("a ruby version no image carries", "ruby", "2.5"),
    ];
    for (index, (case, runtime, version)) in rows.iter().enumerate() {
        let project = format!("unprovided-proj-{index}");
        let buildspec = buildspec_selecting(runtime, version, &["echo installed"], &[]);
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("FAILED"),
            "case {case:?}: a selection the environment cannot satisfy fails the build, got \
             {build}"
        );
        let download = phase_of(&build, "DOWNLOAD_SOURCE")
            .unwrap_or_else(|| panic!("case {case:?}: a DOWNLOAD_SOURCE phase, got {build}"));
        assert_eq!(
            download["phaseStatus"], json!("FAILED"),
            "case {case:?}: the preparing phase carries the failure, got {download}"
        );
        assert_eq!(
            download["contexts"][0]["statusCode"], json!("COMMAND_EXECUTION_ERROR"),
            "case {case:?}: expected the command-execution status code, got {download}"
        );
        assert_eq!(
            download["contexts"][0]["message"],
            json!(format!(
                "Unknown runtime version named '{version}' of {runtime}. This build image \
                 requires selecting at least one runtime version."
            )),
            "case {case:?}: unexpected failure message, got {download}"
        );
        assert!(
            phase_of(&build, "INSTALL").is_none(),
            "case {case:?}: no phase of the buildspec ran, got {build}"
        );
    }
}

/// RULE: a runtime the build environment has none of is the buildspec naming something that
/// is not a runtime of this image at all, which fails the build the way CodeBuild reports a
/// buildspec it could not use — naming what was selected and what the image does have.
#[tokio::test]
async fn a_runtime_the_environment_has_none_of_fails_the_build() {
    let router = emulator();
    // (case, the name the buildspec selects a version of)
    let rows = [
        ("a runtime name with the version glued to it", "nodejs16"),
        ("a runtime this image has none of", "cobol"),
        ("a provided runtime's name in the wrong case", "NodeJS"),
    ];
    for (index, (case, runtime)) in rows.iter().enumerate() {
        let project = format!("unknown-runtime-proj-{index}");
        let buildspec = buildspec_selecting(runtime, "20", &["echo installed"], &[]);
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("FAILED"),
            "case {case:?}: expected FAILED, got {build}"
        );
        let download = phase_of(&build, "DOWNLOAD_SOURCE")
            .unwrap_or_else(|| panic!("case {case:?}: a DOWNLOAD_SOURCE phase, got {build}"));
        assert_eq!(
            download["contexts"][0]["statusCode"], json!("YAML_FILE_ERROR"),
            "case {case:?}: expected CodeBuild's buildspec status code, got {download}"
        );
        let message = download["contexts"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.starts_with(&format!("Unknown runtime named '{runtime}'.")),
            "case {case:?}: expected the sentence to name what was selected, got {message:?}"
        );
        assert!(
            message.contains("nodejs"),
            "case {case:?}: expected the sentence to name the runtimes the image has, got \
             {message:?}"
        );
        assert!(
            phase_of(&build, "INSTALL").is_none(),
            "case {case:?}: no phase of the buildspec ran, got {build}"
        );
    }
}

/// RULE: a runtime is installed for the build that selected it alone — a `buildspecOverride`
/// installs its own selection for that build, leaving the project's buildspec and every
/// later build of it unchanged.
#[tokio::test]
async fn a_buildspec_override_installs_its_own_runtime_for_that_build_only() {
    let router = emulator();
    let project_spec = buildspec_selecting(
        "nodejs",
        "20",
        &["export NODE_V=$(node -v)"],
        &["NODE_V"],
    );
    create_project(&router, "override-proj", source_with(&project_spec)).await;

    // (case, the buildspecOverride this build is started with, the major it must observe)
    let rows = [
        ("another major, carried by the override", Some("24"), "24"),
        ("a third major, carried by the override", Some("22"), "22"),
        ("no override at all: the project's own", None, "20"),
    ];
    for (case, override_major, expected) in rows {
        let extra = match override_major {
            Some(major) => json!({
                "buildspecOverride": buildspec_selecting(
                    "nodejs",
                    major,
                    &["export NODE_V=$(node -v)"],
                    &["NODE_V"],
                ),
            }),
            None => json!({}),
        };
        let build = settled_build(&router, "override-proj", extra).await;
        assert_eq!(
            build["buildStatus"], json!("SUCCEEDED"),
            "case {case:?}: the build must succeed, got {build}"
        );
        let reported = exported_value(&build, "NODE_V").unwrap_or_default();
        assert!(
            reported.starts_with(&format!("v{expected}.")),
            "case {case:?}: expected the build to observe nodejs {expected}, got {reported:?}"
        );
    }

    let (status, read) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["override-proj"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetProjects: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["projects"][0]["source"]["buildspec"], json!(project_spec),
        "an override must leave the project's own buildspec exactly as it was, got {read}"
    );
}

/// RULE: what an installed runtime's program answers is the version that was installed;
/// every other invocation of it would have to RUN a program with that runtime, which this
/// build environment does not do, so it is answered the way a shell answers a command it did
/// not run — and a program of a runtime the build never installed is not there at all.
#[tokio::test]
async fn an_installed_runtimes_program_performs_only_its_version_query() {
    let router = emulator();
    // (case, the runtime-versions selection, the command the install phase runs, the status
    // the build answers with)
    let rows = [
        (
            "the version query of an installed runtime",
            Some(("nodejs", "20")),
            "node -v",
            "SUCCEEDED",
        ),
        (
            "an installed runtime asked to run a program",
            Some(("nodejs", "20")),
            "node index.js",
            "FAILED",
        ),
        (
            "an installed runtime's program with no argument at all",
            Some(("nodejs", "20")),
            "node",
            "FAILED",
        ),
        (
            // `-v` is `node`'s version query and `python`'s verbose trace, so the arguments
            // a program answers are that program's own.
            "a version query another program takes but this one does not",
            Some(("python", "3.12")),
            "python -v",
            "FAILED",
        ),
        (
            "an installed runtime's package manager asked to install",
            Some(("nodejs", "20")),
            "npm install",
            "FAILED",
        ),
        (
            "a runtime the environment installs but carries no program of",
            Some(("java", "corretto17")),
            "echo installed",
            "SUCCEEDED",
        ),
        (
            "that runtime's own program, whose version line this environment has nothing to \
             take from",
            Some(("java", "corretto17")),
            "java --version",
            "FAILED",
        ),
        (
            "a program of a runtime this build never installed",
            None,
            "node -v",
            "FAILED",
        ),
    ];
    for (index, (case, selection, command, expected)) in rows.iter().enumerate() {
        let project = format!("program-scope-proj-{index}");
        let buildspec = match selection {
            Some((runtime, version)) => buildspec_selecting(runtime, version, &[command], &[]),
            None => format!(
                "version: 0.2\nphases:\n  install:\n    commands:\n      - {command}\n"
            ),
        };
        create_project(&router, &project, source_with(&buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!(expected),
            "case {case:?}: expected buildStatus {expected}, got {build}"
        );
        if *expected == "FAILED" {
            let install = phase_of(&build, "INSTALL")
                .unwrap_or_else(|| panic!("case {case:?}: an INSTALL phase, got {build}"));
            assert_eq!(
                install["contexts"][0]["message"],
                json!(format!(
                    "Error while executing command: {command}. Reason: exit status 127"
                )),
                "case {case:?}: expected the status a shell answers for a command it did not \
                 run, got {install}"
            );
        }
    }
}
