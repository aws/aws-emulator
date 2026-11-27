// SPDX-License-Identifier: CC0-1.0

//! The bulk-operation catalogue: the records `StartBulkPutItems` writes and the
//! bulk-operation readers report.
//!
//! A bulk operation is a RESOURCE OF ITS OWN rather than part of a table — it is addressed
//! by its own ARN and it outlives the writes it performed — so it lives beside [`super`]
//! rather than inside it, like the import and export catalogues next door; `super`
//! re-exports everything here, so every caller still names it `crate::state::…`.
//!
//! WHERE THE SHAPE OF THIS COMES FROM. The model this crate is built from declares the
//! bulk-operation shapes (`smithy_query(query="closure",
//! operation="StartBulkPutItems")`), so a record holds exactly what the two readers report:
//! the operation's own ARN, its status, the table it acted on and that table's id, the source
//! it read and the location it reports into, the instants it began and ended, and the figures
//! its task settled on — the items it processed, the ones it could not write, the source bytes
//! it read and the item bytes it wrote.

use super::*;

/// The `BulkOperationStatus` a bulk operation is in while it is still running: a bulk put
/// is asynchronous — the call answers an operation to watch rather than a result — so a
/// caller that reads straight after the request still sees it running.
pub const BULK_OPERATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `BulkOperationStatus` a bulk operation reports once its span has elapsed.
pub const BULK_OPERATION_STATUS_COMPLETED: &str = "COMPLETED";

/// The `BulkOperationStatus` of a bulk operation that read its source and could not write
/// every record it read: the status says exactly that — the work ran to the end of the
/// source and some of its items did not land, which is what `FailedItemCount` counts beside
/// it (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#BulkOperationStatus")` declares it).
pub const BULK_OPERATION_STATUS_COMPLETED_WITH_ERRORS: &str = "COMPLETED_WITH_ERRORS";

/// The `BulkOperationStatus` of a bulk operation whose source could not be read at all,
/// which it reports once its span has elapsed rather than a completion it did not achieve.
pub const BULK_OPERATION_STATUS_FAILED: &str = "FAILED";

/// The `BulkOperationStatus` a bulk operation reports while a cancel of it is being carried
/// out: stopping work takes a span exactly as doing it does, so a caller that reads straight
/// after asking for the cancel still sees it stopping rather than stopped.
pub const BULK_OPERATION_STATUS_CANCELLING: &str = "CANCELLING";

/// The `BulkOperationStatus` such an operation settles into once that span has elapsed.
pub const BULK_OPERATION_STATUS_CANCELLED: &str = "CANCELLED";

/// The resource type a bulk operation's ARN names, which is the one type a member that
/// addresses a bulk operation accepts ([`new_bulk_operation_arn`] mints it, and
/// [`resolve_bulk_operation_arn`] is what admits it).
pub const BULK_OPERATION_RESOURCE_TYPE: &str = "bulk-operation";

/// The service field a bulk operation's ARN carries: a bulk operation is a resource of THIS
/// service, so an ARN of any other service names none of them.
const BULK_OPERATION_ARN_SERVICE: &str = "dynamodb";

/// How AWS's refusals name the member a bulk-operation ARN arrives in: the lowerCamel path
/// its own request validator reports a violation of that member against.
pub const BULK_OPERATION_ARN_MEMBER_PATH: &str = "bulkOperationArn";

/// How a value that is not the ARN of a bulk operation is refused: FIXED text, naming
/// neither the member it arrived in nor the value it held — the whole message is this
/// sentence, because what is wrong is that the value is no bulk-operation ARN at all and no
/// one field of it can be pointed at.
pub const INVALID_BULK_OPERATION_ARN: &str = "Invalid Bulk operation ARN";

/// Admit `arn` as the ARN of a bulk operation, or refuse it with the one sentence above.
///
/// The form: `arn:<partition>:dynamodb:<region>:<account>:bulk-operation/<id>` — every
/// field stated and non-empty, the service field this service's own, the resource type a
/// bulk operation's ([`BULK_OPERATION_RESOURCE_TYPE`]) and a resource id after it. Read the
/// way this service reads every other ARN-shaped member (`super::ARN_PREFIX` and the
/// five colon-separated fields after it), so a value that is not an ARN, an ARN of another
/// service, and an ARN naming a resource of another kind are all the same defect: the value
/// addresses no bulk operation.
///
/// The LENGTH the member admits is not read here: the model declares it
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BulkOperationArn")` gives
/// `@length` 1..1024), so the framework's own validating walk refuses a value outside it in
/// the clause that states that bound, before any handler runs.
///
/// WHICH bulk operation it addresses is a question for the catalogue
/// ([`ComAmazonawsDynamodbDynamodb20120810State::bulk_operation_in`]), not for this: a
/// well-formed ARN the account holds no operation under is a resource that is not there
/// rather than a request that is malformed.
pub fn resolve_bulk_operation_arn(arn: &str) -> Result<(), RequestError> {
    let invalid = || RequestError::Validation(INVALID_BULK_OPERATION_ARN.to_string());
    let Some(rest) = arn.strip_prefix(ARN_PREFIX) else {
        return Err(invalid());
    };
    let fields: Vec<&str> = rest.splitn(5, ':').collect();
    let [partition, service, region, account, resource] = fields.as_slice() else {
        return Err(invalid());
    };
    if partition.is_empty() || region.is_empty() || account.is_empty() {
        return Err(invalid());
    }
    if *service != BULK_OPERATION_ARN_SERVICE {
        return Err(invalid());
    }
    match resource.split_once('/') {
        Some((resource_type, id))
            if resource_type == BULK_OPERATION_RESOURCE_TYPE && !id.is_empty() =>
        {
            Ok(())
        }
        _ => Err(invalid()),
    }
}

/// How a bulk-operation ARN the account holds no operation under is refused: the
/// `ResourceNotFoundException` sentence this service states for a resource that is not
/// there, naming the kind of resource and the one the request addressed — the form its
/// sibling catalogues state it in (`Requested resource not found: Table: <name> not found`).
pub fn bulk_operation_not_found(arn: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: Bulk operation: {arn} not found"
    ))
}

/// How a cancel of a bulk operation that has already finished is refused: stopping work
/// that is over changes nothing, so the request is refused rather than answered with a
/// cancellation that did not happen.
///
/// The sentence is this service's own — the recorded calls carry none for this case — and
/// states the operation addressed and the status it had already reached, which is what tells
/// the caller why the cancel could not be carried out.
pub fn bulk_operation_not_cancellable(arn: &str, status: &str) -> RequestError {
    RequestError::Validation(format!(
        "Bulk operation {arn} has already reached status {status} and cannot be cancelled"
    ))
}

/// How long a `ClientToken` stands for the bulk operation it first started.
///
/// The window every other idempotent start of this service measures a repeated token over
/// ([`IMPORT_CLIENT_TOKEN_WINDOW`]): a token is "valid for 8 hours after the first request
/// that uses it is completed. After 8 hours, any request with the same client token is
/// treated as a new request" (`ImportTable$ClientToken`, the one window this service's
/// model states for a client token). Measured on every read from the instant the operation
/// was requested, so nothing sleeps and no read writes.
pub const BULK_OPERATION_CLIENT_TOKEN_WINDOW: Duration = IMPORT_CLIENT_TOKEN_WINDOW;

/// The most bulk operations this emulator will hold IN ONE REGION.
///
/// This emulator's own ceiling, like the import catalogue's
/// ([`MAX_IMPORTS_PER_REGION`]): it keeps the catalogue of one region bounded by the number
/// of tables that region may hold, so a client cannot grow it without end. Reaching it
/// answers the modelled `LimitExceededException`.
pub const MAX_BULK_OPERATIONS_PER_REGION: usize = MAX_IMPORTS_PER_REGION;

/// A new bulk operation's ARN:
/// `arn:aws:dynamodb:<region>:<account>:bulkoperation/<millis>-<id>`, built through the
/// same core ARN helper every other identifier here is minted with.
///
/// ACCOUNT-SCOPED rather than nested under the table it acts on: a bulk operation is a
/// resource of the account's own bulk-operation namespace — the listing that reports them
/// reaches every table's, and the table each one acts on is a member of the record rather
/// than a segment of its name.
///
/// The last segment is the instant the operation was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix: the instant alone does not identify an
/// operation, because two calls can land in the same millisecond. Minted ONCE, when the
/// operation is recorded, and stored on it — and because the fixed-width instant sits
/// inside it, ascending ARN order IS ascending request-instant order, which is the order a
/// listing pages through.
pub fn new_bulk_operation_arn(region: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "{BULK_OPERATION_RESOURCE_TYPE}/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// What two `StartBulkPutItems` requests have to agree on for a replayed `ClientToken` to
/// answer the operation it already started: EVERY member the request carries except the
/// token itself — the table, the source it reads, the location it reports into, the
/// write-unit ceiling and the performance tier.
///
/// One rendering of those members, so the comparison is member for member however deeply a
/// member nests, and a request that changes any of them is a conflicting replay rather than
/// a second operation. The members are compared as the request STATED them, except the one
/// that RESOLVES — the source's compression, resolved to the value its member defaults to,
/// so a request naming `NONE` and one naming no compression ask for the same operation.
pub fn bulk_put_request_signature(
    table_name: &str,
    input: &InputS3Configuration,
    output: &OutputS3Configuration,
    max_write_request_units: Option<i64>,
    performance_tier: Option<&str>,
) -> String {
    format!(
        "{:?}",
        (
            table_name,
            &input.s3_bucket,
            &input.s3_bucket_owner,
            &input.s3_key_prefix,
            &input.input_format,
            input
                .input_compression_type
                .as_deref()
                .unwrap_or(DEFAULT_INPUT_COMPRESSION_TYPE),
            (
                &output.s3_bucket,
                &output.s3_bucket_owner,
                &output.s3_key_prefix,
                &output.s3_sse_algorithm,
                &output.s3_sse_kms_key_id,
                &output.output_type,
            ),
            max_write_request_units,
            performance_tier,
        )
    )
}

/// How a bulk operation's `ClientToken`, re-used with a request the token does not stand
/// for ([`bulk_put_request_signature`]), is refused.
///
/// The modelled error says what happened — "DynamoDB rejected the request because you
/// retried a request with a different payload but with an idempotent token that was already
/// used" (`IdempotentParameterMismatchException`) — and the bulk-operation surface states it
/// in this one fixed sentence, which names the token's window and nothing of the request
/// that was sent or of the operation the token stands for. The sentence belongs to THIS
/// surface: the transactional operations state their own
/// ([`super::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`]) for the same modelled error.
pub const BULK_OPERATION_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "Specified idempotent token was already used with different request parameters within the \
     idempotency window";

/// One recorded bulk operation: what it wrote, into which table, and how far it got.///
/// The status a reader REPORTS is derived at every read from the stored instants
/// ([`Self::status`]), the way an import's and a table's are, so the `IN_PROGRESS` ->
/// terminal transition the clock owes needs no write.
#[derive(Debug, Clone)]
pub struct BulkOperation {
    /// The ARN minted for this operation when it was recorded
    /// ([`new_bulk_operation_arn`]), which is also the key the store holds it under.
    pub arn: String,
    /// The region this operation lives in — the one the table it wrote into lives in,
    /// which is the scope every bulk-operation call resolves inside.
    pub region: String,
    /// The name of the table this operation wrote into, which a reader reports as the one
    /// entry of its `TableDetails` beside that table's own id.
    pub table_name: String,
    /// The `TableId` of that table, as the table was minted with it ([`Table`]'s `id`): "the
    /// name or ARN of the table associated with the bulk operation" and its id are what one
    /// `BulkOperationTableDetail` carries.
    pub table_id: String,
    /// The `ClientToken` the call that started it carried, or `None` where it carried
    /// none: what makes a repeated call answer this operation instead of starting a second
    /// one ([`BULK_OPERATION_CLIENT_TOKEN_WINDOW`]).
    pub client_token: Option<String>,
    /// The S3 location the call named as the source of its items, as it named it.
    pub input_s3_configuration: InputS3Configuration,
    /// The S3 location the call named for the operation's own outcome report — a member
    /// every request states (`StartBulkPutItems$OutputS3Configuration` is `@required`).
    pub output_s3_configuration: OutputS3Configuration,
    /// The write-unit ceiling the call asked the operation to run under, where it asked for
    /// one.
    pub max_write_request_units: Option<i64>,
    /// The performance tier the call asked the operation to run in, where it asked for one.
    pub performance_tier: Option<String>,
    /// "The number of input items that DynamoDB has processed"
    /// (`BulkOperationDescription$ProcessedItemCount`): every record the task READ, the ones
    /// it could not decode included.
    pub processed_item_count: i64,
    /// How many items LANDED in the table.
    pub written_item_count: i64,
    /// "The number of processed input items that failed because of parsing or validation
    /// errors" (`$FailedItemCount`): the records the task read and could not write.
    pub failed_item_count: i64,
    /// "The number of bytes that DynamoDB has read from the Amazon S3 input objects"
    /// (`$ProcessedInputBytes`), which is also the source's whole size (`$TotalInputBytes`):
    /// this emulator applies the whole source under the call that started it, so what was
    /// read is what there was.
    pub processed_input_bytes: i64,
    /// "The total number of bulk put bytes used by the operation"
    /// (`BulkOperationUsage$BulkPutBytes`): the item bytes the writes this operation made
    /// added to the table, as that table meters what it stores.
    pub bulk_put_bytes: i64,
    /// Why the task could not read its source at all, where it could not: the `FailureCode`
    /// and `FailureMessage` a FAILED operation reports, recorded on it so that every reader
    /// reports the same reason — in the one form this service records a staged source's
    /// failure in ([`ImportFailure`], which the import surface next door reports from).
    pub failure: Option<ImportFailure>,
    /// The instant the operation began, which a reader reports as its `StartTime`.
    pub started_at: SystemTime,
    /// The instant it is due to end, which is one transition span after it began
    /// ([`STATUS_TRANSITION`]). Before it the operation reports `IN_PROGRESS` and no end
    /// time; from it the status it settled into, and this instant as its end. A cancel moves
    /// it to one span after the cancel was asked for ([`Self::cancel`]).
    pub ended_at: SystemTime,
    /// The instant a CANCEL of this operation was asked for, or `None` where none was: what
    /// makes the span before [`Self::ended_at`] report `CANCELLING` rather than the
    /// `IN_PROGRESS` of an operation still doing its work.
    pub cancelled_at: Option<SystemTime>,
    /// The status it SETTLES INTO once that instant has passed: one of the
    /// `BulkOperationStatus` values the model declares.
    pub status: String,
}

impl BulkOperation {
    /// The `BulkOperationStatus` at `now`: a bulk operation runs for a span, so a caller
    /// that reads straight after the request still sees it running, and only then the
    /// status it settled into. An operation a cancel has been asked for reports `CANCELLING`
    /// over that span instead, because what it is doing until it stops is stopping.
    pub fn status(&self, now: SystemTime) -> &str {
        if now >= self.ended_at {
            self.status.as_str()
        } else if self.cancelled_at.is_some() {
            BULK_OPERATION_STATUS_CANCELLING
        } else {
            BULK_OPERATION_STATUS_IN_PROGRESS
        }
    }

    /// Whether this operation is still RUNNING at `now` — the one state a cancel can act on,
    /// since an operation whose span has elapsed has already reached its terminal status.
    pub fn is_running(&self, now: SystemTime) -> bool {
        now < self.ended_at
    }

    /// Stop this operation: from `now` it reports `CANCELLING`, and one transition span
    /// later the `CANCELLED` it settles into ([`STATUS_TRANSITION`]).
    ///
    /// IDEMPOTENT — an operation a cancel has already been asked for keeps the instant that
    /// cancel was asked at, so a repeated cancel does not push the instant it stops further
    /// out and two callers asking for one cancellation observe one.
    pub fn cancel(&mut self, now: SystemTime) {
        if self.cancelled_at.is_some() {
            return;
        }
        self.cancelled_at = Some(now);
        self.ended_at = now + STATUS_TRANSITION;
        self.status = BULK_OPERATION_STATUS_CANCELLED.to_string();
    }

    /// The instant this operation ENDED, or `None` while it is still running: no reader
    /// reports an end for work that has not finished.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        Some(self.ended_at).filter(|end| now >= *end)
    }

    /// Whether a repeated `ClientToken` still stands for this operation at `now`
    /// ([`BULK_OPERATION_CLIENT_TOKEN_WINDOW`]), measured from the instant it was
    /// requested.
    pub fn within_idempotency_window(&self, now: SystemTime) -> bool {
        now.duration_since(self.started_at)
            .is_ok_and(|elapsed| elapsed <= BULK_OPERATION_CLIENT_TOKEN_WINDOW)
    }

    /// What this operation was REQUESTED with, in the one form two requests are compared in
    /// ([`bulk_put_request_signature`]).
    pub fn request_signature(&self) -> String {
        bulk_put_request_signature(
            &self.table_name,
            &self.input_s3_configuration,
            &self.output_s3_configuration,
            self.max_write_request_units,
            self.performance_tier.as_deref(),
        )
    }

    /// This operation as `ListBulkOperations` reports it, as it stands at `now`: every
    /// member read from the record.
    pub fn summary(&self, now: SystemTime) -> BulkOperationSummary {
        BulkOperationSummary {
            bulk_operation_arn: self.arn.clone(),
            bulk_operation_status: self.status(now).to_string(),
            start_time: super::reported_date(self.started_at),
            end_time: self.ended_at(now).map(super::reported_date),
        }
    }

    /// This operation as the call that started it — and every call that replays its token, and
    /// its own describe — reports it, as it stands at `now`.
    ///
    /// The figures a FINISHED task reports are reported only once the task has finished, the
    /// way an import's are: while it runs the counters stand at zero, because nothing has
    /// been counted yet, and the source's total size is left out until it is known —
    /// "DynamoDB omits this value until the initial input-size calculation completes"
    /// (`BulkOperationDescription$TotalInputBytes`).
    ///
    /// The failure is reported for the one status it belongs to: "DynamoDB returns this value
    /// only when `BulkOperationStatus` is `FAILED` or `CANCELLED`" (`$FailureCode`) — a
    /// cancelled operation stopped because a caller asked it to and carries no failure of its
    /// own, so what is reported is the recorded reason of one that FAILED.
    pub fn describe(&self, now: SystemTime) -> BulkOperationDescription {
        let finished = self.ended_at(now).is_some();
        let status = self.status(now).to_string();
        let failed = self
            .failure
            .as_ref()
            .filter(|_| status == BULK_OPERATION_STATUS_FAILED);
        BulkOperationDescription {
            bulk_operation_arn: Some(self.arn.clone()),
            bulk_operation_status: Some(status),
            table_details: Some(vec![BulkOperationTableDetail {
                table_name: Some(self.table_name.clone()),
                table_id: Some(self.table_id.clone()),
            }]),
            client_token: self.client_token.clone(),
            input_s3_configuration: Some(self.input_s3_configuration.clone()),
            output_s3_configuration: Some(self.output_s3_configuration.clone()),
            start_time: Some(super::reported_date(self.started_at)),
            end_time: self.ended_at(now).map(super::reported_date),
            processed_item_count: if finished {
                self.processed_item_count
            } else {
                0
            },
            processed_input_bytes: Some(if finished {
                self.processed_input_bytes
            } else {
                0
            }),
            total_input_bytes: finished.then_some(self.processed_input_bytes),
            bulk_operation_usage: finished.then(|| vec![self.usage()]),
            failed_item_count: if finished { self.failed_item_count } else { 0 },
            failure_code: failed.map(|failure| failure.code.clone()),
            failure_message: failed.map(|failure| failure.message.clone()),
            max_write_request_units: self.max_write_request_units,
            performance_tier: self.performance_tier.clone(),
        }
    }

    /// What this operation's writes USED, which a finished operation reports one entry of per
    /// table it wrote into — one, since a bulk put addresses one table.
    ///
    /// "The number of bulk put bytes for the table, excluding its indexes"
    /// (`BulkOperationUsage$Table`) is the item bytes the writes added to the table, and
    /// "the total number of bulk put bytes used by the operation" (`$BulkPutBytes`) is that
    /// same figure here: this service keeps no stored entry per secondary index — every index
    /// is projected from the table's own items on each read — so there are no index bytes to
    /// report beside it and the per-index maps are left out.
    fn usage(&self) -> BulkOperationUsage {
        BulkOperationUsage {
            table_name: Some(self.table_name.clone()),
            bulk_put_bytes: Some(self.bulk_put_bytes),
            table: Some(BulkOperationBytes {
                bulk_put_bytes: Some(self.bulk_put_bytes),
            }),
            ..BulkOperationUsage::default()
        }
    }
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Every bulk operation of one region, in ascending ARN order, which is the scope a
    /// listing reports and the order it pages through.
    pub fn bulk_operations_in<'s>(
        &'s self,
        region: &'s str,
    ) -> impl Iterator<Item = &'s BulkOperation> + 's {
        self.bulk_operations
            .values()
            .filter(move |operation| operation.region == region)
    }

    /// How many bulk operations that region already holds — what
    /// [`MAX_BULK_OPERATIONS_PER_REGION`] is measured against.
    pub fn bulk_operation_count_in(&self, region: &str) -> usize {
        self.bulk_operations_in(region).count()
    }

    /// Record one bulk operation under the ARN minted for it.
    pub fn insert_bulk_operation(&mut self, operation: BulkOperation) {
        self.bulk_operations
            .insert(operation.arn.clone(), operation);
    }

    /// The bulk operation `arn` names in that region, or `None` where the account holds
    /// none under it.
    ///
    /// Scoped to the region the request reached, like every other bulk-operation lookup: an
    /// endpoint serves one region, so an ARN naming another names nothing it can reach.
    pub fn bulk_operation_in(&self, region: &str, arn: &str) -> Option<&BulkOperation> {
        self.bulk_operations
            .get(arn)
            .filter(|operation| operation.region == region)
    }

    /// The same operation, for the one call that CHANGES it ([`BulkOperation::cancel`]).
    pub fn bulk_operation_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut BulkOperation> {
        self.bulk_operations
            .get_mut(arn)
            .filter(|operation| operation.region == region)
    }

    /// The bulk operation a `ClientToken` still stands for at `now` in that region, if any
    /// — what makes a repeated call idempotent rather than a second operation.
    ///
    /// Scoped to the region the request reached, like every other bulk-operation lookup: a
    /// token names an operation of the endpoint it was first used against.
    pub fn bulk_operation_for_client_token(
        &self,
        region: &str,
        client_token: &str,
        now: SystemTime,
    ) -> Option<&BulkOperation> {
        self.bulk_operations.values().rev().find(|operation| {
            operation.region == region
                && operation.client_token.as_deref() == Some(client_token)
                && operation.within_idempotency_window(now)
        })
    }
}
