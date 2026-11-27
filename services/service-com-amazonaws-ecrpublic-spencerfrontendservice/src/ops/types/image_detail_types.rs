// SPDX-License-Identifier: CC0-1.0

//! `ImageDetail`: an image of a public repository, as AWS describes it.
//!
//! This is the ONE `ImageDetail` type of this crate — the shape
//! `DescribeImagesResponse$imageDetails` reaches through `ImageDetailList` — so what it
//! declares is what `DescribeImages` reports for every image it pages over.
//!
//! It is declared here rather than beside `describe_images`'s other shapes because AWS
//! answers with one member the published model snapshot does not declare, and that
//! snapshot is kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `repositoryType` — the registry tier of the repository the image sits in, reported on
//!   every `ImageDetail` entry of a `DescribeImages` response. The snapshot declares
//!   neither the member nor a shape of that name.
//!
//! So the type is generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the eight members the
//! snapshot declares, with their own targets so every length and pattern constraint is
//! captured exactly as the snapshot spells it, and adds the ninth. Nothing about the wire
//! is hand-written: the member names, the shape id and the constraints all come from that
//! file.
//!
//! `tests/all/image_detail_repository_type.rs` pins both halves: the member on the wire,
//! and the snapshot's own member list, which this declaration must cover in full.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ecrpublic#ImageDetail")]
#[derive(Debug, Clone, Default)]
pub struct ImageDetail {
    pub registry_id: Option<String>,
    pub repository_name: Option<String>,
    pub image_digest: Option<String>,
    pub image_tags: Option<Vec<String>>,
    pub image_size_in_bytes: Option<i64>,
    pub image_pushed_at: Option<::aws_smithy_types::DateTime>,
    pub image_manifest_media_type: Option<String>,
    pub artifact_media_type: Option<String>,
    /// The member the snapshot does not declare: the tier of the repository that holds
    /// the image. Which tier this registry reports is
    /// [`crate::state::DEFAULT_REPOSITORY_TYPE`].
    pub repository_type: Option<String>,
}

/// The values the overlay's `RepositoryType` enum declares, in the form every generated
/// enum value list takes — the catalogue a reported tier must come from. Written out here
/// because the shape is declared in the overlay rather than in the snapshot;
/// `tests/all/image_detail_repository_type.rs` checks it against that file.
pub const REPOSITORY_TYPE_VALUES: &[&str] = &["FREE"];
