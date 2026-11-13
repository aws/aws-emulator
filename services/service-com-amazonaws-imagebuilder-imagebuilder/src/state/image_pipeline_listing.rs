// SPDX-License-Identifier: CC0-1.0

//! What a pipeline LISTING asks for, and which member of a stored pipeline one of its
//! filters selects on.
//!
//! `ListImagePipelines` is one of this service's paginated, filtered listings, so it needs
//! the same three facts every other one of them does: which operation a page token belongs
//! to ([`LIST_IMAGE_PIPELINES_OPERATION`]), how large one page may be
//! ([`MAX_IMAGE_PIPELINES_PER_PAGE`]), and which of its own members the listing filters on
//! ([`IMAGE_PIPELINE_FILTER_NAMES`]) \u2014 a filter naming any other member is refused rather
//! than ignored, which is what keeps a listing from answering with pipelines the caller did
//! not ask for.

use crate::ops::types::Filter;

use super::ImagePipelineRecord;

/// The operation a pipeline listing's page token is issued for. See
/// [`super::LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_IMAGE_PIPELINES_OPERATION: &str = "ListImagePipelines";

/// The most pipelines one page of a listing carries: the modelled `maxResults` maximum
/// (`smithy_query(query="operation",operation="ListImagePipelines")` reports
/// `range 1..25`), which is also what a request naming no page size gets.
pub const MAX_IMAGE_PIPELINES_PER_PAGE: usize = 25;

/// The members `ListImagePipelines` filters on, as its own `filters` member documents them
/// (`smithy_query(query="doc",shape="ListImagePipelinesRequest",member="filters")`: "Use
/// the following filters to streamline results: description,
/// distributionConfigurationArn, imageRecipeArn, infrastructureConfigurationArn, name,
/// status").
pub const IMAGE_PIPELINE_FILTER_NAMES: &[&str] = &[
    "description",
    "distributionConfigurationArn",
    "imageRecipeArn",
    "infrastructureConfigurationArn",
    "name",
    "status",
];

impl ImagePipelineRecord {
    /// The value of the member `filter_name` filters on, or `None` when this record
    /// reports none \u2014 a pipeline that holds a container recipe reports no
    /// `imageRecipeArn`, so a filter on that member does not select it. See
    /// [`IMAGE_PIPELINE_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "description" => self.description.as_deref(),
            "distributionConfigurationArn" => self.distribution_configuration_arn.as_deref(),
            "imageRecipeArn" => self.image_recipe_arn.as_deref(),
            "infrastructureConfigurationArn" => Some(self.infrastructure_configuration_arn.as_str()),
            "name" => Some(self.name.as_str()),
            "status" => Some(self.status.as_str()),
            _ => None,
        }
    }
}

/// A deterministic rendering of which pipelines one `ListImagePipelines` request asked for,
/// so a token issued for one query is refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`super::image_recipe_query_fingerprint`] states.
#[must_use]
pub fn image_pipeline_query_fingerprint(filters: Option<&[Filter]>) -> String {
    format!("filters={};", super::filter_list(filters))
}
