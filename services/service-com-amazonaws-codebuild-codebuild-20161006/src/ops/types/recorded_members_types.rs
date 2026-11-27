// SPDX-License-Identifier: CC0-1.0

//! The shapes CodeBuild answers with members for that the published model does not
//! declare.
//!
//! Three response shapes carry a member the model snapshot in `smithy-model/` declares
//! nowhere, and that snapshot is kept exactly as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`), so those three are declared here instead of beside
//! the shapes rendered from it:
//!
//! * `BuildArtifacts$type` — the artifact destination type a build RECORD reports, in the
//!   token form the service puts on the wire ([`crate::state::wire_artifacts_type`]). The
//!   snapshot declares `BuildArtifacts` with the location and the checksums only.
//! * `ProjectEnvironment$customEntrypoint` — that the container a provisioned build ran in
//!   was started with CodeBuild's own entrypoint, the agent that runs the buildspec, rather
//!   than the image's. The snapshot declares the member on no shape at all.
//! * `DeleteProjectOutput$webhookDeletedStatus` — the disposition of the deleted project's
//!   webhook ([`crate::state::webhook_deleted_status`]). The snapshot declares
//!   `DeleteProjectOutput` with no members whatsoever.
//!
//! Each is generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every length, enum and required constraint is
//! captured exactly as the snapshot spells it, and adds the one AWS carries. Nothing about
//! the wire is hand-written: the member names, the shape ids and the constraints all come
//! from that file.
//!
//! The VALUES are not decided here: they are computed from the record being reported, in
//! `crate::state` and the handlers that read it.
//!
//! `tests/all/local_model_additions.rs` pins both halves — the members on the wire, and
//! the overlay against the snapshot it restates, shape by shape.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The output artifacts of a build RECORD: what a build or batch build reports for the
/// destination it ran with.
#[smithy_macros::smithy(shape = "com.amazonaws.codebuild#BuildArtifacts")]
#[derive(Debug, Clone, Default)]
pub struct BuildArtifacts {
    /// The member the snapshot does not declare: the destination's type in the token form
    /// the service reports it in, `None` for a record that resolved no destination.
    pub r#type: Option<String>,
    pub location: Option<String>,
    pub sha256sum: Option<String>,
    pub md5sum: Option<String>,
    pub override_artifact_name: Option<bool>,
    pub encryption_disabled: Option<bool>,
    pub artifact_identifier: Option<String>,
    pub bucket_owner_access: Option<String>,
}

/// The build environment of a project, and of every build and batch build that reports the
/// environment it was started with or ran in.
#[smithy_macros::smithy(shape = "com.amazonaws.codebuild#ProjectEnvironment")]
#[derive(Debug, Clone, Default)]
pub struct ProjectEnvironment {
    pub r#type: String,
    pub image: String,
    pub compute_type: String,
    pub compute_configuration: Option<ComputeConfiguration>,
    pub fleet: Option<ProjectFleet>,
    pub environment_variables: Option<Vec<EnvironmentVariable>>,
    pub privileged_mode: Option<bool>,
    pub certificate: Option<String>,
    pub registry_credential: Option<RegistryCredential>,
    pub image_pull_credentials_type: Option<String>,
    pub docker_server: Option<DockerServer>,
    pub host_kernel: Option<String>,
    /// The member the snapshot does not declare: set once an environment has been
    /// provisioned for the build, and absent on a configuration that was never run.
    pub custom_entrypoint: Option<bool>,
}

/// What `DeleteProject` answers with.
#[smithy_macros::smithy(shape = "com.amazonaws.codebuild#DeleteProjectOutput")]
#[derive(Debug, Clone, Default)]
pub struct DeleteProjectOutput {
    /// The member the snapshot does not declare: what the delete did to the project's
    /// webhook.
    pub webhook_deleted_status: Option<String>,
}
