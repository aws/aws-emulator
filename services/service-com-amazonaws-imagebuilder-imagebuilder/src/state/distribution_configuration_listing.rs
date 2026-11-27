// SPDX-License-Identifier: CC0-1.0

//! What a distribution-configuration LISTING asks for, which member one of its filters
//! selects on, and what one of its entries reports.
//!
//! `ListDistributionConfigurations` is one of this service's paginated, filtered listings,
//! so it needs the same facts every other one of them does: which operation a page token
//! belongs to ([`LIST_DISTRIBUTION_CONFIGURATIONS_OPERATION`]), how large one page may be
//! ([`super::MAX_LIST_PAGE_SIZE`]), which of its own members the listing
//! filters on ([`DISTRIBUTION_CONFIGURATION_FILTER_NAMES`]) — a filter naming any other
//! member is refused rather than ignored, which is what keeps a listing from answering with
//! configurations the caller did not ask for — and what one listed entry carries
//! ([`DistributionConfigurationRecord::summary`]).

use crate::ops::types::{DistributionConfigurationSummary, Filter};

use super::{reported_tags, DistributionConfigurationRecord};

/// The operation a distribution-configuration listing's page token is issued for. See
/// [`super::LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_DISTRIBUTION_CONFIGURATIONS_OPERATION: &str = "ListDistributionConfigurations";

/// The members `ListDistributionConfigurations` filters on, as its own `filters` member
/// documents them
/// (`smithy_query(query="doc",shape="ListDistributionConfigurationsRequest",member="filters")`:
/// "You can filter on name to streamline results").
pub const DISTRIBUTION_CONFIGURATION_FILTER_NAMES: &[&str] = &["name"];

impl DistributionConfigurationRecord {
    /// This record as the modelled `DistributionConfigurationSummary` one listed page entry
    /// carries: the members that shape names, read off the same stored record
    /// [`Self::view`] reports, so a listing and a read never disagree about one
    /// configuration.
    #[must_use]
    pub fn summary(&self) -> DistributionConfigurationSummary {
        DistributionConfigurationSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            date_created: Some(self.date_created.clone()),
            date_updated: self.date_updated.clone(),
            tags: Some(reported_tags(self.tags.as_ref())),
            // "A list of the Regions that the distribution configuration distributes images
            // to" (the modelled `regions` member): the target Region of each stored entry,
            // since "each entry defines how output images are distributed in one target
            // Region", in the order the stored entries carry them.
            regions: Some(
                self.distributions
                    .iter()
                    .map(|distribution| distribution.region.clone())
                    .collect(),
            ),
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record reports
    /// none. See [`DISTRIBUTION_CONFIGURATION_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "name" => Some(self.name.as_str()),
            _ => None,
        }
    }

    /// Whether the filter `filter_name` carrying `values` admits this record: one of its
    /// values IS the value the record reports for the member the filter names — "Filter
    /// values are case-sensitive" (the modelled `values`).
    #[must_use]
    pub fn admits_filter(&self, filter_name: &str, values: &[String]) -> bool {
        self.filterable(filter_name)
            .is_some_and(|reported| values.iter().any(|value| value == reported))
    }
}

/// A deterministic rendering of which configurations one `ListDistributionConfigurations`
/// request asked for, so a token issued for one query is refused for another.
///
/// `maxResults` is deliberately NOT part of it, for the reason
/// [`super::image_recipe_query_fingerprint`] states.
#[must_use]
pub fn distribution_configuration_query_fingerprint(filters: Option<&[Filter]>) -> String {
    format!("filters={};", super::filter_list(filters))
}
