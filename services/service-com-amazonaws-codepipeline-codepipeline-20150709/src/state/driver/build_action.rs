// SPDX-License-Identifier: CC0-1.0

//! The real work of a CodeBuild action: starting the build of the project the action
//! names, and settling the action from that build's own terminal status.
//!
//! # The rule this module implements
//!
//! A CodePipeline action of category `Build` (or `Test`) provided by `CodeBuild` does not
//! perform the build itself: it INVOKES the project its `ProjectName` configuration names
//! and then follows the build that service started, so the action's outcome is the build's
//! outcome and the run a client sees for the action is that build. Nothing about the action
//! is decided here: [`run_codebuild_start`] starts the build and reports the action as
//! running, [`run_codebuild_poll`] reads the build again each time the action's next step
//! falls due, and only a build CodeBuild reports COMPLETE settles the action — succeeded
//! when the build succeeded, failed with the state the build ended in otherwise.
//!
//! The build is reached through the framework's in-process callback, so an action can only
//! report a build that service really holds: a project nothing created is a declaration the
//! pipeline cannot run, which the action's own `ConfigurationError` failure reports.

use super::super::execution::JobOutcome;
use super::{service_error_detail, FAILURE_CONFIGURATION_ERROR, FAILURE_JOB_FAILED};

/// The `StatusType` a build that ran to success reports (`Build$buildStatus`).
const BUILD_SUCCEEDED: &str = "SUCCEEDED";

/// Start a build of `project_name`, and report the action as running that build.
///
/// The environment variables the action's configuration declares are the build's own
/// overrides — "a set of environment variables" the action passes to the project — so they
/// travel with the start rather than being applied to the project itself.
pub(super) async fn run_codebuild_start(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    project_name: &str,
    environment_overrides: &super::super::execution::EnvironmentOverrides,
) -> JobOutcome {
    let codebuild = codebuild_client(callback, region);
    let mut started = codebuild.start_build().project_name(project_name);
    for (name, value, kind) in environment_overrides {
        let mut variable = ::aws_sdk_codebuild::types::EnvironmentVariable::builder()
            .name(name)
            .value(value);
        if let Some(kind) = kind {
            variable = variable
                .r#type(::aws_sdk_codebuild::types::EnvironmentVariableType::from(
                    kind.as_str(),
                ));
        }
        match variable.build() {
            Ok(variable) => started = started.environment_variables_override(variable),
            Err(err) => {
                return JobOutcome::Failed {
                    code: FAILURE_CONFIGURATION_ERROR.to_string(),
                    message: format!(
                        "an environment variable this action declares cannot be passed to \
                         the project {project_name}: {err}"
                    ),
                    external_id: None,
                }
            }
        }
    }
    match started.send().await {
        Ok(answer) => {
            let Some(build_id) = answer.build_value.and_then(|build| build.id) else {
                return JobOutcome::Failed {
                    code: FAILURE_JOB_FAILED.to_string(),
                    message: format!(
                        "the build of the project {project_name} was started without an \
                         id, so the action cannot follow it"
                    ),
                    external_id: None,
                };
            };
            JobOutcome::Running {
                summary: format!("Started the build {build_id} of the project {project_name}"),
                external_run: build_id,
            }
        }
        Err(err) => {
            // A project this account does not hold is the declaration's problem, not the
            // run's: no build was started, and none could be.
            let absent = err.as_service_error().is_some_and(
                ::aws_sdk_codebuild::operation::start_build::StartBuildError::is_resource_not_found_exception,
            );
            let code = if absent {
                FAILURE_CONFIGURATION_ERROR
            } else {
                FAILURE_JOB_FAILED
            };
            JobOutcome::Failed {
                code: code.to_string(),
                message: format!(
                    "a build of the project {project_name} could not be started for this \
                     action: {}",
                    service_error_detail(&err)
                ),
                external_id: None,
            }
        }
    }
}

/// Read the build this action started, and settle the action once that build is complete.
///
/// `Build$buildComplete` is "whether the build is complete", so a build that is not is one
/// this action is still waiting for: the action stays running and the read falls due again.
/// A complete build settles the action with the state it ended in, which is the wording
/// CodePipeline reports for a build action that did not succeed.
pub(super) async fn run_codebuild_poll(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    build_id: &str,
) -> JobOutcome {
    let codebuild = codebuild_client(callback, region);
    let read = codebuild.batch_get_builds().ids(build_id).send().await;
    let build = match read {
        Ok(answer) => answer.builds.unwrap_or_default().into_iter().next(),
        Err(err) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the build {build_id} this action started could not be read: {}",
                    service_error_detail(&err)
                ),
                external_id: Some(build_id.to_string()),
            }
        }
    };
    let Some(build) = build else {
        return JobOutcome::Failed {
            code: FAILURE_JOB_FAILED.to_string(),
            message: format!(
                "CodeBuild no longer holds the build {build_id} this action started"
            ),
            external_id: Some(build_id.to_string()),
        };
    };
    if !build.build_complete {
        return JobOutcome::Running {
            summary: format!("Waiting for the build {build_id}"),
            external_run: build_id.to_string(),
        };
    }
    let status = build
        .build_status
        .as_ref()
        .map_or_else(String::new, |status| status.as_str().to_string());
    if status != BUILD_SUCCEEDED {
        return JobOutcome::Failed {
            code: FAILURE_JOB_FAILED.to_string(),
            message: format!("Build terminated with state: {status}"),
            external_id: Some(build_id.to_string()),
        };
    }
    JobOutcome::Done {
        summary: format!("Build {build_id} succeeded"),
        // A build is no revision of anything: a build action resolves no source revision.
        revision: None,
        // The variables a CodeBuild action publishes are the ones the build EXPORTED
        // (`exported-variables` in its buildspec), which is what that service reports as
        // the build's `exportedEnvironmentVariables`; a build that exported none
        // publishes none — nothing is invented here.
        variables: build
            .exported_environment_variables
            .unwrap_or_default()
            .into_iter()
            .filter_map(|variable| Some((variable.name?, variable.value?)))
            .collect(),
        external_id: Some(build_id.to_string()),
    }
}

/// A CodeBuild client over the in-process callback into this same emulator.
///
/// Built exactly like the clients the driver performs the other actions' work with: a fixed
/// endpoint literal (`state::in_process_endpoint`), the emulator's own credentials, and no
/// retry, timeout or stalled-stream machinery, because the transport is in process.
fn codebuild_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_codebuild::Client {
    let config = ::aws_sdk_codebuild::Config::builder()
        .behavior_version(::aws_sdk_codebuild::config::BehaviorVersion::latest())
        .region(::aws_sdk_codebuild::config::Region::new(region.to_string()))
        .endpoint_url(crate::state::in_process_endpoint("codebuild"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_codebuild::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_codebuild::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_codebuild::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_codebuild::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_codebuild::Client::from_conf(config)
}
