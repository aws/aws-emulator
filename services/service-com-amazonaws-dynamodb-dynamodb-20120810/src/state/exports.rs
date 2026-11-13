// SPDX-License-Identifier: CC0-1.0

//! The export catalogue: the records `ExportTableToPointInTime` writes and the export
//! readers report.
//!
//! An export is a RESOURCE OF ITS OWN rather than part of a table — it is addressed by
//! its own ARN and it outlives the table it read — so it lives beside [`super`] rather
//! than inside it, like the backup catalogue next door; `super` re-exports everything
//! here, so every caller still names it `crate::state::…`.

use super::*;

/// How far back a listing of exports reaches: "Lists completed exports within the past
/// 90 days" (the `ListExports` doc). The same span [`IMPORT_HISTORY_WINDOW`] carries,
/// named once and shared, so the two listings of this service cannot drift apart.
/// Measured on every read from the instant the export finished, and from the instant it
/// began while it is still running, so nothing sleeps and no read writes.
pub const EXPORT_HISTORY_WINDOW: Duration = IMPORT_HISTORY_WINDOW;

/// The `ExportStatus` an export task is in while it is still running: "Export can be in
/// one of the following states: IN_PROGRESS, COMPLETED, or FAILED"
/// (`ExportDescription$ExportStatus`), and an export runs asynchronously — "the table
/// export will be a snapshot of the table's state" taken after the call returns.
pub const EXPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ExportStatus` an export task reports once its span has elapsed and the snapshot
/// is in the bucket.
pub const EXPORT_STATUS_COMPLETED: &str = "COMPLETED";

/// The `ExportStatus` of an export whose task could not write its objects — the third
/// value the enum declares, which a record carrying a failure reports once its span has
/// elapsed.
pub const EXPORT_STATUS_FAILED: &str = "FAILED";

/// The `ExportType` of an export of the whole table at one point in time: "Valid values
/// are FULL_EXPORT or INCREMENTAL_EXPORT. The default value is FULL_EXPORT"
/// (`ExportTableToPointInTime$ExportType`).
pub const EXPORT_TYPE_FULL: &str = "FULL_EXPORT";

/// The `ExportType` of an export of only what changed between two points in time.
pub const EXPORT_TYPE_INCREMENTAL: &str = "INCREMENTAL_EXPORT";

/// The `ExportFormat` an export that names none is taken in: the first of the two values
/// the member declares, "Valid values for ExportFormat are DYNAMODB_JSON or ION"
/// (`ExportTableToPointInTime$ExportFormat`), which is the one AWS reports for a request
/// that leaves the member out.
pub const DEFAULT_EXPORT_FORMAT: &str = "DYNAMODB_JSON";

/// The other `ExportFormat` that member declares: Amazon Ion.
pub const EXPORT_FORMAT_ION: &str = "ION";

/// How long a `ClientToken` stands for the export it first started: "A client token is
/// valid for 8 hours after the first request that uses it is completed. After 8 hours,
/// any request with the same client token is treated as a new request"
/// (`ExportTableToPointInTime$ClientToken`). Measured on every read from the instant the
/// export was requested, so nothing sleeps and no read writes.
pub const EXPORT_CLIENT_TOKEN_WINDOW: Duration = Duration::from_secs(8 * 60 * 60);

/// The `ExportConflictException` sentence a `ClientToken` replayed with a CHANGED
/// parameter is refused with: "if you submit a request with the same client token but a
/// change in other parameters within the 8-hour idempotency window, DynamoDB returns an
/// ExportConflictException" (`ExportTableToPointInTime$ClientToken`).
///
/// Fixed text with no variable part: the refusal states that a duplicate request with
/// conflicting parameters was detected, and names neither the token nor the parameter
/// that changed.
pub const EXPORT_CONFLICT_MESSAGE: &str =
    "Export conflict: Duplicate request detected with conflicting parameters";

/// The `InvalidExportTimeException` sentence an `ExportTime` the export cannot be taken
/// at is refused with.
///
/// Fixed text with no variable part, and one sentence for every such instant: "you can
/// export data from any time within the point in time recovery window" (the
/// `ExportTableToPointInTime` doc), so an instant before the table existed, one in the
/// future and one that has fallen out of that window are all simply invalid — the refusal
/// states that and nothing about which of them it was.
pub const INVALID_EXPORT_TIME_MESSAGE: &str = "Export Time is invalid";

/// The most exports this emulator will hold IN ONE REGION.
///
/// This emulator's own ceiling, like the backup catalogue's ([`MAX_BACKUPS_PER_REGION`]):
/// it is the per-region table quota over again ([`MAX_TABLES`]), which keeps the
/// catalogue of one region bounded by the number of tables that region may hold. Reaching
/// it answers the modelled `LimitExceededException` rather than growing without end.
pub const MAX_EXPORTS_PER_REGION: usize = MAX_TABLES;

/// A new export's `ExportArn`:
/// `arn:aws:dynamodb:<region>:<account>:table/<table>/export/<millis>-<id>`, built
/// through the same core ARN helper every other identifier here is minted with
/// ([`table_arn`]), in the same construction as [`new_import_arn`].
///
/// The last segment is the instant the export was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix: the instant alone does not identify an
/// export, because two calls can land in the same millisecond. Minted ONCE, when the
/// export is recorded, and stored on it — and because the fixed-width instant sits
/// inside it, descending ARN order IS descending (table, request instant) order, which
/// is the "reverse alphanumeric order of ExportArn" a listing reports (the
/// `ListExports` doc) and the order its `NextToken` cursor pages through.
pub fn new_export_arn(region: &str, table: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "table/{table}/export/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// What one `ExportTableToPointInTime` request ASKED FOR, as the request stated it.
///
/// Held beside the record so a repeat under the same `ClientToken` can be told from a
/// changed one: "If you submit a request with the same client token but a change in other
/// parameters within the 8-hour idempotency window, DynamoDB returns an
/// ExportConflictException" (`$ClientToken`). So this carries the members as the REQUEST
/// carried them — `export_time` stays the `Option` it arrived as, since two calls that
/// both leave it out ask for the same export, while the instant each would resolve it to
/// moves with the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// "Time in the past from which to export table data" (`$ExportTime`), or `None`
    /// where the request named none.
    pub export_time: Option<SystemTime>,
    /// "The format for the exported data" (`$ExportFormat`), resolved to the default
    /// where the request named none ([`DEFAULT_EXPORT_FORMAT`]).
    pub export_format: String,
    /// "Choice of whether to execute as a full export or incremental export"
    /// (`$ExportType`), resolved to `FULL_EXPORT` where the request named none.
    pub export_type: String,
    /// "The name of the Amazon S3 bucket to export the snapshot to" (`$S3Bucket`).
    pub s3_bucket: String,
    /// "The ID of the Amazon Web Services account that owns the bucket the export will be
    /// stored in" (`$S3BucketOwner`).
    pub s3_bucket_owner: Option<String>,
    /// "The Amazon S3 bucket prefix to use as the file name and path of the exported
    /// snapshot" (`$S3Prefix`).
    pub s3_prefix: Option<String>,
    /// "Type of encryption used on the bucket where export data will be stored"
    /// (`$S3SseAlgorithm`).
    pub s3_sse_algorithm: Option<String>,
    /// "The ID of the KMS managed key used to encrypt the S3 bucket where export data
    /// will be stored" (`$S3SseKmsKeyId`).
    pub s3_sse_kms_key_id: Option<String>,
}

/// Why an export task did not write its data: "Status code for the result of the failed
/// export" (`ExportDescription$FailureCode`) and "Export failure reason description"
/// (`$FailureMessage`), recorded on the export when its write did not land so that every
/// reader reports the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFailure {
    pub code: String,
    pub message: String,
}

/// One recorded export of a table's data: the table it read, what the request asked for,
/// and how far the task has got.
///
/// An export is a RESOURCE OF ITS OWN rather than part of a table, for the reason
/// [`Import`] is: it is addressed by its own ARN, it outlives the table it was taken
/// from, and every member the export readers report is read from here. Its STATUS is not
/// stored: it is derived from the stored instants at every read ([`Self::status`]), the
/// way a table's is, so the `IN_PROGRESS` -> `COMPLETED` transition the clock owes needs
/// no write.
#[derive(Debug, Clone)]
pub struct Export {
    /// The `ExportArn` minted for this export when it was recorded
    /// ([`new_export_arn`]), which is also the key the store holds it under.
    pub arn: String,
    /// The region this export lives in — the one the table it read lives in, which is
    /// the scope every export operation resolves inside.
    pub region: String,
    /// The name of the table this export read. The `TableArn` an export request selects
    /// by is minted from it and this region ([`table_arn`]), so one stored table
    /// identity cannot drift from the ARN the table reports for itself.
    pub table_name: String,
    /// "Unique ID of the table that was exported" (`ExportDescription$TableId`), as the
    /// table carried it when the export was requested.
    pub table_id: String,
    /// "The client token that was provided for the export task"
    /// (`ExportDescription$ClientToken`), or `None` where the request carried none.
    pub client_token: Option<String>,
    /// What the request asked for, which is what a repeat under the same client token is
    /// compared against ([`ExportRequest`]).
    pub request: ExportRequest,
    /// "Point in time from which table data was exported"
    /// (`ExportDescription$ExportTime`): the instant the request named, or the newest
    /// restorable one where it named none.
    pub export_time: SystemTime,
    /// The instant this export task began (`ExportDescription$StartTime`).
    pub started_at: SystemTime,
    /// The instant this export task is due to finish — one [`STATUS_TRANSITION`] after it
    /// began. Before it the task is `IN_PROGRESS` and after it `COMPLETED`, and it is the
    /// `EndTime` a finished export reports.
    pub completes_at: SystemTime,
    /// "The number of items exported" (`ExportDescription$ItemCount`): the items the
    /// table held at the instant the export read it.
    pub item_count: i64,
    /// "The billable size of the table export" (`ExportDescription$BilledSizeBytes`): the
    /// bytes of item data the table held at that same instant.
    pub billed_size_bytes: i64,
    /// Why this export's task did not write its data, where it did not: an export whose
    /// objects did not land reports `FAILED` and this reason once its span has elapsed,
    /// rather than a completion it did not achieve.
    pub failure: Option<ExportFailure>,
}

impl Export {
    /// The export an `ExportTableToPointInTime` at `now` starts from `table`, over
    /// `exported` — the items that table held AT `export_time`
    /// ([`PointInTimeRecovery::items_at`]), which are the items this export writes.
    ///
    /// Everything the record reports of the TABLE is read off it here, at the instant of
    /// the request — its own id, and the items and item data of the point in time it is
    /// taken at — so the record stands on its own once the table moves on, and the figures
    /// it reports are the figures of the data it wrote.
    pub fn started(
        table: &Table,
        exported: &BTreeMap<ItemKey, Item>,
        request: ExportRequest,
        export_time: SystemTime,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        // The instant this export is recorded at, at the precision it is reported in, so
        // the instant in its ARN, the instant it reports and the instant a client can
        // send back are all the same one.
        let started_at = at_timestamp_precision(now);
        Self {
            arn: new_export_arn(&table.region, &table.name, started_at),
            region: table.region.clone(),
            table_name: table.name.clone(),
            table_id: table.id.clone(),
            client_token,
            request,
            export_time,
            started_at,
            completes_at: started_at + STATUS_TRANSITION,
            item_count: i64::try_from(exported.len()).unwrap_or(i64::MAX),
            billed_size_bytes: exported
                .values()
                .fold(0_i64, |total, item| total.saturating_add(item_size(item))),
            // Nothing has been written yet, so nothing has failed yet: the caller records
            // the reason here if the write does not land.
            failure: None,
        }
    }

    /// The `ExportStatus` at `now`, derived from the stored instants and whether the task's
    /// write landed: an export task runs for a span before its snapshot is in the bucket,
    /// so a caller that reads straight after the request still sees it running.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.completes_at {
            EXPORT_STATUS_IN_PROGRESS
        } else if self.failure.is_some() {
            EXPORT_STATUS_FAILED
        } else {
            EXPORT_STATUS_COMPLETED
        }
    }

    /// "The time at which the export task completed" (`ExportDescription$EndTime`) —
    /// `None` while the task is still running, so no reader reports an end for work that
    /// has not finished.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.completes_at).then_some(self.completes_at)
    }

    /// The export's own id: the last segment of its ARN, which is what names the folder
    /// the snapshot is written under ([`new_export_arn`]).
    pub fn export_id(&self) -> &str {
        self.arn.rsplit('/').next().unwrap_or(&self.arn)
    }

    /// "The name of the manifest file for the export task"
    /// (`ExportDescription$ExportManifest`) — the key of the summary manifest the finished
    /// snapshot was written to, under the path the export writes its objects beneath
    /// ([`export_object_prefix`]), so the member names an object the bucket really holds.
    ///
    /// `None` while the task is still running, and for one whose write did not land:
    /// there is no manifest in the bucket until the export has written one.
    pub fn export_manifest(&self, now: SystemTime) -> Option<String> {
        if self.ended_at(now).is_none() || self.failure.is_some() {
            return None;
        }
        Some(format!(
            "{}manifest-summary.json",
            export_object_prefix(self.request.s3_prefix.as_deref(), self.export_id())
        ))
    }

    /// This export as `ListExports` reports it, as it stands at `now`: every member read
    /// from the record.
    pub fn summary(&self, now: SystemTime) -> ExportSummary {
        ExportSummary {
            export_arn: Some(self.arn.clone()),
            export_status: Some(self.status(now).to_string()),
            export_type: Some(self.request.export_type.clone()),
        }
    }

    /// This export as the two single-export operations report it, as it stands at `now`.
    ///
    /// The figures a FINISHED export reports — its item count, its billable size and its
    /// manifest — are reported only once the task has finished writing them; while it
    /// runs the members are absent, as they are for work still in flight, and an export
    /// whose write did not land reports the reason instead.
    pub fn describe(&self, now: SystemTime) -> ExportDescription {
        let finished = self.ended_at(now).is_some();
        let wrote = finished && self.failure.is_none();
        let failed = finished.then_some(self.failure.as_ref()).flatten();
        ExportDescription {
            export_arn: Some(self.arn.clone()),
            export_status: Some(self.status(now).to_string()),
            start_time: Some(super::reported_date(self.started_at)),
            end_time: self.ended_at(now).map(super::reported_date),
            export_manifest: self.export_manifest(now),
            table_arn: Some(table_arn(&self.region, &self.table_name)),
            table_id: Some(self.table_id.clone()),
            export_time: Some(super::reported_date(self.export_time)),
            client_token: self.client_token.clone(),
            s3_bucket: Some(self.request.s3_bucket.clone()),
            s3_bucket_owner: self.request.s3_bucket_owner.clone(),
            s3_prefix: self.request.s3_prefix.clone(),
            s3_sse_algorithm: self.request.s3_sse_algorithm.clone(),
            s3_sse_kms_key_id: self.request.s3_sse_kms_key_id.clone(),
            // "Status code for the result of the failed export"
            // (`ExportDescription$FailureCode`): reported for an export whose task did
            // not write its data, and absent for one that did.
            failure_code: failed.map(|failure| failure.code.clone()),
            failure_message: failed.map(|failure| failure.message.clone()),
            export_format: Some(self.request.export_format.clone()),
            billed_size_bytes: wrote.then_some(self.billed_size_bytes),
            item_count: wrote.then_some(self.item_count),
            export_type: Some(self.request.export_type.clone()),
            // An incremental export is the only kind that carries one, and no stored
            // record is of that kind yet (the operation refuses it as unimplemented), so
            // there is none to report; the same holds for an export's filter criteria.
            incremental_export_specification: None,
            filter_specification: None,
        }
    }

    /// Whether this record still falls inside the window a listing reaches back over
    /// ([`EXPORT_HISTORY_WINDOW`]), measured at `now` against the instant the export
    /// finished — or, while it is still running, the instant it began.
    pub fn within_history_window(&self, now: SystemTime) -> bool {
        let marker = self.ended_at(now).unwrap_or(self.started_at);
        match now.duration_since(marker) {
            Ok(age) => age < EXPORT_HISTORY_WINDOW,
            // Recorded at or after `now`: inside any window measured back from it.
            Err(_) => true,
        }
    }

    /// Whether this export's `ClientToken` still stands for it at `now`
    /// ([`EXPORT_CLIENT_TOKEN_WINDOW`]), measured from the instant it was requested: past
    /// that window "any request with the same client token is treated as a new request".
    pub fn within_idempotency_window(&self, now: SystemTime) -> bool {
        match now.duration_since(self.started_at) {
            Ok(age) => age < EXPORT_CLIENT_TOKEN_WINDOW,
            Err(_) => true,
        }
    }
}

/// The `ExportNotFoundException` message an export-addressed operation answers an ARN it
/// holds no export for with: built from the request — the ARN the caller addressed — and
/// nothing about what the catalogue does hold.
pub fn export_not_found(export_arn: &str) -> RequestError {
    RequestError::NotFound(format!("Export not found: {export_arn}"))
}

/// Hold an ARN-addressed EXPORT operation's `ExportArn` to the form such an ARN takes in
/// the scope the request itself was made in.
///
/// The same two-part rule [`check_backup_arn`] states for a backup ARN, over the form an
/// export carries
/// (`arn:<partition>:dynamodb:<region>:<account>:table/<table>/export/<id>`): the value
/// must be an ARN of this service naming an export, and it must address the partition,
/// account and REGION this request reached — which is exactly the prefix [`table_arn`]
/// mints for a table of this region. Anything else names no export whose existence could
/// be looked up, so it is an INPUT failure ([`RequestError::Validation`]) rather than a
/// missing resource, and only a well-formed, in-scope ARN reaches the catalogue.
pub fn check_export_arn(export_arn: &str, request_region: &str) -> Result<(), RequestError> {
    let invalid = || {
        RequestError::Validation(format!(
            "Invalid ExportArn: Invalid ExportArn provided as input {export_arn}"
        ))
    };
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and carries `/`, so the split stops at six.
    let fields: Vec<&str> = export_arn.splitn(6, ':').collect();
    let resource = match fields.as_slice() {
        ["arn", _partition, "dynamodb", _region, _account, resource] => *resource,
        _ => return Err(invalid()),
    };
    let (table, export) = resource
        .strip_prefix("table/")
        .and_then(|path| path.split_once("/export/"))
        .ok_or_else(invalid)?;
    if export.is_empty() || export.contains('/') {
        return Err(invalid());
    }
    let table = resolve_table_name(table).map_err(|_| invalid())?;
    if export_arn != format!("{}/export/{export}", table_arn(request_region, &table)) {
        return Err(invalid());
    }
    Ok(())
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Every export of one region, in DESCENDING `ExportArn` order — "in reverse
    /// alphanumeric order of ExportArn" (the `ListExports` doc), which is the scope a
    /// listing reports and the order it pages through.
    pub fn exports_in<'s>(&'s self, region: &'s str) -> impl Iterator<Item = &'s Export> + 's {
        self.exports
            .values()
            .rev()
            .filter(move |export| export.region == region)
    }

    /// How many exports that region already holds — what [`MAX_EXPORTS_PER_REGION`] is
    /// measured against.
    pub fn export_count_in(&self, region: &str) -> usize {
        self.exports_in(region).count()
    }

    /// Record one export under the ARN minted for it.
    pub fn insert_export(&mut self, export: Export) {
        self.exports.insert(export.arn.clone(), export);
    }

    /// The export that `ExportArn` names, inside the region the request reached.
    ///
    /// An export of ANOTHER region is not addressable from this endpoint, so it is the
    /// same miss as one that was never started ([`export_not_found`]).
    pub fn export(&self, region: &str, export_arn: &str) -> Result<&Export, RequestError> {
        self.exports
            .get(export_arn)
            .filter(|export| export.region == region)
            .ok_or_else(|| export_not_found(export_arn))
    }

    /// The export that `ExportArn` names, inside the region the request reached, for a
    /// caller that must CHANGE it — the task's own outcome, once its write has landed.
    pub fn export_mut(
        &mut self,
        region: &str,
        export_arn: &str,
    ) -> Result<&mut Export, RequestError> {
        self.exports
            .get_mut(export_arn)
            .filter(|export| export.region == region)
            .ok_or_else(|| export_not_found(export_arn))
    }

    /// The export a `ClientToken` still stands for at `now` in that region, if any —
    /// what makes a repeated request idempotent ([`EXPORT_CLIENT_TOKEN_WINDOW`]).
    ///
    /// Scoped to the region the request reached, like every other export lookup: a token
    /// names an export of the endpoint it was first used against.
    pub fn export_for_client_token(
        &self,
        region: &str,
        client_token: &str,
        now: SystemTime,
    ) -> Option<&Export> {
        self.exports.values().rev().find(|export| {
            export.region == region
                && export.client_token.as_deref() == Some(client_token)
                && export.within_idempotency_window(now)
        })
    }
}
