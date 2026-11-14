// SPDX-License-Identifier: CC0-1.0

//! The CREATE side of the Capacity Manager data export configurations: what one is created
//! from, the quota a create asks about, the bytes a stored configuration is charged, the
//! comparison a replayed `ClientToken` is judged on, and the one writer that stores one.
//!
//! "Creates a new data export configuration for EC2 Capacity Manager"
//! (`CreateCapacityManagerDataExport`): a configuration is a RESOURCE of the account, held in
//! [`ComAmazonawsEc2Amazonec2State::capacity_manager_data_exports`] as the modelled
//! [`CapacityManagerDataExportResponse`] shape `DescribeCapacityManagerDataExports` reports,
//! so every member the writer below sets round-trips without a field-by-field copy that can
//! silently drop one.
//!
//! One instant a configuration carries is not a member the caller chooses: "the timestamp
//! when the data export configuration was created"
//! (`CapacityManagerDataExportResponse$CreateTime`) is read off the clock at the create and
//! never again.
//!
//! The four `LatestDelivery*` members are facts of an export RUN rather than of the
//! configuration — "the status of the most recent export delivery", "the S3 URI of the most
//! recently delivered export file" — so a configuration no export has run for yet carries
//! none of them, which is what a fresh create stores and what the describe's filters already
//! read as "no values" ([`super::filter_timestamp_values`] and the describe's own
//! `filter_values`).
//!
//! The id SPACE of a configuration, the two refusals that id can draw, the count the account
//! reports and the removal path stay beside this crate's other id readers
//! ([`super::capacity_manager_data_export_id_is_well_formed`]); this module holds only what the
//! CREATE needs. A
//! SUBMODULE of [`crate::state`] for the same reason [`super::image_usage_reports`] is one,
//! and `pub use` keeps every name reachable as `crate::state::<name>`, which is where the
//! handler names it.

use std::time::SystemTime;

use crate::ops::types::{CapacityManagerDataExportResponse, Tag};
use crate::scaffolded::Scaffolded;

use super::{
    CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX, ComAmazonawsEc2Amazonec2State, same_tags,
    system_time_to_date_time, tag_list_bytes,
};

/// The `TagSpecification$ResourceType` a create may name when it tags the configuration it
/// creates: the `capacity-manager-data-export` member of the modelled `ResourceType` catalog
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`), which is the resource this operation makes.
pub const CAPACITY_MANAGER_DATA_EXPORT_TAGGABLE_RESOURCE: &str = "capacity-manager-data-export";

/// How many data export configurations this store holds before a create is refused.
///
/// CHOSEN: AWS publishes no quota for this resource, and a store that grows without bound is
/// not a store — the same reason [`super::MAX_IMAGE_USAGE_REPORTS`] exists. Well above the
/// number of configurations a test or a demo creates.
pub const MAX_CAPACITY_MANAGER_DATA_EXPORTS: usize = 1_000;

/// Refuse a create that would take the account past [`MAX_CAPACITY_MANAGER_DATA_EXPORTS`]:
/// HTTP 400 [`super::RESOURCE_LIMIT_EXCEEDED`], the code this service answers for a ceiling
/// with no code of its own.
pub fn capacity_manager_data_export_limit_exceeded<E>(held: usize) -> Scaffolded<E> {
    super::resource_limit_exceeded(format!(
        "This emulator holds {held} of at most {MAX_CAPACITY_MANAGER_DATA_EXPORTS} Capacity \
         Manager data export configurations, so it cannot store another one"
    ))
}

/// What one data export configuration is created from: the members the request chooses, each
/// already read by the operation that took them.
#[derive(Debug, Clone, Default)]
pub struct CapacityManagerDataExportSpec {
    /// "The name of the S3 bucket where the capacity data export files will be delivered"
    /// (`CreateCapacityManagerDataExportRequest$S3BucketName`).
    pub s3_bucket_name: String,
    /// "The S3 key prefix for the exported data files … If not specified, files are placed at
    /// the bucket root" (`$S3BucketPrefix`), so `None` for a request that names none.
    pub s3_bucket_prefix: Option<String>,
    /// "The frequency at which data exports are generated" (`$Schedule`), as the word out of
    /// the modelled `Schedule` catalog — a `&'static str`, which is what keeps the request's
    /// own text out of the store.
    pub schedule: &'static str,
    /// "The file format for the exported data" (`$OutputFormat`), out of the modelled
    /// `OutputFormat` catalog for the same reason.
    pub output_format: &'static str,
    /// "The tags to apply to the data export configuration" (`$TagSpecifications`).
    pub tags: Vec<Tag>,
}

impl CapacityManagerDataExportSpec {
    /// The configuration this spec asks for, as the modelled shape the store holds — with
    /// every member the SERVICE decides (the id, the creation instant, and the delivery a run
    /// would report) still unset.
    ///
    /// The one builder: the create charges [`capacity_manager_data_export_payload_bytes`] for
    /// exactly this shape, judges a replayed `ClientToken` against it
    /// ([`same_capacity_manager_data_export_request`]) and
    /// [`ComAmazonawsEc2Amazonec2State::insert_capacity_manager_data_export`] stores it, so
    /// the bytes asked for, the request compared and the row written can never drift apart.
    pub fn configuration(&self) -> CapacityManagerDataExportResponse {
        CapacityManagerDataExportResponse {
            capacity_manager_data_export_id: None,
            s3_bucket_name: Some(self.s3_bucket_name.clone()),
            s3_bucket_prefix: self.s3_bucket_prefix.clone(),
            schedule: Some(self.schedule.to_string()),
            output_format: Some(self.output_format.to_string()),
            create_time: None,
            // No export has run for a configuration that has just been created, so there is
            // no "most recent delivery" to report a status, a message, a location or an
            // instant for (see the module docs).
            latest_delivery_status: None,
            latest_delivery_status_message: None,
            latest_delivery_s3_location_uri: None,
            latest_delivery_time: None,
            tags: Some(self.tags.clone()),
        }
    }
}

/// The caller-chosen bytes one stored data export configuration holds: the bucket and key
/// prefix it names, its tags, and the `ClientToken` the token map keeps for it. The id, the
/// creation instant and the two catalog words are this crate's to decide, so they are
/// fixed-size.
///
/// The ONE definition of that charge, the way [`super::image_usage_report_payload_bytes`] is
/// for a report: the create asks it against the store's budget before the write and charges
/// exactly it, and
/// [`ComAmazonawsEc2Amazonec2State::remove_capacity_manager_data_export`] gives back the same
/// number, so a configuration that comes and goes leaves the budget where it found it.
pub fn capacity_manager_data_export_payload_bytes(
    export: &CapacityManagerDataExportResponse,
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(export.tags.as_deref().unwrap_or_default())
        .saturating_add(export.s3_bucket_name.as_deref().map_or(0, str::len))
        .saturating_add(export.s3_bucket_prefix.as_deref().map_or(0, str::len))
        .saturating_add(client_token.map_or(0, str::len))
}

/// Do two requests ask for the SAME data export configuration — every member a create takes
/// from the request, and nothing the service decides for itself (the id, the creation instant
/// and the delivery members)?
///
/// What an idempotency replay is judged on: "unique, case-sensitive identifier that you
/// provide to ensure the idempotency of the request"
/// (`CreateCapacityManagerDataExportRequest$ClientToken`) holds for a retry of the SAME
/// request, so a token reused for a different configuration names a configuration the caller
/// did not ask for, which EC2 refuses instead. The tags are compared the way every other
/// replay of this crate compares a tag list ([`same_tags`]); the modelled shape carries no
/// `PartialEq`, which is why the comparison is spelled out here once rather than in the
/// handler.
pub fn same_capacity_manager_data_export_request(
    stored: &CapacityManagerDataExportResponse,
    asked: &CapacityManagerDataExportResponse,
) -> bool {
    stored.s3_bucket_name == asked.s3_bucket_name
        && stored.s3_bucket_prefix == asked.s3_bucket_prefix
        && stored.schedule == asked.schedule
        && stored.output_format == asked.output_format
        && same_tags(
            stored.tags.as_deref().unwrap_or_default(),
            asked.tags.as_deref().unwrap_or_default(),
        )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another data export configuration fits inside the store's ceiling
    /// ([`MAX_CAPACITY_MANAGER_DATA_EXPORTS`]).
    pub fn has_capacity_manager_data_export_capacity(&self) -> bool {
        self.capacity_manager_data_exports.len() < MAX_CAPACITY_MANAGER_DATA_EXPORTS
    }

    /// Store one data export configuration created at `created_at`, and answer the
    /// `CapacityManagerDataExportId` it was minted with.
    ///
    /// The ONE writer, so what the describe reports and what the count answers is always what
    /// this left here: the id comes from the store's own mint
    /// ([`Self::next_id`] under [`CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX`], the prefix every
    /// reader of this id space reads), and the creation instant is read off `created_at`.
    /// Nothing is owed to the clock: a configuration AWS hands back carries no status of its
    /// own, and the deliveries its schedule names are runs rather than states of this
    /// resource (see the module docs).
    ///
    /// The bytes the record retains are NOT charged here: the create asks
    /// [`capacity_manager_data_export_payload_bytes`] against the store's budget before it
    /// calls this, the way every other create of this crate does.
    pub fn insert_capacity_manager_data_export(
        &mut self,
        spec: CapacityManagerDataExportSpec,
        created_at: SystemTime,
    ) -> String {
        let export_id = self.next_id(CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX);
        let mut export = spec.configuration();
        export.capacity_manager_data_export_id = Some(export_id.clone());
        export.create_time = Some(system_time_to_date_time(created_at));
        self.capacity_manager_data_exports
            .insert(export_id.clone(), export);
        export_id
    }
}
