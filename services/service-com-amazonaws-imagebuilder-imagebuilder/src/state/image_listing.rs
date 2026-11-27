// SPDX-License-Identifier: CC0-1.0

//! What an image LISTING asks for, and what one of its entries stands for.
//!
//! `ListImages` is one of this service's paginated, filtered listings, so it needs the same
//! facts every other one of them does: which operation a page token belongs to
//! ([`LIST_IMAGES_OPERATION`]), how large one page may be ([`super::MAX_LIST_PAGE_SIZE`]), which
//! members the listing filters on ([`super::IMAGE_FILTER_NAMES`]) and which listing a token
//! was issued for ([`image_query_fingerprint`]).
//!
//! What it reports is one entry per image VERSION — "the list of image semantic versions"
//! — or one per NAME, with "all versions of each image aggregated" (the modelled `byName`
//! member). The store holds one record per BUILD version, so the entries are the stored
//! build versions GROUPED under the ARN the entry reports, read from the greatest build
//! version in the group ([`ImageBuildVersionRecord::version_order`]) — the same build a
//! reference to that version or name resolves to.
//!
//! The listing reaches two inventories, as the modelled `owner` names them: the images this
//! ACCOUNT built, which are the records of the build store, and the ones AWS publishes,
//! which are this service's catalog ([`super::MANAGED_IMAGES`]) and are reported by
//! [`super::ManagedImage::version_entry`].

use crate::ops::types::{Filter, ImageVersion};

use super::{
    normalized_name, parse_semantic_version, ImageBuildVersionRecord, ARN_SERVICE,
    ASSIGNED_VERSION_NODES, IMAGE_ARN_PREFIX, OWNERSHIP_SELF, WILDCARD_VERSION,
};

/// The operation an image listing's page token is issued for. See
/// [`super::LIST_IMAGE_RECIPES_OPERATION`].
pub const LIST_IMAGES_OPERATION: &str = "ListImages";

/// The filter names `ListImages`' own refusal STATES, in the order it states them.
///
/// The SET is [`super::IMAGE_FILTER_NAMES`], the `filters` member's own list, which this
/// operation shares with `ListImageBuildVersions` — membership is what every other reader
/// asks of it, and order does not change it. The ORDER is this operation's own: AWS's
/// recorded unsupported-filter refusal of `ListImages` lists the names in it rather than in
/// the alphabetical order the member's documentation happens to use, exactly as
/// [`super::COMPONENT_FILTER_NAMES`] does for the component listing. The operations whose own
/// refusal no recording of this service words state the documented order.
pub const LIST_IMAGES_STATED_FILTER_NAMES: &[&str] =
    &["name", "version", "platform", "osVersion", "type"];

/// The ARN of the image `name` names at `version` in `region`, carrying no build number: the
/// VERSION ARN of three assigned nodes, or the name ARN when `version` is
/// [`WILDCARD_VERSION`]. See [`super::component_version_arn`], the same ARN of the other
/// resource this service versions twice over.
#[must_use]
pub fn image_version_arn(region: &str, name: &str, version: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{IMAGE_ARN_PREFIX}{}/{version}", normalized_name(name)),
    )
}

/// The partition a listing reports an image of AWS's own catalog in.
///
/// A READ reaches one of those images through a reference that names the partition, so it
/// reports that partition back ([`super::managed_image`]); a listing starts from no
/// reference, so it reports the partition every ARN this emulator mints carries, read off
/// one rather than written here a second time.
#[must_use]
pub fn listing_partition(region: &str) -> String {
    let minted = image_version_arn(region, "listing", WILDCARD_VERSION);
    ::service_common::arn::parse_of(&minted, ARN_SERVICE)
        .map(|arn| arn.partition.to_string())
        .unwrap_or_default()
}

/// A deterministic rendering of which images one `ListImages` request asked for, so a token
/// issued for one query is refused for another.
///
/// `byName` is part of it: it decides whether an entry stands for one version or for a whole
/// name, so the two listings are keyed differently and a token issued for one cannot be
/// spent on the other. `maxResults` is deliberately NOT part of it, for the reason
/// [`super::image_recipe_query_fingerprint`] states, and neither is `includeDeprecated`: it
/// selects whether DEPRECATED Amazon-managed images are included, and no image this service
/// reports carries a deprecation time, so it names the same listing either way.
#[must_use]
pub fn image_query_fingerprint(
    owner: Option<&str>,
    by_name: bool,
    filters: Option<&[Filter]>,
) -> String {
    format!(
        "owner={};byName={by_name};filters={};",
        super::text(Some(owner.unwrap_or(OWNERSHIP_SELF))),
        super::filter_list(filters)
    )
}

impl ImageBuildVersionRecord {
    /// The four version nodes this build version sorts by: the three assigned ones and the
    /// build number, so the latest build of a version — or of a name — is the greatest of
    /// them. See [`super::ComponentRecord::version_order`].
    #[must_use]
    pub fn version_order(&self) -> [u64; ASSIGNED_VERSION_NODES + 1] {
        let [major, minor, patch] = parse_semantic_version(&self.version).unwrap_or_default();
        [major, minor, patch, self.build_version]
    }

    /// The version a LISTING entry this record stands for reports: its own three assigned
    /// nodes, or [`WILDCARD_VERSION`] when the entry aggregates every version of one name.
    ///
    /// Never the build number: an entry stands for a version or for a name, and the shape it
    /// is reported as admits the three assigned nodes alone (`ImageVersion`'s `version`
    /// targets `VersionNumber`, `^[0-9]+\.[0-9]+\.[0-9]+$`), unlike a build version's own
    /// ([`ImageBuildVersionRecord::semantic_version`]).
    #[must_use]
    pub fn listed_version(&self, aggregated_by_name: bool) -> &str {
        if aggregated_by_name {
            WILDCARD_VERSION
        } else {
            self.version.as_str()
        }
    }

    /// The ARN a LISTING entry this record stands for reports: the version ARN of its three
    /// assigned nodes, or the name ARN when the entry aggregates every version of one name.
    #[must_use]
    pub fn listed_arn(&self, aggregated_by_name: bool) -> String {
        image_version_arn(
            &self.region,
            &self.name,
            self.listed_version(aggregated_by_name),
        )
    }

    /// This record as the modelled `ImageVersion` a listing reports for the version, or for
    /// the name, its entry stands for.
    #[must_use]
    pub fn version_entry(&self, aggregated_by_name: bool) -> ImageVersion {
        ImageVersion {
            arn: Some(self.listed_arn(aggregated_by_name)),
            name: Some(self.name.clone()),
            r#type: self.image_type.clone(),
            version: Some(self.listed_version(aggregated_by_name).to_string()),
            platform: self.platform.clone(),
            // The operating system of the base image the build this entry is read from ran
            // against, as that build recorded it ([`ImageBuildVersionRecord::summary`]).
            os_version: self.os_version.clone(),
            owner: Some(::engine::EMULATOR_ACCOUNT_ID.to_string()),
            date_created: Some(self.date_created.clone()),
            build_type: self.build_type.clone(),
            // "The origin of the base image that Image Builder used to build this image": a
            // recipe's `parentImage` is caller-supplied text that nothing here can place in
            // that enum, so none is reported — as [`ImageBuildVersionRecord::summary`] states.
            image_source: None,
        }
    }

    /// Whether every filter of `filters` admits this record, so a listing selects it
    /// ([`image_filters_admit`]), read through this record's one reading of a filterable
    /// member ([`ImageBuildVersionRecord::filterable`]).
    #[must_use]
    pub fn admitted_by(&self, filters: Option<&[Filter]>) -> bool {
        image_filters_admit(filters, |name| {
            self.filterable(name).map(ToString::to_string)
        })
    }
}

/// Whether every filter of `filters` admits an entry that reports `reported` for the member
/// a filter names.
///
/// A filter admits an entry when one of its values IS the value the entry reports for that
/// member — "Filter values are case-sensitive" (the modelled `values`) — so an entry that
/// reports none for the member a filter names is admitted by no value of it. The ONE reading
/// of an image listing's filters, so the images an account built and the ones AWS publishes
/// are selected by the same rule.
#[must_use]
pub fn image_filters_admit(
    filters: Option<&[Filter]>,
    reported: impl Fn(&str) -> Option<String>,
) -> bool {
    filters.into_iter().flatten().all(|filter| {
        let reported = filter.name.as_deref().and_then(&reported);
        filter
            .values
            .iter()
            .flatten()
            .any(|value| reported.as_deref() == Some(value.as_str()))
    })
}
