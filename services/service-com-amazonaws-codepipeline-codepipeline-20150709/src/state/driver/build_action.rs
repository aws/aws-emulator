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
//! # The artifacts the action and the build exchange
//!
//! Such a project's own source states no location: `ProjectSource$location` "should not be
//! specified" for that source type because "CodePipeline uses the settings in a pipeline's
//! source action instead of this value". So the build is built FROM the action's input
//! artifact and puts what it produces at the location the action's OUTPUT artifact occupies
//! in the pipeline's artifact store, and the start is what hands both over: the input
//! artifact's object as the build's `sourceVersion`, and the output artifact's object as the
//! destination its artifacts are uploaded to. A build whose action declares no input
//! artifact is built from nothing, and one whose action declares no output artifact uploads
//! nothing — neither is invented here.
//!
//! The build is reached through the framework's in-process callback, so an action can only
//! report a build that service really holds: a project nothing created is a declaration the
//! pipeline cannot run, which the action's own `ConfigurationError` failure reports.

use super::super::execution::{ArtifactRef, JobOutcome};
use super::{service_error_detail, FAILURE_CONFIGURATION_ERROR, FAILURE_JOB_FAILED};

/// The `StatusType` a build that ran to success reports (`Build$buildStatus`).
const BUILD_SUCCEEDED: &str = "SUCCEEDED";

/// The `ArtifactsType` of a destination whose objects go into a bucket of the account's
/// object store: "S3: The build project stores build output in Amazon S3"
/// (`ProjectArtifacts$type`). The pipeline's artifact store IS such a bucket, so the
/// destination a build action hands the build is one of that type, at the key the output
/// artifact occupies.
const ARTIFACTS_TYPE_S3: &str = "S3";

/// The `ArtifactNamespace` that puts nothing of its own into the artifact's key: "NONE: Do
/// not include the build ID". The key is the one this execution assigned the output
/// artifact, so nothing else may be composed into it.
const ARTIFACTS_NAMESPACE_NONE: &str = "NONE";

/// The `ArtifactPackaging` that puts the whole output into ONE object: "ZIP: CodeBuild
/// creates in the output bucket a ZIP file that contains the build output." One artifact of
/// a pipeline is one object, which the actions after it read as the archive it is.
const ARTIFACTS_PACKAGING_ZIP: &str = "ZIP";

/// The S3 ARN of the object `location` holds, which is how an artifact object is ADDRESSED:
/// the form a build reports its own artifacts' location in, and the form the input artifact
/// is handed to a build as.
fn object_arn(location: &ArtifactRef) -> String {
    format!("arn:aws:s3:::{}/{}", location.bucket, location.key)
}

/// Start a build of `project_name`, and report the action as running that build.
///
/// The environment variables the action's configuration declares are the build's own
/// overrides — "a set of environment variables" the action passes to the project — so they
/// travel with the start rather than being applied to the project itself.
///
/// So do the action's ARTIFACTS: the first of `inputs` is the object the build is built
/// from, handed over as the build's `sourceVersion`, and the first of `outputs` is where
/// the build's own output goes, handed over as the destination its artifacts are uploaded
/// to. This action type binds one of each, so a location past the first is not passed as a
/// second source or a second destination.
///
/// The start is made UNDER `role_arn` — the role this action is performed as — so it is
/// authorized against the permissions in force on that role BEFORE anything is started: a
/// role that may not start a build of the project starts none, and the action fails with
/// the denial rather than with the outcome of a build nothing should have created.
pub(super) async fn run_codebuild_start(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    project_name: &str,
    role_arn: &str,
    environment_overrides: &super::super::execution::EnvironmentOverrides,
    inputs: &[ArtifactRef],
    outputs: &[ArtifactRef],
) -> JobOutcome {
    if let Some(denial) = start_build_denial(callback, region, role_arn, project_name).await {
        return JobOutcome::Failed {
            code: FAILURE_JOB_FAILED.to_string(),
            message: denial,
            // No build was started, so the action names no run in another service: a
            // client reading the result must not find one.
            external_id: None,
        };
    }
    let codebuild = codebuild_client(callback, region);
    let mut started = codebuild.start_build().project_name(project_name);
    if let Some(input) = inputs.first() {
        // What the build is built from: the artifact object the earlier action produced.
        started = started.source_version(object_arn(input));
    }
    if let Some(output) = outputs.first() {
        // Where the build's output goes: the object this execution assigned the action's
        // output artifact, as one archive at exactly that key.
        let (path, name) = match output.key.rsplit_once('/') {
            Some((path, name)) => (path.to_string(), name.to_string()),
            None => (String::new(), output.key.clone()),
        };
        match ::aws_sdk_codebuild::types::ProjectArtifacts::builder()
            .r#type(::aws_sdk_codebuild::types::ArtifactsType::from(
                ARTIFACTS_TYPE_S3,
            ))
            .location(&output.bucket)
            .path(path)
            .name(name)
            .namespace_type(::aws_sdk_codebuild::types::ArtifactNamespace::from(
                ARTIFACTS_NAMESPACE_NONE,
            ))
            .packaging(::aws_sdk_codebuild::types::ArtifactPackaging::from(
                ARTIFACTS_PACKAGING_ZIP,
            ))
            .build()
        {
            Ok(destination) => started = started.artifacts_override(destination),
            Err(err) => {
                return JobOutcome::Failed {
                    code: FAILURE_CONFIGURATION_ERROR.to_string(),
                    message: format!(
                        "the output artifact of this action cannot be passed to the \
                         project {project_name} as a build destination: {err}"
                    ),
                    external_id: None,
                }
            }
        }
    }
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
///
/// `outputs` are the locations the build was told to upload to: a succeeded build that
/// reports no artifacts put nothing there, so the action does not publish an artifact
/// nothing wrote.
pub(super) async fn run_codebuild_poll(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    build_id: &str,
    outputs: &[ArtifactRef],
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
    // The object the action publishes as its output artifact is the one the build
    // UPLOADED: `BuildArtifacts$location` is "the location of the artifact", so a build
    // that reports none uploaded none, and the action must not publish an artifact for a
    // later action to read where nothing was written.
    let uploaded = build
        .artifacts
        .as_ref()
        .and_then(|artifacts| artifacts.location.as_deref())
        .unwrap_or_default();
    if let Some(output) = outputs.first() {
        let expected = object_arn(output);
        if uploaded != expected {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the build {build_id} this action started did not put its output \
                     artifact at {expected}"
                ),
                external_id: Some(build_id.to_string()),
            };
        }
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

/// The IAM action CodePipeline performs a `CodeBuild` action's work with
/// (`codebuild:StartBuild`, "Actions defined by AWS CodeBuild", IAM Service Authorization
/// Reference).
const START_BUILD_ACTION: &str = "codebuild:StartBuild";

/// The ARN namespace, and the resource type, of the project a build action builds
/// (`arn:<partition>:codebuild:<region>:<account>:project/<name>`, "Resource types defined
/// by AWS CodeBuild").
const CODEBUILD_ARN_SERVICE: &str = "codebuild";

/// `Some(message)` when the permissions in force on `role_arn` do NOT let it start a build
/// of `project_name`, and `None` when they do or when nothing is known about the role.
///
/// The question is the one every service asks before it ACTS UNDER a caller-supplied role:
/// the call CodePipeline makes is authorized against that role's own permissions rather than
/// against the caller's, so a role whose policies do not allow `codebuild:StartBuild` on the
/// project cannot make it — and this service must not make it either. It is put to IAM
/// itself, through the workspace's one reading of what a role may do
/// (`::service_common::iam_role`).
///
/// Which answer counts follows that module's own rule: a role that STATES identity-based
/// policies is the authority on what it may do, so it has to be ALLOWED the action; a role
/// IAM holds no policy for says nothing about the action — a pipeline role is routinely
/// provisioned out of band — so only a statement that DENIES it holds the start back.
/// Everything that is not an answer (no IAM to ask, a value naming no role of this account,
/// a failed hop) withholds nothing.
///
/// Which of IAM's rules withheld the grant is read off the DECISION itself rather than off
/// how much the role states: a statement that denies the action is an explicit deny whether
/// or not the same role also grants it, and a policy set that merely does not mention the
/// action is IAM's default deny. So the question is put in its three-valued form
/// (`role_action_decision_through`), and the role's own statements are only consulted for
/// the one case where a default deny is not an answer: a role carrying no identity-based
/// policy at all.
async fn start_build_denial(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    role_arn: &str,
    project_name: &str,
) -> Option<String> {
    use ::service_common::iam_authorization::Decision;

    let project = ::engine::emulator_arn(
        CODEBUILD_ARN_SERVICE,
        region,
        &format!("project/{project_name}"),
    );
    let decision = ::service_common::iam_role::role_action_decision_through(
        callback,
        region,
        role_arn,
        START_BUILD_ACTION,
        &project,
        &[],
    )
    .await?;
    let reason = match decision {
        Decision::Allowed => return None,
        // A fact about the role's own documents, so the sentence names them rather than the
        // action.
        Decision::Denied => {
            ::service_common::iam_authorization::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
        }
        // IAM's default deny — an answer about this role only where the role STATES
        // identity-based policies at all (see above).
        Decision::NotStated => {
            if !::service_common::iam_role::role_states_permissions_through(
                callback, region, role_arn,
            )
            .await
            {
                return None;
            }
            ::service_common::iam_authorization::no_identity_policy_allows(START_BUILD_ACTION)
        }
    };
    // The sentence AWS refuses the call with, worded once for every service that decides the
    // identity-based question (`::service_common::iam_authorization`): the principal the call
    // is made by, the action it asked for, the project it asked for, and which of IAM's rules
    // withheld the grant.
    Some(
        ::service_common::iam_authorization::not_authorized_by_identity_policy(
            role_arn,
            START_BUILD_ACTION,
            &project,
            &reason,
        ),
    )
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
