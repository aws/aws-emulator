// SPDX-License-Identifier: CC0-1.0

//! The real work of an Amazon ECR SOURCE action: resolving the image a pipeline sources
//! and writing that image's detail into the artifact store.
//!
//! # The rule this module implements
//!
//! An Amazon ECR source action's revision is an IMAGE of the repository its configuration
//! names, at the tag it names: "when a new image is pushed with the configured tag, the
//! pipeline is triggered", and the action's output artifact is the `imageDetail.json`
//! document that names the image — its digest, its tags, its size, when it was pushed and
//! the URI a deploy action downstream pulls it by. The revision the execution records for
//! the action is that image's DIGEST, which is what makes one push distinguishable from
//! the next under the same tag.
//!
//! Nothing here invents any of it: the image is read from Amazon ECR itself, through the
//! framework's in-process callback, so an action can only report an image that registry
//! really holds — a tag it holds no image for is a revision the pipeline cannot source,
//! which the action's own failure reports.

use ::std::collections::BTreeMap;

use super::super::execution::{ArtifactRef, JobOutcome};
use super::{service_error_detail, FAILURE_CONFIGURATION_ERROR, FAILURE_JOB_FAILED};

/// The name of the document an Amazon ECR source action's output artifact carries, as AWS
/// documents it for that action ("the `imageDetail.json` file").
const IMAGE_DETAIL_FILE: &str = "imageDetail.json";

/// One source revision an execution's START resolved: which action it belongs to, which of
/// that action's output artifacts carry it, and the revision itself.
#[derive(Debug, Clone)]
pub struct StartedSourceRevision {
    /// The declared action that sources it.
    pub action_name: String,
    /// The `OutputArtifact$name`s that action produces from it.
    pub artifact_names: Vec<String>,
    /// The revision — an Amazon ECR source action's is the image's digest.
    pub revision_id: String,
}

/// The revision each Amazon ECR source action of `declaration` is sourcing AT THIS INSTANT:
/// the digest of the image its repository holds at the tag it names.
///
/// An execution is started FOR a particular revision of each of its sources, which is what
/// lets it report the revision it is running before it has reached the action that produces
/// the artifact from it — and what pins the action to that image, so an image pushed while
/// the execution is in flight does not change what this execution built.
///
/// Read from Amazon ECR itself, so only a revision that registry really holds is ever
/// recorded: an action whose repository or tag resolves to nothing contributes nothing here,
/// and the action's own run is what then reports that as its failure.
pub async fn resolved_ecr_source_revisions(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    declaration: &crate::ops::types::PipelineDeclaration,
) -> Vec<StartedSourceRevision> {
    let ecr = ecr_client(callback, region);
    let mut resolved = Vec::new();
    for action in declaration
        .stages
        .iter()
        .flat_map(|stage| stage.actions.iter())
    {
        if action.action_type_id.category != super::CATEGORY_SOURCE
            || action.action_type_id.provider != super::PROVIDER_ECR
        {
            continue;
        }
        let configuration = action.configuration.clone().unwrap_or_default();
        let Some(repository) = configuration.get(super::CONFIG_REPOSITORY_NAME) else {
            continue;
        };
        let image_tag = configuration
            .get(super::CONFIG_IMAGE_TAG)
            .filter(|tag| !tag.is_empty())
            .cloned()
            .unwrap_or_else(|| super::DEFAULT_IMAGE_TAG.to_string());
        let read = ecr
            .describe_images()
            .repository_name(repository)
            .image_ids(
                ::aws_sdk_ecr::types::ImageIdentifier::builder()
                    .image_tag(&image_tag)
                    .build(),
            )
            .send()
            .await;
        let digest = match read {
            Ok(answer) => answer
                .image_details
                .unwrap_or_default()
                .into_iter()
                .next()
                .and_then(|detail| detail.image_digest),
            // Nothing is recorded for a repository or a tag the registry does not hold:
            // the action's own run reports that the pipeline cannot source it.
            Err(_) => None,
        };
        let Some(digest) = digest else {
            continue;
        };
        resolved.push(StartedSourceRevision {
            action_name: action.name.clone(),
            artifact_names: action
                .output_artifacts
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|artifact| artifact.name)
                .collect(),
            revision_id: digest,
        });
    }
    resolved
}

/// The `Version` the `imageDetail.json` document declares, from AWS's own documentation of
/// the file.
const IMAGE_DETAIL_VERSION: &str = "1.0";

/// Resolve the image at `image_tag` of `repository` and write its detail into the artifact
/// store as the action's output artifact.
///
/// A repository or a tag the registry does not hold is a declaration this pipeline cannot
/// source from at all, which is what the `ConfigurationError` failure type reports;
/// anything that goes wrong while the artifact is being written is the run's own failure
/// (`JobFailed`).
pub(super) async fn run_ecr_source(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    s3: &::aws_sdk_s3::Client,
    region: &str,
    repository: &str,
    image_tag: &str,
    pinned_digest: Option<&str>,
    outputs: &[ArtifactRef],
) -> JobOutcome {
    let ecr = ecr_client(callback, region);
    // The image this execution was STARTED for, when its start resolved one: an execution
    // sources the revision it is running, so an image pushed to the same tag since does
    // not change what this run produces. Without a pinned revision the action resolves the
    // tag now, which is what a start that resolved nothing leaves it to do.
    let addressed = match pinned_digest {
        Some(digest) => ::aws_sdk_ecr::types::ImageIdentifier::builder()
            .image_digest(digest)
            .build(),
        None => ::aws_sdk_ecr::types::ImageIdentifier::builder()
            .image_tag(image_tag)
            .build(),
    };
    let read = ecr
        .describe_images()
        .repository_name(repository)
        .image_ids(addressed)
        .send()
        .await;
    let detail = match read {
        Ok(answer) => answer
            .image_details
            .unwrap_or_default()
            .into_iter()
            .next(),
        Err(err) => {
            let service = err.as_service_error();
            // A repository or an image the registry does not hold is the declaration's
            // problem, not the run's: there is no revision to source.
            let unreachable = service.is_some_and(|err| {
                err.is_repository_not_found_exception() || err.is_image_not_found_exception()
            });
            let code = if unreachable {
                FAILURE_CONFIGURATION_ERROR
            } else {
                FAILURE_JOB_FAILED
            };
            return JobOutcome::Failed {
                code: code.to_string(),
                message: format!(
                    "the image {image_tag} of the repository {repository} could not be \
                     resolved: {}",
                    service_error_detail(&err)
                ),
                external_id: None,
            };
        }
    };
    let Some(detail) = detail else {
        return JobOutcome::Failed {
            code: FAILURE_CONFIGURATION_ERROR.to_string(),
            message: format!(
                "the repository {repository} holds no image tagged {image_tag}, so there \
                 is no revision to source"
            ),
            external_id: None,
        };
    };
    let Some(digest) = detail.image_digest.clone() else {
        return JobOutcome::Failed {
            code: FAILURE_JOB_FAILED.to_string(),
            message: format!(
                "the image {image_tag} of the repository {repository} reports no digest, \
                 so there is no revision to source"
            ),
            external_id: None,
        };
    };
    let registry_id = detail.registry_id.clone().unwrap_or_default();
    let image_uri = format!("{registry_id}.dkr.ecr.{region}.amazonaws.com/{repository}@{digest}");
    let document = image_detail_document(&detail, repository, &registry_id, &digest, &image_uri);

    // The artifact a source action produces is an ARCHIVE, which is what a later action
    // extracts: this one carries the single document the action publishes.
    let entries = vec![(IMAGE_DETAIL_FILE.to_string(), document.into_bytes())];
    let archive = match ::tokio::task::spawn_blocking(move || super::zip_of(&entries)).await {
        Ok(Ok(archive)) => archive,
        Ok(Err(detail)) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!("the source artifact could not be packed: {detail}"),
                external_id: Some(digest),
            }
        }
        Err(err) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!("the source artifact was not packed: {err}"),
                external_id: Some(digest),
            }
        }
    };
    for output in outputs {
        if let Err(err) = super::encrypted_put(
            s3.put_object()
                .bucket(&output.bucket)
                .key(&output.key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(archive.clone())),
            output.encryption.as_deref(),
        )
        .send()
        .await
        {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the artifact could not be written to s3://{}/{}: {}",
                    output.bucket,
                    output.key,
                    service_error_detail(&err)
                ),
                external_id: Some(digest),
            };
        }
    }

    // The output variables an Amazon ECR source action publishes for the later actions of
    // its execution, under the namespace the action declares.
    let mut variables = BTreeMap::new();
    variables.insert("ImageDigest".to_string(), digest.clone());
    variables.insert("ImageTag".to_string(), image_tag.to_string());
    variables.insert("ImageURI".to_string(), image_uri);
    variables.insert("RegistryId".to_string(), registry_id);
    variables.insert("RepositoryName".to_string(), repository.to_string());
    JobOutcome::Done {
        summary: format!(
            "Resolved the image {repository}:{image_tag} at {digest} as the output artifact"
        ),
        // The revision a source action resolved: the image's digest, which is what the
        // execution records as the action's source revision.
        revision: Some(digest.clone()),
        variables,
        external_id: Some(digest),
    }
}

/// The `imageDetail.json` document for one image, as AWS documents the file for an Amazon
/// ECR source action: the image's size, digest, push instant, registry, repository, URI and
/// tags, beside the file's own version.
///
/// Every value is the one Amazon ECR reported for the image; a member the registry states
/// nothing for is left out of the document rather than carrying an invented value.
fn image_detail_document(
    detail: &::aws_sdk_ecr::types::ImageDetail,
    repository: &str,
    registry_id: &str,
    digest: &str,
    image_uri: &str,
) -> String {
    let mut document = ::serde_json::Map::new();
    // "ImageSizeInBytes": a STRING in the document AWS publishes, even though the registry
    // reports the size as a number.
    if let Some(size) = detail.image_size_in_bytes {
        document.insert(
            "ImageSizeInBytes".to_string(),
            ::serde_json::Value::String(size.to_string()),
        );
    }
    document.insert(
        "ImageDigest".to_string(),
        ::serde_json::Value::String(digest.to_string()),
    );
    document.insert(
        "Version".to_string(),
        ::serde_json::Value::String(IMAGE_DETAIL_VERSION.to_string()),
    );
    if let Some(pushed_at) = detail.image_pushed_at {
        document.insert(
            "ImagePushedAt".to_string(),
            ::serde_json::Value::String(pushed_instant(pushed_at)),
        );
    }
    document.insert(
        "RegistryId".to_string(),
        ::serde_json::Value::String(registry_id.to_string()),
    );
    document.insert(
        "RepositoryName".to_string(),
        ::serde_json::Value::String(repository.to_string()),
    );
    document.insert(
        "ImageURI".to_string(),
        ::serde_json::Value::String(image_uri.to_string()),
    );
    document.insert(
        "ImageTags".to_string(),
        ::serde_json::Value::Array(
            detail
                .image_tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(::serde_json::Value::String)
                .collect(),
        ),
    );
    ::serde_json::Value::Object(document).to_string()
}

/// When the image was pushed, in the form the `imageDetail.json` document carries it —
/// `Mon Jan 21 20:04:00 UTC 2019`, as AWS's own example of the file shows it.
///
/// The instant is Amazon ECR's; only its RENDERING is this function's, and it is rendered
/// in UTC because that is the zone the form names.
fn pushed_instant(pushed_at: ::aws_smithy_types::DateTime) -> String {
    let seconds = pushed_at.secs();
    let rendered = ::chrono::DateTime::from_timestamp(seconds, 0);
    rendered.map_or_else(
        || seconds.to_string(),
        |at| at.format("%a %b %d %H:%M:%S UTC %Y").to_string(),
    )
}

/// An Amazon ECR client over the in-process callback into this same emulator.
///
/// Built exactly like the clients the driver performs the other actions' work with: a fixed
/// endpoint literal (`state::in_process_endpoint`), the emulator's own credentials, and no
/// retry, timeout or stalled-stream machinery, because the transport is in process.
fn ecr_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_ecr::Client {
    let config = ::aws_sdk_ecr::Config::builder()
        .behavior_version(::aws_sdk_ecr::config::BehaviorVersion::latest())
        .region(::aws_sdk_ecr::config::Region::new(region.to_string()))
        .endpoint_url(crate::state::in_process_endpoint("ecr"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ecr::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ecr::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ecr::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ecr::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ecr::Client::from_conf(config)
}
