// SPDX-License-Identifier: CC0-1.0

//! The buildspec: the phases a build runs, and what running them answered.
//!
//! # The rule this module implements
//!
//! A build RUNS the commands its buildspec declares, phase by phase, and its terminal
//! `buildStatus` is what those commands answered: `SUCCEEDED` when every command of
//! every phase exited zero, and `FAILED` when one did not. `FAULT` is CodeBuild's
//! build-SYSTEM failure, so it is not what a build whose own commands failed reports.
//! Each phase's `phaseStatus` is the same answer for that phase's commands, and a phase
//! whose failure aborts the build is the last phase the build reports — the phases after
//! it never ran, so none is reported. A phase whose `on-failure` is `CONTINUE` does not
//! abort, however many phases have already failed.
//!
//! The buildspec is YAML, read through the one bounded parse this crate reads a
//! buildspec with ([`super::batchspec::buildspec_document`]), and its command lines are
//! run by the build environment's shell ([`super::build_shell`]) — which performs, beside
//! its own utilities, the programs of the runtimes the install phase's `runtime-versions`
//! selected ([`super::runtime_versions`]). A build whose buildspec cannot be read has no
//! phase to run: the source-download phase fails, and what it reports is what the reading
//! answered — the YAML parser's own diagnostic for a buildspec that is not YAML, and
//! CodeBuild's `YAML_FILE_ERROR` classification for a buildspec there was none of to parse.
//! A build whose runtime selection the environment cannot satisfy has none either, and that
//! same phase reports what CodeBuild states about the selection
//! ([`runtime_install_context`]).

use std::collections::BTreeMap;

use super::batchspec::{buildspec_document, mapping_get, scalar_string, UnreadableBuildspec};
use super::build_shell::BuildShell;
use super::runtime_versions::{install_runtimes, RuntimeInstallFailure};
use super::{finished_phase, open_phase};
use crate::ops::types::{Build, BuildPhase, ExportedEnvironmentVariable, PhaseContext};

/// `StatusType` values a build and its phases report.
pub const BUILD_STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// See [`BUILD_STATUS_SUCCEEDED`].
pub const BUILD_STATUS_FAILED: &str = "FAILED";

/// `BuildPhaseType` values, in the order a build reaches them.
const PHASE_SUBMITTED: &str = "SUBMITTED";
/// See [`PHASE_SUBMITTED`].
const PHASE_QUEUED: &str = "QUEUED";
/// See [`PHASE_SUBMITTED`].
const PHASE_PROVISIONING: &str = "PROVISIONING";
/// See [`PHASE_SUBMITTED`].
const PHASE_DOWNLOAD_SOURCE: &str = "DOWNLOAD_SOURCE";
/// See [`PHASE_SUBMITTED`].
const PHASE_UPLOAD_ARTIFACTS: &str = "UPLOAD_ARTIFACTS";
/// See [`PHASE_SUBMITTED`].
const PHASE_FINALIZING: &str = "FINALIZING";
/// See [`PHASE_SUBMITTED`].
const PHASE_COMPLETED: &str = "COMPLETED";

/// The phases a buildspec declares commands for, as the `BuildPhaseType` each is
/// reported by paired with the buildspec key that declares it, in the order CodeBuild
/// runs them.
///
/// From the buildspec reference's `phases` sequence: `install`, `pre_build`, `build` and
/// `post_build` are the phases whose `commands` the build runs.
const COMMAND_PHASES: [(&str, &str); 4] = [
    ("install", "INSTALL"),
    ("pre_build", "PRE_BUILD"),
    ("build", "BUILD"),
    ("post_build", "POST_BUILD"),
];

/// The `PhaseContext$statusCode` CodeBuild reports when the buildspec itself could not
/// be read, and the two readings that carry it.
const YAML_FILE_ERROR: &str = "YAML_FILE_ERROR";

/// What CodeBuild states when the build has no buildspec to run at all: the project
/// carries none inline, so the specification would have to come from a source tree, and
/// this build environment fetches none.
const NO_YAML_FILE_MESSAGE: &str = "YAML file does not exist";

/// What CodeBuild states when the buildspec is there, parses, and is still not the
/// document a build runs — it holds no mapping, or it is outside the bounds this crate
/// reads a buildspec in. A document the PARSER refused is not this: the parser's own
/// diagnostic is what the phase reports for that, unclassified.
const UNREADABLE_YAML_MESSAGE: &str = "Unable to parse YAML file";

/// The `PhaseContext$statusCode` a phase whose command failed reports.
const COMMAND_EXECUTION_ERROR: &str = "COMMAND_EXECUTION_ERROR";

/// The `PhaseContext$statusCode` CodeBuild reports when the fault is the build's own
/// declaration rather than a command's — which is how it reports an artifact upload that
/// found nothing to upload.
const CLIENT_ERROR: &str = "CLIENT_ERROR";

/// What CodeBuild states when a build whose artifacts have a destination runs a buildspec
/// that names no artifact file to put there.
const NO_ARTIFACT_FILES_MESSAGE: &str = "No artifact files specified";

/// What CodeBuild states when the artifact paths a buildspec names match nothing to
/// upload at the build's destination.
const NO_MATCHING_ARTIFACT_PATHS_MESSAGE: &str = "No matching artifact paths found";

/// What CodeBuild states as the reason a phase's command failed: the command it ran and
/// the status that command exited with.
fn command_failure_message(command: &str, status: i32) -> String {
    format!("Error while executing command: {command}. Reason: exit status {status}")
}

/// The buildspec's `env/variables` key: "variables … you want to make available".
const ENV_KEY: &str = "env";
/// See [`ENV_KEY`].
const VARIABLES_KEY: &str = "variables";
/// See [`ENV_KEY`]: "the variables whose values you want to export".
const EXPORTED_VARIABLES_KEY: &str = "exported-variables";

/// The buildspec's `phases` key, and the members of a phase this reading uses.
const PHASES_KEY: &str = "phases";
/// See [`PHASES_KEY`].
const COMMANDS_KEY: &str = "commands";
/// See [`PHASES_KEY`]: the `install` phase's "runtime-versions … specify the runtime and
/// version".
const RUNTIME_VERSIONS_KEY: &str = "runtime-versions";
/// The buildspec key of the phase `runtime-versions` is declared in.
const INSTALL_PHASE_KEY: &str = "install";
/// See [`PHASES_KEY`]: "on-failure … ABORT … CONTINUE".
const ON_FAILURE_KEY: &str = "on-failure";
/// The buildspec's `artifacts` key, whose `files` are "the locations … that contain the
/// build output artifacts".
const ARTIFACTS_KEY: &str = "artifacts";
/// The `on-failure` value that lets the build go on to the next phase; the other,
/// `ABORT`, is the default and ends the build.
const ON_FAILURE_CONTINUE: &str = "CONTINUE";

/// The `EnvironmentVariable$type` whose value IS the value the build sees. The other
/// two name a parameter or a secret the build would resolve through another service,
/// which this reading does not do, so a variable of those types sets nothing here.
const PLAINTEXT_VARIABLE_TYPE: &str = "PLAINTEXT";

/// A buildspec as the parts of it a build runs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Buildspec {
    /// The `env/variables` the buildspec declares, in declaration order.
    pub variables: Vec<(String, String)>,
    /// The names in `env/exported-variables`, whose values the build reports once it
    /// has run.
    pub exported_variables: Vec<String>,
    /// The runtimes the install phase's `runtime-versions` selects, as the runtime and the
    /// version each entry names, in declaration order.
    pub runtime_versions: Vec<(String, String)>,
    /// The phases that declare commands, in the order they run.
    pub phases: Vec<DeclaredPhase>,
    /// Whether the buildspec declares an `artifacts` section at all, which is what says
    /// WHICH files the build's artifacts are.
    pub declares_artifacts: bool,
}

/// One phase of a buildspec: the `BuildPhaseType` it is reported as, the command lines
/// it declares, and whether a failure of one of them ends the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredPhase {
    /// The modelled `BuildPhaseType` this phase is reported as.
    pub phase_type: &'static str,
    /// The command lines this phase declares, in declaration order.
    pub commands: Vec<String>,
    /// Whether a failing command ends the build — `on-failure: ABORT`, the default,
    /// against `CONTINUE`, which goes on to the next phase.
    pub abort_on_failure: bool,
}

/// Read `buildspec` as the phases a build runs, or WHY it could not be read: what the
/// YAML parser answered about a text that is not YAML, and [`UnreadableBuildspec::NotRead`]
/// for one this crate does not read inside the bounds it parses a buildspec in.
///
/// A readable buildspec that declares no `phases`, or a phase that declares no
/// `commands`, declares no command: the build then has nothing to run, which is a build
/// that runs to success rather than one that failed.
pub fn read_buildspec(buildspec: &str) -> Result<Buildspec, UnreadableBuildspec> {
    let document = buildspec_document(buildspec)?;
    let root = document.as_hash().ok_or(UnreadableBuildspec::NotRead)?;
    let mut read = Buildspec {
        declares_artifacts: mapping_get(root, ARTIFACTS_KEY).is_some(),
        ..Buildspec::default()
    };
    if let Some(env) = mapping_get(root, ENV_KEY).and_then(::yaml_rust2::Yaml::as_hash) {
        if let Some(variables) =
            mapping_get(env, VARIABLES_KEY).and_then(::yaml_rust2::Yaml::as_hash)
        {
            read.variables = variables
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.as_str()?.to_string(), scalar_string(value)?))
                })
                .collect();
        }
        if let Some(exported) =
            mapping_get(env, EXPORTED_VARIABLES_KEY).and_then(::yaml_rust2::Yaml::as_vec)
        {
            read.exported_variables = exported.iter().filter_map(scalar_string).collect();
        }
    }
    if let Some(phases) = mapping_get(root, PHASES_KEY).and_then(::yaml_rust2::Yaml::as_hash) {
        for (key, phase_type) in COMMAND_PHASES {
            let Some(phase) = mapping_get(phases, key).and_then(::yaml_rust2::Yaml::as_hash) else {
                continue;
            };
            let commands = mapping_get(phase, COMMANDS_KEY)
                .and_then(::yaml_rust2::Yaml::as_vec)
                .map(|listed| listed.iter().filter_map(scalar_string).collect())
                .unwrap_or_default();
            let abort_on_failure = mapping_get(phase, ON_FAILURE_KEY)
                .and_then(scalar_string)
                .is_none_or(|on_failure| !on_failure.eq_ignore_ascii_case(ON_FAILURE_CONTINUE));
            if key == INSTALL_PHASE_KEY {
                if let Some(runtimes) =
                    mapping_get(phase, RUNTIME_VERSIONS_KEY).and_then(::yaml_rust2::Yaml::as_hash)
                {
                    read.runtime_versions = runtimes
                        .iter()
                        .filter_map(|(runtime, version)| {
                            Some((runtime.as_str()?.to_string(), scalar_string(version)?))
                        })
                        .collect();
                }
            }
            read.phases.push(DeclaredPhase {
                phase_type,
                commands,
                abort_on_failure,
            });
        }
    }
    Ok(read)
}

/// What the source-download phase reports about a buildspec it could not read.
///
/// A document the parser REFUSED is reported as the parser answered it — its own
/// diagnostic, unclassified, which is what names the construct and the line a caller has
/// to fix. CodeBuild's own `YAML_FILE_ERROR` classification is what it states about a
/// buildspec it could not obtain or make a specification of at all.
fn unreadable_context(unreadable: &UnreadableBuildspec) -> PhaseContext {
    match unreadable {
        UnreadableBuildspec::Diagnostic(diagnostic) => PhaseContext {
            status_code: None,
            message: Some(diagnostic.clone()),
        },
        UnreadableBuildspec::NotRead => yaml_file_error_context(UNREADABLE_YAML_MESSAGE),
    }
}

/// The context CodeBuild's `YAML_FILE_ERROR` classification is reported with.
fn yaml_file_error_context(message: &str) -> PhaseContext {
    PhaseContext {
        status_code: Some(YAML_FILE_ERROR.to_string()),
        message: Some(message.to_string()),
    }
}

/// What the source-download phase reports about a runtime selection the build environment
/// could not satisfy.
///
/// A runtime NO build image has is the buildspec naming something that is not a runtime, so
/// it carries the same classification as a buildspec CodeBuild could not make a
/// specification of; a version of a known runtime the environment does not have is an
/// installation that could not be performed, which is reported as the command failure it is.
fn runtime_install_context(failure: &RuntimeInstallFailure) -> PhaseContext {
    match failure {
        RuntimeInstallFailure::UnknownRuntime(message) => yaml_file_error_context(message),
        RuntimeInstallFailure::UnknownVersion(message) => PhaseContext {
            status_code: Some(COMMAND_EXECUTION_ERROR.to_string()),
            message: Some(message.clone()),
        },
    }
}

/// What running a build answered: its terminal status, the phase it ends in, the phases
/// it reports and the variables its buildspec exported.
#[derive(Debug, Clone)]
pub struct BuildRun {
    /// The build's terminal `buildStatus`.
    pub status: String,
    /// The phases the build reports, in the order it reached them.
    pub phases: Vec<BuildPhase>,
    /// The variables the buildspec's `exported-variables` named and the build's session
    /// held a value for — `None` when it exported none.
    pub exported_variables: Option<Vec<ExportedEnvironmentVariable>>,
}

/// Run `build`'s buildspec and answer what it did.
///
/// `start_millis` is the instant the build was submitted and `due_millis` the instant the
/// clock owed its terminal transition at, so the queue closes at `due_millis` and every
/// phase that then ran is reported inside the same instant: this session runs the whole
/// buildspec at once rather than over wall-clock time.
pub fn run_build(build: &Build, start_millis: i64, due_millis: i64) -> BuildRun {
    let mut phases = vec![
        finished_phase(
            PHASE_SUBMITTED,
            BUILD_STATUS_SUCCEEDED,
            start_millis,
            start_millis,
            None,
        ),
        finished_phase(
            PHASE_QUEUED,
            BUILD_STATUS_SUCCEEDED,
            start_millis,
            due_millis,
            None,
        ),
        // The build environment this emulator provides is created in process, which is
        // what this phase is: see `super::build_shell`.
        finished_phase(
            PHASE_PROVISIONING,
            BUILD_STATUS_SUCCEEDED,
            due_millis,
            due_millis,
            None,
        ),
    ];

    let buildspec = build
        .source
        .as_ref()
        .and_then(|source| source.buildspec.as_deref())
        .map(str::trim)
        .filter(|buildspec| !buildspec.is_empty());
    let read = match buildspec {
        None => Err(yaml_file_error_context(NO_YAML_FILE_MESSAGE)),
        Some(buildspec) => {
            read_buildspec(buildspec).map_err(|unreadable| unreadable_context(&unreadable))
        }
    };
    let read = match read {
        Ok(read) => read,
        Err(context) => {
            // The specification the build would run could not be read, so no phase of
            // it ran: the build fails where CodeBuild reports a buildspec it cannot
            // read.
            phases.push(finished_phase(
                PHASE_DOWNLOAD_SOURCE,
                BUILD_STATUS_FAILED,
                due_millis,
                due_millis,
                Some(vec![context]),
            ));
            phases.push(open_phase(PHASE_COMPLETED, due_millis));
            return BuildRun {
                status: BUILD_STATUS_FAILED.to_string(),
                phases,
                exported_variables: None,
            };
        }
    };
    // The runtimes the buildspec selected are installed from what the build environment
    // has, in the phase that prepares the build from its specification: a selection it
    // cannot satisfy leaves the build nothing to run, reported as
    // [`runtime_install_context`] states.
    let installed = match install_runtimes(&read.runtime_versions) {
        Ok(installed) => installed,
        Err(failure) => {
            phases.push(finished_phase(
                PHASE_DOWNLOAD_SOURCE,
                BUILD_STATUS_FAILED,
                due_millis,
                due_millis,
                Some(vec![runtime_install_context(&failure)]),
            ));
            phases.push(open_phase(PHASE_COMPLETED, due_millis));
            return BuildRun {
                status: BUILD_STATUS_FAILED.to_string(),
                phases,
                exported_variables: None,
            };
        }
    };
    // The buildspec was obtained and read, which is the work of this phase.
    phases.push(finished_phase(
        PHASE_DOWNLOAD_SOURCE,
        BUILD_STATUS_SUCCEEDED,
        due_millis,
        due_millis,
        None,
    ));

    let mut shell = BuildShell::new(session_variables(build, &read, start_millis), installed);
    let mut failed = false;
    let mut aborted = false;
    for declared in &read.phases {
        let outcome = run_phase(&mut shell, declared);
        let phase_failed = outcome.is_some();
        failed |= phase_failed;
        phases.push(finished_phase(
            declared.phase_type,
            if phase_failed {
                BUILD_STATUS_FAILED
            } else {
                BUILD_STATUS_SUCCEEDED
            },
            due_millis,
            due_millis,
            outcome.map(|context| vec![context]),
        ));
        // A phase whose failure aborts the build is the last phase that ran, so the build
        // reports none of the ones after it; `on-failure: CONTINUE` goes on to the next
        // phase, however many phases have already failed.
        if phase_failed && declared.abort_on_failure {
            aborted = true;
            break;
        }
        // `exit` ended the build's session: a non-zero status already failed the phase
        // above (so the build was aborted there), and a zero status is a build that ran
        // to success with the phases after it left unrun.
        if shell.exit_status().is_some() {
            break;
        }
    }

    if !aborted {
        // The build's own finishing work: putting the artifact files where the build's
        // artifacts say, and ending the session.
        let upload = upload_artifacts_phase(build, &read, due_millis);
        failed |= upload.phase_status.as_deref() == Some(BUILD_STATUS_FAILED);
        phases.push(upload);
        phases.push(finished_phase(
            PHASE_FINALIZING,
            BUILD_STATUS_SUCCEEDED,
            due_millis,
            due_millis,
            None,
        ));
    }
    phases.push(open_phase(PHASE_COMPLETED, due_millis));
    BuildRun {
        status: if failed {
            BUILD_STATUS_FAILED.to_string()
        } else {
            BUILD_STATUS_SUCCEEDED.to_string()
        },
        phases,
        exported_variables: exported_variables(&shell, &read),
    }
}

/// The `UPLOAD_ARTIFACTS` phase of a build that got through its commands: putting the
/// files the buildspec's `artifacts` section names where the build's `artifacts` say.
///
/// A build whose artifacts name no destination has nothing to put anywhere, so the phase
/// is the whole of its own work and succeeds — which is every build whose artifacts
/// CodePipeline manages and every build that produces none, the two types whose
/// `location` the model documents as ignored.
///
/// A build that DOES name a destination bucket would have to PUT its artifact objects
/// into that bucket, and this build environment puts none there — the files its commands
/// wrote are its own session's ([`super::build_shell`]) and are not uploaded anywhere —
/// so no upload is claimed: the phase ends in the client error CodeBuild reports for an
/// upload that found nothing to put at its destination, which is the buildspec naming no
/// artifact file at all, or no file arriving for the paths it named. Nothing reports an
/// upload that did not happen.
fn upload_artifacts_phase(build: &Build, read: &Buildspec, due_millis: i64) -> BuildPhase {
    let destination = build
        .artifacts
        .as_ref()
        .and_then(|artifacts| artifacts.location.as_deref())
        .unwrap_or_default()
        .trim();
    if destination.is_empty() {
        return finished_phase(
            PHASE_UPLOAD_ARTIFACTS,
            BUILD_STATUS_SUCCEEDED,
            due_millis,
            due_millis,
            None,
        );
    }
    let message = if read.declares_artifacts {
        NO_MATCHING_ARTIFACT_PATHS_MESSAGE
    } else {
        NO_ARTIFACT_FILES_MESSAGE
    };
    finished_phase(
        PHASE_UPLOAD_ARTIFACTS,
        BUILD_STATUS_FAILED,
        due_millis,
        due_millis,
        Some(vec![PhaseContext {
            status_code: Some(CLIENT_ERROR.to_string()),
            message: Some(message.to_string()),
        }]),
    )
}

/// Run one phase's command lines in `shell`, and answer the context of the first command
/// that failed — `None` when every command of the phase exited zero, which is the phase
/// having succeeded.
///
/// A phase stops at its first failing command ("CodeBuild runs each command, one at a
/// time, in the order listed"), and at an `exit` that ended the session.
fn run_phase(shell: &mut BuildShell, declared: &DeclaredPhase) -> Option<PhaseContext> {
    for command in &declared.commands {
        let status = shell.run(command);
        if status != 0 {
            return Some(PhaseContext {
                status_code: Some(COMMAND_EXECUTION_ERROR.to_string()),
                message: Some(command_failure_message(command, status)),
            });
        }
        if shell.exit_status().is_some() {
            return None;
        }
    }
    None
}

/// The variables one build's session starts with.
///
/// In the order AWS resolves them, so a later one replaces an earlier one of the same
/// name: the variables CodeBuild itself sets for the build, which are read off the
/// build's own record; then the buildspec's `env/variables`; then the build's
/// `environment.environmentVariables`, which `StartBuild`'s overrides are already
/// applied to.
///
/// A variable whose type is not [`PLAINTEXT_VARIABLE_TYPE`] names a parameter or a
/// secret held by another service, which this session does not resolve, so it sets
/// nothing rather than setting the reference as if it were the value.
fn session_variables(
    build: &Build,
    read: &Buildspec,
    start_millis: i64,
) -> BTreeMap<String, String> {
    let mut variables = BTreeMap::new();
    let provided: [(&str, Option<String>); 6] = [
        ("CODEBUILD_BUILD_ID", build.id.clone()),
        ("CODEBUILD_BUILD_ARN", build.arn.clone()),
        (
            "CODEBUILD_BUILD_NUMBER",
            build.build_number.map(|number| number.to_string()),
        ),
        ("CODEBUILD_INITIATOR", build.initiator.clone()),
        ("CODEBUILD_START_TIME", Some(start_millis.to_string())),
        ("CODEBUILD_SOURCE_VERSION", build.source_version.clone()),
    ];
    for (name, value) in provided {
        if let Some(value) = value {
            variables.insert(name.to_string(), value);
        }
    }
    for (name, value) in &read.variables {
        variables.insert(name.clone(), value.clone());
    }
    for variable in build
        .environment
        .as_ref()
        .and_then(|environment| environment.environment_variables.as_deref())
        .unwrap_or_default()
    {
        let plaintext = variable
            .r#type
            .as_deref()
            .is_none_or(|kind| kind == PLAINTEXT_VARIABLE_TYPE);
        if plaintext {
            variables.insert(variable.name.clone(), variable.value.clone());
        }
    }
    variables
}

/// The variables the buildspec's `exported-variables` named, with the values the build's
/// session ended holding — `None` when the buildspec named none or the session held a
/// value for none of them.
fn exported_variables(
    shell: &BuildShell,
    read: &Buildspec,
) -> Option<Vec<ExportedEnvironmentVariable>> {
    let exported: Vec<ExportedEnvironmentVariable> = read
        .exported_variables
        .iter()
        .filter_map(|name| {
            Some(ExportedEnvironmentVariable {
                name: Some(name.clone()),
                value: Some(shell.variable(name)?.to_string()),
            })
        })
        .collect();
    (!exported.is_empty()).then_some(exported)
}
