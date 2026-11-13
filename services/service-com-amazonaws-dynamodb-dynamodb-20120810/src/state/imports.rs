// SPDX-License-Identifier: CC0-1.0

//! The import catalogue: the records `ImportTable` writes and the import readers report.
//!
//! An import is a RESOURCE OF ITS OWN rather than part of a table — it is addressed by
//! its own ARN and it outlives the table it wrote into — so it lives beside [`super`]
//! rather than inside it, like the export catalogue next door; `super` re-exports
//! everything here, so every caller still names it `crate::state::…`.

use super::*;

/// How far back a listing of imports reaches: "Lists completed imports within the past
/// 90 days" (the `ListImports` doc), so a record older than that window is no longer
/// reported. Measured on every read from the instant the import finished, and from the
/// instant it began while it is still running, so nothing sleeps and no read writes.
pub const IMPORT_HISTORY_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The `ImportStatus` an import task is in while it is still running: "Imports table data
/// from an S3 bucket" (the `ImportTable` doc) is asynchronous, so a caller that reads
/// straight after the request still sees the task running.
pub const IMPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ImportStatus` an import task reports once its span has elapsed and the data it
/// read is in the table.
pub const IMPORT_STATUS_COMPLETED: &str = "COMPLETED";

/// The `ImportStatus` of an import whose source could not be read at all, which the
/// record reports together with the reason once its span has elapsed.
pub const IMPORT_STATUS_FAILED: &str = "FAILED";

/// How long a `ClientToken` stands for the import it first started: "A client token is
/// valid for 8 hours after the first request that uses it is completed. After 8 hours,
/// any request with the same client token is treated as a new request"
/// (`ImportTable$ClientToken`). Measured on every read from the instant the import was
/// requested, so nothing sleeps and no read writes.
pub const IMPORT_CLIENT_TOKEN_WINDOW: Duration = Duration::from_secs(8 * 60 * 60);

/// The `ImportConflictException` sentence a `ClientToken` replayed with a CHANGED
/// parameter is refused with: "There was a conflict when importing from the specified S3
/// source. This can occur when the current import conflicts with a previous import
/// request that had the same client token" (`ImportConflictException`).
///
/// Fixed text with no variable part: the refusal states that a duplicate request with
/// conflicting parameters was detected, and names neither the token nor the parameter
/// that changed.
pub const IMPORT_CONFLICT_MESSAGE: &str =
    "Import conflict: Duplicate request detected with conflicting parameters";

/// The `InputCompressionType` an import that names none reads its source with: "Type of
/// compression to be used on the input coming from the imported table"
/// (`ImportTable$InputCompressionType`) — a source named with no compression is read as
/// the bytes it holds, which is the `NONE` the member declares.
pub const DEFAULT_INPUT_COMPRESSION_TYPE: &str = "NONE";

/// The `InputFormat` whose source files carry Amazon Ion, which no import reads yet.
pub const INPUT_FORMAT_ION: &str = "ION";

/// The `InputFormat` whose source files carry one DynamoDB JSON item per line — the form
/// a table export writes ([`exported_objects`]).
pub const INPUT_FORMAT_DYNAMODB_JSON: &str = "DYNAMODB_JSON";

/// The `InputFormat` whose source files carry comma-separated values.
pub const INPUT_FORMAT_CSV: &str = "CSV";

/// The most imports this emulator will hold IN ONE REGION.
///
/// This emulator's own ceiling, like the export catalogue's
/// ([`MAX_EXPORTS_PER_REGION`]): it is the per-region table quota over again
/// ([`MAX_TABLES`]), which keeps the catalogue of one region bounded by the number of
/// tables that region may hold — and an import creates a table, so it cannot outrun that
/// quota by much anyway. Reaching it answers the modelled `LimitExceededException` rather
/// than growing without end.
pub const MAX_IMPORTS_PER_REGION: usize = MAX_TABLES;

/// A new import's `ImportArn`:
/// `arn:aws:dynamodb:<region>:<account>:table/<table>/import/<millis>-<id>`, built
/// through the same core ARN helper every other identifier here is minted with
/// ([`table_arn`]), in the same construction as [`new_backup_arn`].
///
/// The last segment is the instant the import was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix: the instant alone does not identify an
/// import, because two calls can land in the same millisecond. Minted ONCE, when the
/// import is recorded, and stored on it — and because the fixed-width instant sits
/// inside it, ascending ARN order IS ascending (table, request instant) order, which is
/// the order a listing reports and the `NextToken` cursor pages through.
pub fn new_import_arn(region: &str, table: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "table/{table}/import/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// Why an import task could not read its source: "Status code for the failed import"
/// (`ImportTableDescription$FailureCode`) and "The error message corresponding to the
/// failure code" (`$FailureMessage`), recorded on the import when its source could not be
/// read so that every reader reports the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    pub code: String,
    pub message: String,
}

/// The `FailureCode` of an import whose source bucket the object store holds none of,
/// with the sentence S3 states for that condition — the same condition an export whose
/// destination bucket is missing reports.
pub const IMPORT_FAILURE_NO_SUCH_BUCKET: &str = EXPORT_FAILURE_NO_SUCH_BUCKET;
pub const IMPORT_FAILURE_NO_SUCH_BUCKET_MESSAGE: &str = EXPORT_FAILURE_NO_SUCH_BUCKET_MESSAGE;

/// The `FailureCode` of an import whose TARGET name already belongs to a table: the
/// condition names exactly that — the table the import was to create is already there.
/// An import names its target in the request and only its TASK runs into the name being
/// taken, so this is reported on the record rather than answered to the caller.
pub const IMPORT_FAILURE_TABLE_ALREADY_EXISTS: &str = "TableAlreadyExists";

/// The `FailureCode` of an import whose source carried an item the table could not hold:
/// "Status code for the failed import" (`ImportTableDescription$FailureCode`). The source
/// was read and the items it carried were validated one by one, so the code names the
/// validation the items failed rather than anything about the source as a whole.
pub const IMPORT_FAILURE_ITEM_VALIDATION: &str = "ItemValidationError";

/// The sentence an import that could not write every item it read reports: fixed text with
/// no variable part — it states that some items failed validation and sends the caller to
/// the error log for which ones, and names neither an item nor a count (the count is the
/// `ErrorCount` member beside it).
pub const IMPORT_FAILURE_ITEM_VALIDATION_MESSAGE: &str =
    "Some of the items failed validation checks and were not imported. Please check \
     CloudWatch error logs for more details.";

/// The sentence an import whose target name is already a table's reports: the one this
/// service states for a name that is already taken, built from the name the request
/// asked the new table to be created under.
pub fn table_already_exists_message(table: &str) -> String {
    format!("Table already exists: {table}")
}

/// What ONE `ImportTable` request asked for beyond the two members the record already
/// carries at its top level, and what the task made of it.
///
/// Held on the record because every one of these members is reported by
/// `ImportTableDescription`: the table the import created, the token it was started
/// under, the source's format options and compression, the parameters the table was
/// created with, and the figures the task settled on. A listing reports none of them,
/// which is why they are grouped here rather than beside the members a summary reads.
#[derive(Debug, Clone, Default)]
pub struct ImportDetails {
    /// "The table id corresponding to the table created by the import task"
    /// (`ImportTableDescription$TableId`), as that table was minted with it.
    pub table_id: Option<String>,
    /// "The client token that was provided for the import task"
    /// (`ImportTableDescription$ClientToken`), or `None` where the request carried none.
    pub client_token: Option<String>,
    /// "Type of compression to be used on the input coming from the imported table"
    /// (`$InputCompressionType`), resolved to [`DEFAULT_INPUT_COMPRESSION_TYPE`] where the
    /// request named none.
    pub input_compression_type: Option<String>,
    /// "The format options for the data that was imported into the target table"
    /// (`$InputFormatOptions`), as the request stated them.
    pub input_format_options: Option<InputFormatOptions>,
    /// "The parameters for the new table that was imported into"
    /// (`$TableCreationParameters`), as the request stated them.
    pub table_creation_parameters: Option<TableCreationParameters>,
    /// "The total number of items processed from the source file"
    /// (`$ProcessedItemCount`): every item the task read, whether it landed or not.
    pub processed_item_count: i64,
    /// "The number of items successfully imported into the new table"
    /// (`$ImportedItemCount`).
    pub imported_item_count: i64,
    /// "The number of errors occurred on importing the source file into the target
    /// table" (`$ErrorCount`): the items the task read and could not write.
    pub error_count: i64,
    /// "The total size of data processed from the source file, in Bytes"
    /// (`$ProcessedSizeBytes`), or `None` for a task that read nothing at all.
    pub processed_size_bytes: Option<i64>,
    /// Why this import's task could not read its source, where it could not: such an
    /// import reports `FAILED` and this reason once its span has elapsed, rather than a
    /// completion it did not achieve.
    pub failure: Option<ImportFailure>,
}

/// One recorded import of table data: the source it read, the table it wrote into, and
/// how far it got.
///
/// An import is a RESOURCE OF ITS OWN rather than part of a table: it is addressed by its
/// own ARN, it outlives the table it imported into, and every member the import readers
/// report is read from here. The status a reader REPORTS is derived at every read from the
/// stored instants ([`Self::status`]), the way a table's and an export's are, so the
/// `IN_PROGRESS` -> terminal transition the clock owes needs no write.
#[derive(Debug, Clone)]
pub struct Import {
    /// The `ImportArn` minted for this import when it was recorded
    /// ([`new_import_arn`]), which is also the key the store holds it under.
    pub arn: String,
    /// The region this import lives in — the one the table it imported into lives in,
    /// which is the scope every import operation resolves inside.
    pub region: String,
    /// The name of the table this import wrote into. The `TableArn` a reader reports is
    /// minted from it and this region ([`table_arn`]), so one stored table identity
    /// cannot drift from the ARN the table reports for itself.
    pub table_name: String,
    /// The status this import's task SETTLES INTO: one of the `ImportStatus` values the
    /// model declares. A reader reports [`IMPORT_STATUS_IN_PROGRESS`] until the task's
    /// own instant has passed and this one afterwards ([`Self::status`]), so the value
    /// here is where the task ended up rather than what every read answers.
    pub status: String,
    /// "The S3 bucket that provides the source for the import"
    /// (`ImportTable$S3BucketSource`), as the request named it.
    pub s3_bucket_source: S3BucketSource,
    /// The log group the import wrote its errors to, where one was made for it
    /// (`ImportSummary$CloudWatchLogGroupArn`).
    pub cloud_watch_log_group_arn: Option<String>,
    /// "The format of the source data" (`ImportSummary$InputFormat`): one of the
    /// `InputFormat` values the model declares.
    pub input_format: String,
    /// "The time at which this import task began" (`ImportSummary$StartTime`).
    pub started_at: SystemTime,
    /// The instant this import task is due to end (`ImportSummary$EndTime`) — `None` for
    /// a task that never ends. Before it the import reports `IN_PROGRESS` and no end
    /// time; from it the status the task settled into, and this instant as its end.
    pub ended_at: Option<SystemTime>,
    /// What the request asked for beyond the members above, and what the task made of it
    /// ([`ImportDetails`]).
    pub details: ImportDetails,
}

impl Import {
    /// The `ImportStatus` at `now`: an import task runs for a span before the data it
    /// read is in the table, so a caller that reads straight after the request still sees
    /// it running, and only then the status the task settled into.
    pub fn status(&self, now: SystemTime) -> &str {
        match self.ended_at {
            Some(end) if now >= end => self.status.as_str(),
            _ => IMPORT_STATUS_IN_PROGRESS,
        }
    }

    /// "The time at which the creation of the table associated with this import task
    /// completed" (`ImportTableDescription$EndTime`) — `None` while the task is still
    /// running, so no reader reports an end for work that has not finished.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.ended_at.filter(|end| now >= *end)
    }

    /// This import as `ListImports` reports it, as it stands at `now`: every member read
    /// from the record.
    pub fn summary(&self, now: SystemTime) -> ImportSummary {
        ImportSummary {
            import_arn: Some(self.arn.clone()),
            import_status: Some(self.status(now).to_string()),
            table_arn: Some(table_arn(&self.region, &self.table_name)),
            s3_bucket_source: Some(self.s3_bucket_source.clone()),
            cloud_watch_log_group_arn: self.cloud_watch_log_group_arn.clone(),
            input_format: Some(self.input_format.clone()),
            start_time: Some(super::reported_date(self.started_at)),
            end_time: self.ended_at(now).map(super::reported_date),
        }
    }

    /// This import as the two single-import operations report it, as it stands at `now`.
    ///
    /// The figures a FINISHED task reports — what it processed, what it imported and the
    /// reason it did not — are reported only once the task has finished, as they are for
    /// an export; while it runs the counters stand at zero, because nothing has been
    /// counted yet.
    pub fn describe(&self, now: SystemTime) -> ImportTableDescription {
        let finished = self.ended_at(now).is_some();
        let failed = if finished {
            self.details.failure.as_ref()
        } else {
            None
        };
        ImportTableDescription {
            import_arn: Some(self.arn.clone()),
            import_status: Some(self.status(now).to_string()),
            table_arn: Some(table_arn(&self.region, &self.table_name)),
            table_id: self.details.table_id.clone(),
            client_token: self.details.client_token.clone(),
            s3_bucket_source: Some(self.s3_bucket_source.clone()),
            error_count: if finished { self.details.error_count } else { 0 },
            cloud_watch_log_group_arn: self.cloud_watch_log_group_arn.clone(),
            input_format: Some(self.input_format.clone()),
            input_format_options: self.details.input_format_options.clone(),
            input_compression_type: self.details.input_compression_type.clone(),
            table_creation_parameters: self.details.table_creation_parameters.clone(),
            start_time: Some(super::reported_date(self.started_at)),
            end_time: self.ended_at(now).map(super::reported_date),
            processed_size_bytes: if finished {
                self.details.processed_size_bytes
            } else {
                None
            },
            processed_item_count: if finished {
                self.details.processed_item_count
            } else {
                0
            },
            imported_item_count: if finished {
                self.details.imported_item_count
            } else {
                0
            },
            failure_code: failed.map(|failure| failure.code.clone()),
            failure_message: failed.map(|failure| failure.message.clone()),
        }
    }

    /// Whether this record still falls inside the window a listing reaches back over
    /// ([`IMPORT_HISTORY_WINDOW`]), measured at `now` against the instant the import
    /// finished — or, while it is still running, the instant it began.
    pub fn within_history_window(&self, now: SystemTime) -> bool {
        let marker = self.ended_at(now).unwrap_or(self.started_at);
        match now.duration_since(marker) {
            Ok(age) => age < IMPORT_HISTORY_WINDOW,
            // Recorded at or after `now`: inside any window measured back from it.
            Err(_) => true,
        }
    }

    /// Whether this import's `ClientToken` still stands for it at `now`
    /// ([`IMPORT_CLIENT_TOKEN_WINDOW`]), measured from the instant it was requested: past
    /// that window "any request with the same client token is treated as a new request".
    pub fn within_idempotency_window(&self, now: SystemTime) -> bool {
        match now.duration_since(self.started_at) {
            Ok(age) => age < IMPORT_CLIENT_TOKEN_WINDOW,
            Err(_) => true,
        }
    }

    /// What this import was REQUESTED with, in the one form two requests are compared in
    /// ([`import_request_signature`]).
    pub fn request_signature(&self) -> String {
        import_request_signature(
            &self.s3_bucket_source,
            &self.input_format,
            self.details.input_format_options.as_ref(),
            self.details.input_compression_type.as_deref(),
            self.details.table_creation_parameters.as_ref(),
        )
    }
}

/// What two `ImportTable` requests have to agree on for a replayed `ClientToken` to
/// answer the import it already started: EVERY member the request carries except the
/// token itself — the source, the format, its options, the compression and the parameters
/// the table is created with.
///
/// One rendering of those members, so the comparison is member for member however deeply
/// a member nests, and a request that changes any of them is the conflict the operation
/// models ([`IMPORT_CONFLICT_MESSAGE`]) rather than a second import. The members are
/// compared as the request STATED them, except the compression, which both sides resolve
/// to the value the member defaults to: a request naming `NONE` and one naming nothing
/// ask for the same import.
pub fn import_request_signature(
    s3_bucket_source: &S3BucketSource,
    input_format: &str,
    input_format_options: Option<&InputFormatOptions>,
    input_compression_type: Option<&str>,
    table_creation_parameters: Option<&TableCreationParameters>,
) -> String {
    format!(
        "{:?}",
        (
            &s3_bucket_source.s3_bucket,
            &s3_bucket_source.s3_bucket_owner,
            &s3_bucket_source.s3_key_prefix,
            input_format,
            input_format_options,
            input_compression_type.unwrap_or(DEFAULT_INPUT_COMPRESSION_TYPE),
            table_creation_parameters,
        )
    )
}

/// The `ImportNotFoundException` message an import-addressed operation answers an ARN it
/// holds no import for with: "The specified import was not found"
/// (`ImportNotFoundException`), in the sentence DynamoDB states for that class — fixed
/// text around the ONE variable part, the `ImportArn` the caller addressed, and nothing
/// about what the catalogue does hold.
pub fn import_not_found(import_arn: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Import not found: Import with arn: {import_arn} doesn't exist"
    ))
}

/// Hold an ARN-addressed IMPORT operation's `ImportArn` to the form such an ARN takes in
/// the scope the request itself was made in.
///
/// The same two-part rule [`check_export_arn`] states for an export ARN, over the form an
/// import carries
/// (`arn:<partition>:dynamodb:<region>:<account>:table/<table>/import/<id>`): the value
/// must be an ARN of this service naming an import, and it must address the partition,
/// account and REGION this request reached — which is exactly the prefix [`table_arn`]
/// mints for a table of this region. Anything else names no import whose existence could
/// be looked up, so it is an INPUT failure ([`RequestError::Validation`]) rather than a
/// missing resource, and only a well-formed, in-scope ARN reaches the catalogue.
pub fn check_import_arn(import_arn: &str, request_region: &str) -> Result<(), RequestError> {
    let invalid = || {
        RequestError::Validation(format!(
            "Invalid ImportArn: Invalid ImportArn provided as input {import_arn}"
        ))
    };
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and carries `/`, so the split stops at six.
    let fields: Vec<&str> = import_arn.splitn(6, ':').collect();
    let resource = match fields.as_slice() {
        ["arn", _partition, "dynamodb", _region, _account, resource] => *resource,
        _ => return Err(invalid()),
    };
    let (table, import) = resource
        .strip_prefix("table/")
        .and_then(|path| path.split_once("/import/"))
        .ok_or_else(invalid)?;
    if import.is_empty() || import.contains('/') {
        return Err(invalid());
    }
    let table = resolve_table_name(table).map_err(|_| invalid())?;
    if import_arn != format!("{}/import/{import}", table_arn(request_region, &table)) {
        return Err(invalid());
    }
    Ok(())
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Every import of one region, in ascending `ImportArn` order, which is the scope a
    /// listing reports and the order it pages through.
    pub fn imports_in<'s>(&'s self, region: &'s str) -> impl Iterator<Item = &'s Import> + 's {
        self.imports
            .values()
            .filter(move |import| import.region == region)
    }

    /// How many imports that region already holds — what [`MAX_IMPORTS_PER_REGION`] is
    /// measured against.
    pub fn import_count_in(&self, region: &str) -> usize {
        self.imports_in(region).count()
    }

    /// Record one import under the ARN minted for it.
    pub fn insert_import(&mut self, import: Import) {
        self.imports.insert(import.arn.clone(), import);
    }

    /// The import that `ImportArn` names, inside the region the request reached.
    ///
    /// An import of ANOTHER region is not addressable from this endpoint, so it is the
    /// same miss as one that was never started ([`import_not_found`]).
    pub fn import(&self, region: &str, import_arn: &str) -> Result<&Import, RequestError> {
        self.imports
            .get(import_arn)
            .filter(|import| import.region == region)
            .ok_or_else(|| import_not_found(import_arn))
    }

    /// The import a `ClientToken` still stands for at `now` in that region, if any —
    /// what makes a repeated request idempotent ([`IMPORT_CLIENT_TOKEN_WINDOW`]).
    ///
    /// Scoped to the region the request reached, like every other import lookup: a token
    /// names an import of the endpoint it was first used against.
    pub fn import_for_client_token(
        &self,
        region: &str,
        client_token: &str,
        now: SystemTime,
    ) -> Option<&Import> {
        self.imports.values().rev().find(|import| {
            import.region == region
                && import.details.client_token.as_deref() == Some(client_token)
                && import.within_idempotency_window(now)
        })
    }

    /// Whether an import is STILL RUNNING into that table of that region at `now`.
    ///
    /// What makes an import's target in use: the running import is the one creating that
    /// table, so a second import of the same target would be a second task writing it
    /// ("You tried to update a resource that was already being updated" —
    /// `ResourceInUseException`). An import that has settled holds nothing: its target is
    /// a table like any other from then on.
    pub fn import_in_progress_for_table(
        &self,
        region: &str,
        table: &str,
        now: SystemTime,
    ) -> bool {
        self.imports_in(region).any(|import| {
            import.table_name == table && import.status(now) == IMPORT_STATUS_IN_PROGRESS
        })
    }
}
