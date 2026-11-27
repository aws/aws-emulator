// SPDX-License-Identifier: CC0-1.0

//! @op StartBulkPutItems
//! IMPLEMENTED: starts ONE bulk operation that reads the items staged in Amazon S3 and
//! writes them into an EXISTING table, records that operation under an ARN of its own and
//! answers that ARN with the status the operation is in.
//!
//! A bulk operation is a RESOURCE: it is addressed by its own ARN, it carries a status, and
//! `ListBulkOperations` reports it afterwards — so what this call answers is the operation
//! it started rather than the result of the writes. The operation is reported `IN_PROGRESS`
//! for one transition span and the status it settled into from then on
//! ([`crate::state::STATUS_TRANSITION`]), which is the span every clock-owed transition of
//! this emulator takes; nothing sleeps for it, because the due instant is stored on the
//! record and every reader derives the status from the clock
//! ([`crate::state::BulkOperation::status`]).
//!
//! IDEMPOTENT PER `ClientToken`: a call that repeats the token of an earlier call inside
//! the window a token stands for ([`crate::state::BULK_OPERATION_CLIENT_TOKEN_WINDOW`])
//! starts NO second operation and answers the one that token already started, so a client
//! that retries a bulk put reads back one operation rather than two — which is what makes
//! the retry safe, since a second operation would write the source a second time. A call
//! that reuses the token and CHANGES any other member asks for something that token does
//! not stand for, and is refused as the idempotency mismatch it is
//! ([`crate::state::bulk_put_request_signature`] is what two requests are compared
//! through).
//!
//! WHAT IT READS AND WHAT IT WRITES. `InputS3Configuration` names the bucket, the key
//! prefix, the format and the compression the items are staged in, which is the same source
//! an `ImportTable` reads — so this reads it through the same reader and decoder
//! ([`crate::state::read_import_source`], [`crate::state::imported_items`]), with the
//! calling principal's own S3 permissions — though the FORMATS it admits are its own and
//! narrower than an import's
//! (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BulkPutInputFormat")`), and
//! each decoded item is then held to exactly
//! what a single-item write holds one to (its numbers canonicalized, its values held to the
//! documented domain, the documented 400 KB ceiling, the table's own key schema) and stored
//! through the one write seam this service stores an item through. So the items a bulk put
//! wrote are read back by `GetItem`, `Query`, `Scan` and every index of the table exactly as
//! a `PutItem`'s are, and the changes reach the table's stream the same way. A record the
//! table cannot hold is counted among the operation's errors and the rest still land, the
//! way an import task logs an error for one and carries on.
//!
//! WHAT IT DOES NOT DO. `OutputS3Configuration` names where the operation reports its
//! per-record outcomes — a location every request must name — `MaxWriteRequestUnits` the
//! throughput it may consume while it runs, and `PerformanceTier` the tier it runs in: all
//! three are read and held to their own
//! shape, and none of them changes what the call answers here — this emulator applies the
//! whole source under the call that started it, so there is no rate to bound and no
//! long-running task to report progress of, and the layout of the report objects is stated
//! by neither the model nor the recording, so nothing is written into that bucket rather
//! than inventing a file format for it.
//!
//! An unknown — or not yet `ACTIVE` — table is `ResourceNotFoundException` naming the table
//! the request addressed, and a table whose resource-based policy denies the caller this
//! call is `AccessDeniedException`, both decided before the source is read.
//!
//! WHERE THIS OPERATION'S CONTRACT COMES FROM. The service's Smithy model carried no
//! `StartBulkPutItems` shape when this was written, so there is no modelled doc text to
//! quote: the members are the ones the recorded calls carry — the table, the input and
//! output S3 configurations, the write-unit ceiling, the performance tier and the client
//! token the recording replays to confirm one operation is returned — and the shapes this
//! crate is built from now declare them.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so engine
//! access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#StartBulkPutItems")]
#[derive(Debug)]
pub enum StartBulkPutItemsErrorModeled {
    IdempotentParameterMismatchException(IdempotentParameterMismatchException),
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    LimitExceededException(LimitExceededException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_bulk_put_items::StartBulkPutItemsError`).
pub type StartBulkPutItemsError = crate::scaffolded::Scaffolded<StartBulkPutItemsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<StartBulkPutItemsErrorModeled> for StartBulkPutItemsError {
    fn from(modeled: StartBulkPutItemsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartBulkPutItems` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this crate
/// names no concrete engine type.
pub struct StartBulkPutItems<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartBulkPutItems<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartBulkPutItems<Eng>
{
    type Input = StartBulkPutItemsInput;
    type Output = StartBulkPutItemsOutput;
    type Error = StartBulkPutItemsError;

    const NAME: &'static str = "StartBulkPutItems";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::START_BULK_PUT_ITEMS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::START_BULK_PUT_ITEMS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        let name = crate::state::resolve_table_name_in(&input.table_name, &region).map_err(lift)?;
        let source = &input.input_s3_configuration;
        // The format and the compression the source is read with, resolved to what each
        // member defaults to where the request named none — the same two values an import
        // reads its own source with.
        let format = source
            .input_format
            .clone()
            .unwrap_or_else(|| crate::state::INPUT_FORMAT_DYNAMODB_JSON.to_string());
        let compression = source
            .input_compression_type
            .clone()
            .unwrap_or_else(|| crate::state::DEFAULT_INPUT_COMPRESSION_TYPE.to_string());
        // A source in the Amazon Ion encoding is not read yet, so a bulk put of one is
        // refused as unimplemented rather than accepted and answered with an operation that
        // read nothing — exactly as `ImportTable` refuses it.
        if format == crate::state::INPUT_FORMAT_ION {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "StartBulkPutItems with InputFormat ION",
            ));
        }

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        // What this request ASKED FOR, in the one form two requests are compared in: a
        // replayed `ClientToken` answers the operation it already started only if every
        // other member is the one that operation was started with.
        let signature = crate::state::bulk_put_request_signature(
            &name,
            source,
            Some(&input.output_s3_configuration),
            input.max_write_request_units,
            input.performance_tier.as_deref(),
        );
        // The table this call writes into, the caller's authorization for it, the token it
        // may be replaying and this emulator's own ceiling on the catalogue — all settled
        // under the store's own guard and BEFORE the source is read, so a call refused for
        // any of them reads nothing and records nothing. The guard is released before the
        // source is read, and the token is asked again under the guard that writes.
        //
        // AUTHORIZATION COMES FIRST, ahead of the token: what a replay answers is the
        // operation that token started — its ARN, the source it read and its figures — so a
        // caller the policies in force do not let run this call must be refused before any
        // of that is reported back, exactly as the sibling import orders the two
        // (`super::import_table`).
        let declared_types: ::std::collections::BTreeMap<String, String> = {
            let mut store = state.lock();
            store.apply_due_transitions(now);
            let table = store.active_table_named(&region, &name, now).map_err(lift)?;
            authorize(&caller, table, now)?;
            // The items would be stored ENCRYPTED under the table's KMS key, so a key KMS
            // will not use leaves nothing this call could encrypt its writes with
            // ([`crate::state::check_table_key_accessible`]).
            crate::state::check_table_key_accessible(&*self.engine, table).map_err(lift)?;
            // The attribute types the TABLE declares, which is what a column of a
            // record that carries no type of its own is read as.
            let declared_types = table
                .attribute_definitions
                .iter()
                .map(|definition| {
                    (
                        definition.attribute_name.clone(),
                        definition.attribute_type.clone(),
                    )
                })
                .collect();
            if let Some(started) =
                replayed(&store, &region, input.client_token.as_deref(), &signature, now)
            {
                return started;
            }
            if store.bulk_operation_count_in(&region)
                >= crate::state::MAX_BULK_OPERATIONS_PER_REGION
            {
                return Err(limit_exceeded(format!(
                    "Subscriber limit exceeded: this account already holds the maximum of {} \
                     bulk operations in {region}",
                    crate::state::MAX_BULK_OPERATIONS_PER_REGION
                )));
            }
            declared_types
        };

        // The source objects this operation reads, out of Amazon S3's own store on this
        // engine, with the CALLING PRINCIPAL's own permissions: the listing that selects the
        // objects and the read of each one are authorized exactly as those calls by that
        // principal are ([`crate::state::import_listing_denial`],
        // [`crate::state::import_read_denial`]). A bucket that store holds none of, and a
        // read the policies refuse, are a source nothing can be read from — the operation is
        // accepted and SETTLES FAILED, because what could not be read is the task's own
        // outcome rather than a fault in this request.
        let s3_source = source_of(source);
        let objects = match crate::state::read_import_source(&*self.engine, &s3_source) {
            Ok(objects) => {
                let denied =
                    crate::state::import_listing_denial(&*self.engine, &caller, &s3_source.s3_bucket)
                        .or_else(|| {
                            crate::state::import_read_denial(
                                &*self.engine,
                                &caller,
                                &s3_source.s3_bucket,
                                &objects,
                            )
                        });
                match denied {
                    Some(_) => None,
                    None => Some(objects),
                }
            }
            Err(crate::state::ImportSourceFault::StoreUnavailable) => {
                return Err(StartBulkPutItemsErrorModeled::InternalServerError(
                    InternalServerError {
                        message: Some("the Amazon S3 object store is unavailable".to_string()),
                    },
                )
                .into())
            }
            Err(crate::state::ImportSourceFault::NoSuchBucket) => None,
        };
        // Decoding the source is CPU-bound and sized by the objects read, so it runs on a
        // BLOCKING task rather than on a runtime worker a large source would starve.
        let read = match objects {
            Some(objects) => {
                let decoded = ::tokio::task::spawn_blocking(move || {
                    crate::state::imported_items(
                        &objects,
                        &format,
                        &compression,
                        None,
                        &declared_types,
                    )
                })
                .await;
                match decoded {
                    Ok(Ok(read)) => Some(read),
                    // A source past this emulator's own ceilings is refused BEFORE anything
                    // is written, with the modelled limit error.
                    Ok(Err(crate::state::ImportTooLarge)) => {
                        return Err(limit_exceeded(crate::state::import_too_large_message(
                            &s3_source.s3_bucket,
                        )))
                    }
                    Err(error) => {
                        return Err(StartBulkPutItemsErrorModeled::InternalServerError(
                            InternalServerError {
                                message: Some(format!(
                                    "the bulk operation could not read its source: {error}"
                                )),
                            },
                        )
                        .into())
                    }
                }
            }
            None => None,
        };

        let mut store = state.lock();
        store.apply_due_transitions(now);
        // The table and the caller's authorization for it are settled again under the guard
        // that writes, and again AHEAD of the token: the guard was released while the source
        // was read, so a table that has gone — or a policy that has changed — decides this
        // call rather than whatever stood when it started.
        let table = store.active_table_named(&region, &name, now).map_err(lift)?;
        authorize(&caller, table, now)?;
        // Asked again under the same guard: a concurrent replay must answer the operation
        // the token already started rather than record a second one.
        if let Some(started) =
            replayed(&store, &region, input.client_token.as_deref(), &signature, now)
        {
            return started;
        }
        let mut written: Vec<(crate::state::ItemKey, crate::state::Item)> = Vec::new();
        let mut counts = Counts::default();
        if let Some(read) = read {
            counts.read_its_source = true;
            counts.processed = read.processed_item_count;
            counts.errors = read.error_count;
            // Each decoded item is keyed against the table it is written into, and one the
            // table cannot hold is counted among the errors instead
            // ([`crate::state::keyed_items`], the one rule an imported item is held to).
            let table = store.active_table_named(&region, &name, now).map_err(lift)?;
            let keyed = crate::state::keyed_items(table, read.items);            counts.errors = counts.errors.saturating_add(keyed.error_count);
            written.extend(keyed.items);
        }
        counts.written = i64::try_from(written.len()).unwrap_or(i64::MAX);
        for (key, item) in written {
            store
                .put_item(&region, &name, now, key, item)
                .map_err(lift)?;
        }
        // The changes this call made reach the table's stream before the response does, so a
        // consumer that reads the stream after seeing this answer sees them.
        crate::state::publish_item_changes(&*self.engine, &mut store, now);

        let operation = crate::state::BulkOperation {
            arn: crate::state::new_bulk_operation_arn(&region, now),
            region: region.clone(),
            table_name: name.clone(),
            client_token: input.client_token.clone(),
            input_s3_configuration: input.input_s3_configuration.clone(),
            output_s3_configuration: Some(input.output_s3_configuration.clone()),
            max_write_request_units: input.max_write_request_units,
            performance_tier: input.performance_tier.clone(),
            started_at: crate::state::at_timestamp_precision(now),
            ended_at: now + crate::state::STATUS_TRANSITION,
            // Nothing has asked for this operation to stop: a cancel is what sets it
            // ([`crate::state::BulkOperation::cancel`]).
            cancelled_at: None,
            // What the task SETTLES INTO: a source it could not read at all is the one
            // outcome that fails, and anything it read — per-record errors included — is an
            // operation that completed, the way an import task that logged errors completes.
            status: if counts.read_its_source {
                crate::state::BULK_OPERATION_STATUS_COMPLETED.to_string()
            } else {
                crate::state::BULK_OPERATION_STATUS_FAILED.to_string()
            },
            processed_item_count: counts.processed,
            written_item_count: counts.written,
            error_count: counts.errors,
        };
        audited(Self::NAME, ctx, &operation, &s3_source.s3_bucket);
        let answer = started_output(&operation, now);
        store.insert_bulk_operation(operation);
        Ok(answer)
    }
}

/// What one bulk operation's task counted.
#[derive(Debug, Default)]
struct Counts {
    /// Whether the source was read at all, which is what decides the terminal status.
    read_its_source: bool,
    /// Every record the task read, the ones it could not decode included.
    processed: i64,
    /// The items that landed in the table.
    written: i64,
    /// The records the task read and could not write.
    errors: i64,
}

/// The `S3BucketSource` an `InputS3Configuration` names, so the source is read and
/// authorized through the one reader this service reads a staged source with.
fn source_of(configuration: &BulkPutInputS3Configuration) -> S3BucketSource {
    S3BucketSource {
        s3_bucket: configuration.s3_bucket.clone(),
        s3_key_prefix: configuration.s3_key_prefix.clone(),
        s3_bucket_owner: configuration.s3_bucket_owner.clone(),
    }
}

/// What a call replaying `client_token` answers: the operation that token already started,
/// as it stands at `now`, for a call that asks for exactly what that operation was started
/// with — and the conflict for one that changed any other member, because a token stands
/// for the operation it started rather than for whatever a later call asks under it.
/// `None` where the request carries no token, or where the token stands for no operation of
/// this region, which is a call that starts work of its own.
fn replayed(
    store: &crate::state::ComAmazonawsDynamodbDynamodb20120810State,
    region: &str,
    client_token: Option<&str>,
    signature: &str,
    now: ::std::time::SystemTime,
) -> Option<Result<StartBulkPutItemsOutput, StartBulkPutItemsError>> {
    let started = store.bulk_operation_for_client_token(region, client_token?, now)?;
    if started.request_signature() != signature {
        return Some(Err(conflict()));
    }
    Some(Ok(started_output(started, now)))
}

/// The modelled refusal of a `ClientToken` that already stands for a DIFFERENT request:
/// "DynamoDB rejected the request because you retried a request with a different payload
/// but with an idempotent token that was already used"
/// (`IdempotentParameterMismatchException`), stated in the fixed sentence the
/// bulk-operation surface states for that refusal
/// ([`crate::state::BULK_OPERATION_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`]) — naming
/// neither the token nor the member that changed.
fn conflict() -> StartBulkPutItemsError {
    StartBulkPutItemsErrorModeled::IdempotentParameterMismatchException(
        IdempotentParameterMismatchException {
            message: Some(
                crate::state::BULK_OPERATION_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE.to_string(),
            ),
        },
    )
    .into()
}

/// What a call that started — or replayed — `operation` answers: that operation's own
/// description, as it stands at `now`.
fn started_output(
    operation: &crate::state::BulkOperation,
    now: ::std::time::SystemTime,
) -> StartBulkPutItemsOutput {
    StartBulkPutItemsOutput {
        bulk_operation_description: operation.describe(now),
    }
}

/// Whether the policies in force let this caller run a bulk put against `table`.
///
/// The decision is the table's: a bulk put writes that table's items, so it is decided
/// against the table's own resource-based policy and the caller's identity documents, over
/// the whole table rather than over any one partition — the items this operation will write
/// are not known until its source has been read.
fn authorize(
    caller: &crate::state::authorization::Caller,
    table: &crate::state::Table,
    now: ::std::time::SystemTime,
) -> Result<(), StartBulkPutItemsError> {
    match crate::state::authorization::denial(caller, table, ACTION, now) {
        Some(denial) => Err(crate::scaffolded::Scaffolded::access_denied(denial)),
        None => Ok(()),
    }
}

/// The IAM action this operation performs, which a refusal names.
const ACTION: &str = "dynamodb:StartBulkPutItems";

/// The modelled limit error for a bulk put past one of the quotas it is held to.
fn limit_exceeded(message: String) -> StartBulkPutItemsError {
    StartBulkPutItemsErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message),
    })
    .into()
}

/// The audit record for a state-changing call: who asked, what was started and how its task
/// ended up. Never a credential — only the access key id the caller presented.
/// Request-sourced strings are rendered with `Debug`, so a control character in one cannot
/// shape the record.
fn audited(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    started: &crate::state::BulkOperation,
    source_bucket: &str,
) {
    ::tracing::info!(
        operation,
        akid = %::emulator_server::extract_akid(&ctx.parts().headers),
        table_name = ?started.table_name,
        bulk_operation_arn = %started.arn,
        source_bucket = ?source_bucket,
        processed = started.processed_item_count,
        written = started.written_item_count,
        errors = started.error_count,
        outcome = ?started.status,
        "bulk put items started"
    );
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> StartBulkPutItemsError {
    StartBulkPutItemsErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> StartBulkPutItemsError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            StartBulkPutItemsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}
