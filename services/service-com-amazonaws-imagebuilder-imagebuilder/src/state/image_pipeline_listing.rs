// SPDX-License-Identifier: CC0-1.0

//! What a pipeline LISTING asks for, and which member of a stored pipeline one of its
//! filters selects on.
//!
//! `ListImagePipelines` is one of this service's paginated, filtered listings, so it needs
//! the same three facts every other one of them does: which operation a page token belongs
//! to ([`LIST_IMAGE_PIPELINES_OPERATION`]), how large one page may be
//! ([`super::MAX_LIST_PAGE_SIZE`]), and which of its own members the listing filters on
//! ([`IMAGE_PIPELINE_FILTER_NAMES`]) \u2014 a filter naming any other member is refused rather
//! than ignored, which is what keeps a listing from answering with pipelines the caller did
//! not ask for.

use crate::ops::types::Filter;

use super::ImagePipelineRecord;

/// The operation a pipeline listing's page token is issued for. See
/// [`super::LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_IMAGE_PIPELINES_OPERATION: &str = "ListImagePipelines";

/// The members `ListImagePipelines` filters on, as its own `filters` member documents them
/// (`smithy_query(query="doc",shape="ListImagePipelinesRequest",member="filters")`: "Use
/// the following filters to streamline results: description,
/// distributionConfigurationArn, imageRecipeArn, infrastructureConfigurationArn, name,
/// status").
///
/// Held in the order this operation's own refusal STATES them, which is the order AWS's
/// recorded unsupported-filter refusal of it lists them in rather than the alphabetical order
/// the member's documentation happens to use (as [`super::COMPONENT_FILTER_NAMES`] is);
/// membership is what every other reader asks of this set, and order does not change it.
pub const IMAGE_PIPELINE_FILTER_NAMES: &[&str] = &[
    "name",
    "description",
    "status",
    "imageRecipeArn",
    "infrastructureConfigurationArn",
    "distributionConfigurationArn",
];

/// Why the PIPELINE listing refuses a filter naming a member it does not filter on.
///
/// This service's own parameter-value sentence
/// ([`crate::service::parameter_refusal`]) about the parameter the refused value sat at — the
/// failing element's `name` member ([`super::filter_member_parameter`]) — whose requirement
/// reports that the name is not one this listing filters on and lists the ones it does, in the
/// order this operation states them ([`IMAGE_PIPELINE_FILTER_NAMES`]). The name the request
/// carried is the refused VALUE, so it is withheld like every other refused value.
///
/// The other listings of this service state the same class in their own sentence
/// ([`super::unsupported_filter_message`]), which names the `filters` member and the
/// operation whose set it is: each listing's refusal is the one AWS's recording of THAT
/// operation words.
#[must_use]
pub fn unsupported_filter_name_message(parameter: &str, supported: &[&str]) -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        parameter,
        &format!("Unsupported filter name. Allowed: {}", supported.join(", ")),
    )
}

/// Why the PIPELINE listing reports a filter that left a member out, under the
/// missing-parameter class every such report of this service carries
/// ([`super::MISSING_PARAMETERS_CODE`]).
///
/// This listing addresses the omitted member by its PATH through the request — the element and
/// the member of it ([`super::filter_member_parameter`]) — and states nothing else, which is
/// how AWS's own recorded report of this operation reads: a request carrying several filters is
/// told which one left the member out, which the member name alone cannot say. The other
/// listings of this service report the same class in the sentence every parameter their model
/// cannot require is reported in ([`super::missing_filter_member_message`]), which is the
/// wording their own recorded reports carry.
#[must_use]
pub fn missing_filter_parameter_message(parameter: &str) -> String {
    format!("Required parameters are missing: {parameter}")
}

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
