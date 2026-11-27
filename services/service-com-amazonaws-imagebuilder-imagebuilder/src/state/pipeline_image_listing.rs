// SPDX-License-Identifier: CC0-1.0

//! What ONE PIPELINE's image listing asks for, and which stored build versions it reaches.
//!
//! `ListImagePipelineImages` "returns a list of images created by the specified pipeline"
//! (`smithy_query(query="operation",operation="ListImagePipelineImages")`), so it is the image
//! listing of ONE pipeline: it needs the same facts every other paginated listing of this
//! service does — which operation a page token belongs to
//! ([`LIST_IMAGE_PIPELINE_IMAGES_OPERATION`]), how large one page may be
//! ([`super::MAX_LIST_PAGE_SIZE`]), which members it filters on
//! ([`PIPELINE_IMAGE_FILTER_NAMES`]) and which listing a token was issued for
//! ([`pipeline_image_query_fingerprint`]).
//!
//! What it reports is one entry per stored BUILD version whose build a run of that pipeline
//! queued — the modelled `sourcePipelineArn` is "only present for images that a pipeline
//! execution created" — so the entries are read from the build store rather than held on the
//! pipeline ([`super::ComAmazonawsImagebuilderImagebuilderState::pipeline_images`]), and a
//! listing cannot drift from the runs the store holds.

use crate::ops::types::Filter;

use super::{ComAmazonawsImagebuilderImagebuilderState, ImageBuildVersionRecord};

/// The operation a pipeline-image listing's page token is issued for. See
/// [`super::LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_IMAGE_PIPELINE_IMAGES_OPERATION: &str = "ListImagePipelineImages";

/// The filter names `ListImagePipelineImages` accepts, from its own `filters` member's list
/// (`smithy_query(query="doc",shape="ListImagePipelineImagesRequest",member="filters")`: "Use
/// the following filters to streamline results: name, version") — a shorter list than the one
/// the account-wide image listings take ([`super::IMAGE_FILTER_NAMES`]), since this operation
/// states only these two, held in the order it documents them.
pub const PIPELINE_IMAGE_FILTER_NAMES: &[&str] = &["name", "version"];

/// A deterministic rendering of which images one `ListImagePipelineImages` request asked for,
/// so a token issued for one query is refused for another.
///
/// The pipeline is part of it: an entry stands for a build one pipeline's run created, so a
/// token issued for one pipeline cannot be spent on another. `maxResults` is deliberately NOT
/// part of it, for the reason [`super::image_recipe_query_fingerprint`] states.
#[must_use]
pub fn pipeline_image_query_fingerprint(
    image_pipeline_arn: &str,
    filters: Option<&[Filter]>,
) -> String {
    format!(
        "imagePipelineArn={};filters={};",
        super::text(Some(image_pipeline_arn)),
        super::filter_list(filters)
    )
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The stored build versions a run of the pipeline `arn` created, in the store's own key
    /// order, skipping the ones at or before `after` and keeping only those every filter of
    /// `filters` admits.
    ///
    /// The one reading of "images created by the specified pipeline": a build version names the
    /// pipeline whose execution created it, so the listing selects on that rather than on
    /// anything the pipeline record holds. Only the Region the request reached serves its own
    /// resources, exactly as every other read of this store does.
    pub fn pipeline_images<'store>(
        &'store self,
        arn: &'store str,
        request_region: &'store str,
        after: &'store str,
        filters: Option<&'store [Filter]>,
    ) -> impl Iterator<Item = &'store ImageBuildVersionRecord> + 'store {
        self.images
            .iter()
            .filter(move |(key, _)| key.as_str() > after)
            .map(|(_, record)| record)
            .filter(move |record| record.region == request_region)
            .filter(move |record| record.source_pipeline_arn.as_deref() == Some(arn))
            .filter(move |record| record.admitted_by(filters))
    }
}
