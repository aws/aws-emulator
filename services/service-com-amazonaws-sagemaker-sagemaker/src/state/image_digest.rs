// SPDX-License-Identifier: CC0-1.0

//! The digest of the CONTAINER IMAGE a registration references, resolved against the
//! registry that HOLDS that image.
//!
//! `TrainingSpecification$TrainingImageDigest` and
//! `ModelPackageContainerDefinition$ImageDigest` carry one doc sentence — a hash "that
//! identifies the Docker image used for training" — beside the image members that name the
//! image itself: "The Amazon ECR registry path of the Docker image that contains the
//! training algorithm" and "The Amazon Elastic Container Registry (Amazon ECR) path where
//! inference code is stored"
//! (`smithy_query(query="shape",shape="TrainingSpecification,ModelPackageContainerDefinition")`).
//! Both digests target `ImageDigest` (`@length 0..72`, `@pattern
//! ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`).
//! So the value is a property of the IMAGE the reference names, not of the resource that
//! references it: one image reports ONE digest, to every operation and to every resource
//! naming it, and that digest is the one the registry holding the image reports for it — the
//! registry this emulator serves for an image pushed to it, and AWS's own for an image AWS
//! published ([`super::published_image_digest`]).
//!
//! Four ways the digest of a referenced image is known here, asked in this order:
//!
//! 1. the reference PINS it — a path in the documented `registry/repository[@digest]` format
//!    names the image by its content digest, so that digest IS the image's and nothing has
//!    to be resolved to know it ([`super::pinned_image_digest`]);
//! 2. Amazon ECR HOLDS the image — the registry's own store lives in the SAME shared engine,
//!    so the digest reported here is the one that registry minted for the manifest a client
//!    pushed, the very value its own `DescribeImages` and `BatchGetImage` report. The
//!    reference is read by Amazon ECR's OWN reading of a pull reference, so a path this
//!    service resolves and a path that registry serves are the same paths
//!    ([`registry_image_digest`]);
//! 3. AWS PUBLISHED the image — the path addresses one of the registries AWS publishes
//!    SageMaker's containers from, so the image is one AWS pushed once and every account
//!    pulls, and the digest reported for it is that published image's own, the same for every
//!    account and for every Region's copy of it ([`super::published_image_digest`]);
//! 4. nothing resolves one — no registry is mounted to ask, the path addresses another
//!    account's or another registry's host, or that repository holds nothing under the tag
//!    the path names. Then what is reported is this emulator's own identifier for the
//!    reference ([`derived_image_digest`]), which keeps the modelled form and every property
//!    a caller can check without a registry: one digest per reference, the same on every
//!    read, and practically never shared by two references.
//!
//! Resolved on the WRITE path, once, so the stored record is what every reader replays: a
//! registration PINS the image it was registered against, an image pushed to the same tag
//! afterwards does not change what an already-registered resource reports, and no read
//! changes what a later caller sees.

use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state as ecr_store;

use super::{pinned_image_digest, reportable_image_digest};
use crate::ops::types::{
    AdditionalInferenceSpecificationDefinition, InferenceSpecification,
    ModelPackageContainerDefinition, TrainingSpecification,
};

/// This emulator's OWN identifier for a container image reference, in the form
/// `ImageDigest` carries (`len 0..72`, pattern `^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`).
///
/// It is NOT a digest any registry resolved: it is what a reference nothing here holds an
/// image for is reported with. What it does keep is every property a caller can check
/// without a registry — the modelled form, one digest per reference, the same digest on
/// every read of it, and practically never a collision between two references — because it
/// is a SHA-256 over the reference itself and over nothing else, so it depends on neither
/// the clock nor the request that asked for it.
#[must_use]
pub fn derived_image_digest(image: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(image.as_bytes());
    format!("sha256:{}", ::hex::encode(digest))
}

/// The reference `image` makes INTO THIS REGISTRY, in the AWS-hosted spelling Amazon ECR's
/// own reading of a pull reference takes.
///
/// A registry is addressed by two spellings on this emulator, and both name the same
/// registry: AWS's own registry host (`<account>.dkr.ecr.<region>.<domain>`), and the
/// address this registry ANNOUNCES to a client that pushed an image here —
/// `Repository$repositoryUri` is "The URI for the repository. You can use this URI for
/// container image `push` and `pull` operations", and on this emulator that URI is built on
/// the endpoint the emulator answers at, because AWS's own registry host resolves nowhere on
/// the machine it runs on. So a path carrying the address a client pushed to is rewritten to
/// the AWS-hosted spelling and followed exactly like one that carries it already, rather
/// than parsed a second time here. One endpoint serves every Region, so such a reference
/// names no Region of its own and is resolved in the one the request was signed for.
pub(super) fn reference_into_this_registry<'image>(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &'image str,
) -> ::std::borrow::Cow<'image, str> {
    let Some((authority, path)) = image.split_once('/') else {
        return ::std::borrow::Cow::Borrowed(image);
    };
    if !addresses_this_emulator(ctx, authority) {
        return ::std::borrow::Cow::Borrowed(image);
    }
    let region = super::request_region(ctx);
    ::std::borrow::Cow::Owned(format!(
        "{}.dkr.ecr.{region}.amazonaws.com/{path}",
        ::emulator_server::EMULATOR_ACCOUNT_ID
    ))
}

/// Whether `authority` is an address THIS emulator answers at: the endpoint it is served at,
/// or the one the request itself reached it on. Both are read through the tree's own
/// readings of them (`service_common::data_plane_addressing`), and a host is
/// case-insensitive, so the address a client was handed is recognised however it is written
/// back.
fn addresses_this_emulator(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    authority: &str,
) -> bool {
    let served = ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx);
    if served.eq_ignore_ascii_case(authority) {
        return true;
    }
    let parts = ctx.parts();
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .is_some_and(|reached| reached.eq_ignore_ascii_case(authority))
}

/// The digest AMAZON ECR holds for the image `image` names, and `None` when that registry
/// states none.
///
/// The reference is read by Amazon ECR's own reading of a pull reference
/// (`state::registry_pull::parse_reference`), which answers only for a path addressing a
/// repository of THIS account's registry — the
/// `<account>.dkr.ecr.<region>.<domain>/<repository>[:<tag>|@<digest>]` form the Amazon ECR
/// User Guide gives for a pull, with the registry's default tag for a path naming neither
/// tag nor digest — after a path carrying the address this registry announces is brought
/// into that spelling ([`reference_into_this_registry`]). A path addressing anything else,
/// an engine carrying no registry, a repository that registry does not hold and a tag
/// nothing was pushed under all state nothing about the image, which is `None` rather than a
/// digest.
///
/// A READ of that registry: it resolves and reports, and changes nothing there. The digest
/// is held to the modelled `ImageDigest` form, because a value no member can carry is not
/// one this service reports ([`reportable_image_digest`]).
#[must_use]
pub fn registry_image_digest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &str,
) -> Option<String> {
    let addressed = reference_into_this_registry(ctx, image);
    let reference = ecr_store::registry_pull::parse_reference(&addressed)?;
    let registry = ecr_store::service_state(engine)?;
    let held = registry.lock();
    let repository = held
        .repository(&reference.region, &reference.repository_name)
        .ok()?;
    // Whichever way the reference addressed the image: a tag through the repository's own
    // tag index, a digest through the images it holds — so a digest that names nothing
    // pushed resolves to nothing.
    let digest = match (&reference.digest, &reference.tag) {
        (Some(digest), _) => repository
            .images
            .get(digest)
            .map(|image| image.digest.clone()),
        (None, Some(tag)) => repository.tag_index.get(tag).cloned(),
        (None, None) => None,
    };
    digest.filter(|digest| reportable_image_digest(digest))
}

/// The digest of the image `image` names, as every reader of the resource reports it: the
/// digest the reference pins, else the one Amazon ECR holds for it, else the one the image AWS
/// published carries, else this emulator's identifier for the reference — the order the module
/// docs state.
#[must_use]
pub fn resolved_image_digest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &str,
) -> String {
    if let Some(pinned) = pinned_image_digest(image) {
        return pinned.to_string();
    }
    registry_image_digest(engine, ctx, image)
        .or_else(|| super::published_image_digest(image).map(str::to_string))
        .unwrap_or_else(|| derived_image_digest(image))
}

/// The `DeployedImage` the container image reference `image` is REPORTED as, once the
/// resource holding it has resolved the image: the path as specified, that path at the
/// digest it resolves to, and the instant the resolution was made.
///
/// `DeployedImage`'s own members state each part: `SpecifiedImage` is "The image path you
/// specified when you created the model", `ResolvedImage` is "The specific digest path of the
/// image hosted in this `ProductionVariant`" — the `registry/repository[@digest]` form of the
/// same reference ([`super::image_at_digest`]) — and `ResolutionTime` is "The date and time
/// when the image path for the model resolved to the `ResolvedImage`"
/// (`smithy_query(query="shape",shape="DeployedImage")`).
///
/// Resolved on the WRITE path that records the reference, at the instant `at`, so every
/// reader replays one resolution: an image pushed to the same tag afterwards does not change
/// what an already-created resource reports, and no read changes what a later caller sees.
#[must_use]
pub fn deployed_image<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &str,
    at: ::std::time::SystemTime,
) -> crate::ops::types::DeployedImage {
    let digest = resolved_image_digest(engine, ctx, image);
    crate::ops::types::DeployedImage {
        specified_image: Some(image.to_string()),
        resolved_image: Some(super::image_at_digest(image, &digest)),
        resolution_time: Some(super::timestamp(at)),
    }
}

/// The images a MODEL deploys, as every variant hosting that model reports them: one
/// [`deployed_image`] per container of the model, the primary container first and then the
/// additional ones, in the order the model was created with.
///
/// A container that names no image path deploys none, so it contributes no entry.
#[must_use]
pub fn deployed_model_images<'container, Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    containers: impl IntoIterator<Item = &'container crate::ops::types::ContainerDefinition>,
    at: ::std::time::SystemTime,
) -> Vec<crate::ops::types::DeployedImage> {
    containers
        .into_iter()
        .filter_map(|container| container.image.as_deref())
        .map(|image| deployed_image(engine, ctx, image, at))
        .collect()
}

/// Fill in what SageMaker itself resolves for one container of an inference
/// specification: the digest of the image the container names ([`resolved_image_digest`],
/// which leaves a digest the request itself carried alone), and whether its model data
/// is a training checkpoint — which a request that says nothing leaves `false`.
///
/// Called on the WRITE path, so the stored record is the materialised container every
/// reader of it replays, rather than each reader defaulting again.
fn materialise_container<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    container: &mut ModelPackageContainerDefinition,
) {
    if container.image_digest.is_none() {
        container.image_digest = container
            .image
            .as_deref()
            .map(|image| resolved_image_digest(engine, ctx, image));
    }
    container.is_checkpoint = Some(container.is_checkpoint.unwrap_or(false));
}

/// [`materialise_container`] for every container of an inference specification.
pub fn materialise_inference_specification<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    specification: &mut InferenceSpecification,
) {
    for container in &mut specification.containers {
        materialise_container(engine, ctx, container);
    }
}

/// [`materialise_container`] for every container of an ADDITIONAL inference
/// specification, whose containers are the same shape and are resolved the same way.
pub fn materialise_additional_inference_specifications<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    specifications: &mut [AdditionalInferenceSpecificationDefinition],
) {
    for specification in specifications {
        for container in &mut specification.containers {
            materialise_container(engine, ctx, container);
        }
    }
}

/// Fill in what SageMaker itself resolves for an algorithm's training specification: the
/// digest of the training image ([`resolved_image_digest`], which leaves a digest the
/// request itself carried alone), and each training channel's `IsRequired` — which a
/// request that says nothing leaves `false`.
pub fn materialise_training_specification<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    specification: &mut TrainingSpecification,
) {
    if specification.training_image_digest.is_none() {
        specification.training_image_digest = Some(resolved_image_digest(
            engine,
            ctx,
            &specification.training_image,
        ));
    }
    for channel in &mut specification.training_channels {
        channel.is_required = Some(channel.is_required.unwrap_or(false));
    }
}
