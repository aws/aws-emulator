// SPDX-License-Identifier: CC0-1.0

//! @op BatchExecuteStatement
//! IMPLEMENTED: runs the batch's statements against the SAME item store the classic
//! item API and `ExecuteStatement` read and write, each statement ON ITS OWN. "A HTTP
//! 200 response does not mean that all statements in the BatchExecuteStatement
//! succeeded. Error details for individual statements can be found under the Error
//! field of the BatchStatementResponse for each statement" (the operation's doc), so a
//! statement this emulator cannot serve is reported in ITS OWN slot — by the
//! `BatchStatementErrorCodeEnum` code for what was wrong with it — while the slots
//! beside it are served, and the batch still answers 200.
//!
//! Each statement that changes an item publishes the same stream record the equivalent
//! classic write does ([`crate::state::publish_item_changes`]), in the order the batch
//! applied them.
//!
//! The dialect itself is parsed in [`crate::state::partiql`] and the value semantics
//! are [`crate::state::ParsedCondition`]/[`crate::state::ParsedUpdate`], so which
//! comparison holds between two attribute values, and what an update action does to an
//! item, have ONE implementation in this crate — the one `ExecuteStatement`, `PutItem`,
//! `UpdateItem` and `DeleteItem` go through. What this operation adds is the BATCH:
//!
//! * "Each read statement in a BatchExecuteStatement must specify an equality
//!   condition on all key attributes. This enforces that each SELECT statement in a
//!   batch returns at most a single item" — so a batch `SELECT` addresses ONE item of
//!   the table by its whole primary key. One that pins less than that, one whose clause
//!   states a term that is no equality condition at all (a boolean function, a comparison
//!   that is not `=`, a range, a branch of an `OR`), or that reads a
//!   secondary index (which is not keyed by the table's primary key), reports
//!   `ValidationError` in its own slot.
//! * "The entire batch must consist of either read statements or write statements, you
//!   cannot mix both in one batch": a batch that mixes them is refused as a whole, with
//!   a 400 `ValidationException` ([`MIXED_BATCH_MESSAGE`]), because the refusal is of the
//!   REQUEST rather than of any one statement in it.
//! * A write batch applies at most one write per item, like every other multi-item
//!   write path here ([`crate::state::DUPLICATE_BATCH_KEYS_MESSAGE`]): two statements
//!   addressing one item would leave a result that depends on which ran first, so the
//!   request is refused rather than half-applied.
//! * Each statement reports what it did in its own slot: a read reports the item it
//!   found (none at all when the key holds none), a write reports what its `RETURNING`
//!   clause asked for, every slot that REACHED its table echoes that table under
//!   `TableName` — so a batch spread over several tables can be attributed slot by slot,
//!   while a statement refused for its own FORM, before it read or wrote anything, carries
//!   the error alone. A statement the STORED item refused — an operand of the wrong type
//!   for the value an attribute holds, an item the change would leave too large — reached
//!   its table as surely as one that ran, so its slot names that table too
//!   ([`at_table`]) — while `ConsumedCapacity` is reported per TABLE rather than per
//!   statement: `ConsumedCapacityMultiple` carries one entry per table the batch reached,
//!   whose figure is the SUM over the statements that reached it ([`Totals`]).
//! * A statement's write is admitted from the capacity its table is provisioned for
//!   ([`crate::state::Table::admit_write`]): one past that budget is throttled in its
//!   own slot, with the `ProvisionedThroughputExceeded` code, rather than applied.
//!
//! A batch is NOT a transaction: statements run in order and each one's write lands
//! before the next one runs, so a statement that fails leaves the writes before it in
//! place — which is what the operation's own "a HTTP 200 does not mean all statements
//! succeeded" says.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `BatchExecuteStatement::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchExecuteStatement`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! This operation allows you to perform batch reads or writes on data stored in DynamoDB,
//! using PartiQL. Each read statement in a BatchExecuteStatement must specify an equality
//! condition on all key attributes. This enforces that each SELECT statement in a batch
//! returns at most a single item. For more information, see Running batch operations with
//! PartiQL for DynamoDB . The entire batch must consist of either read statements or write
//! statements, you cannot mix both in one batch. A HTTP 200 response does not mean that all
//! statements in the BatchExecuteStatement succeeded. Error details for individual
//! statements can be found under the Error field of the BatchStatementResponse for each
//! statement.
//!
//! ## Input
//!
//! * `ReturnConsumedCapacity` (ReturnConsumedCapacity, one of: INDEXES | NONE | TOTAL) —
//!   Determines the level of detail about either provisioned or on-demand throughput
//!   consumption that is returned in the response: INDEXES - The response includes the
//!   aggregate ConsumedCapacity for the operation, together with ConsumedCapacity for each
//!   table and secondary index that was accessed. Note that some operations, such as GetItem
//!   and BatchGetItem, do not access any indexes at all. In these cases, specifying INDEXES
//!   will only return ConsumedCapacity information for table(s). TOTAL - The response
//!   includes only the aggregate ConsumedCapacity for the operation. NONE - No
//!   ConsumedCapacity details are included in the response.
//! * `Statements` (PartiQLBatchRequest, len 1..25) **required** — The list of PartiQL
//!   statements representing the batch to run.
//!
//! ## Output
//!
//! * `ConsumedCapacity` (ConsumedCapacityMultiple) — The capacity units consumed by the
//!   entire operation. The values of the list are ordered according to the ordering of the
//!   statements.
//! * `Responses` (PartiQLBatchResponse) — The response to each PartiQL statement in the
//!   batch. The values of the list are ordered according to the ordering of the request
//!   statements.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `RequestLimitExceeded` (HTTP 400) — Throughput exceeds the current throughput quota for
//!   your account. For detailed information about why the request was throttled and the ARN
//!   of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   Contact Amazon Web Services Support to request a quota increase.
//! * `ThrottlingException` (HTTP 400) — The request was denied due to request throttling. For
//!   detailed information about why the request was throttled and the ARN of the impacted
//!   resource, find the ThrottlingReason field in the returned exception.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchExecuteStatementError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchExecuteStatementError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#BatchExecuteStatement")]
#[derive(Debug)]
pub enum BatchExecuteStatementErrorModeled {
    InternalServerError(InternalServerError),
    RequestLimitExceeded(RequestLimitExceeded),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_execute_statement::BatchExecuteStatementError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchExecuteStatementError = crate::scaffolded::Scaffolded<BatchExecuteStatementErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchExecuteStatementErrorModeled> for BatchExecuteStatementError {
    fn from(modeled: BatchExecuteStatementErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchExecuteStatement` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchExecuteStatement<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchExecuteStatement<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchExecuteStatement<Eng>
{
    type Input = BatchExecuteStatementInput;
    type Output = BatchExecuteStatementOutput;
    type Error = BatchExecuteStatementError;

    const NAME: &'static str = "BatchExecuteStatement";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_EXECUTE_STATEMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_EXECUTE_STATEMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        let requested_capacity = input.return_consumed_capacity.as_deref();

        // `ReturnValuesOnConditionCheckFailure` carries two values, per statement. A
        // value the model's enum does not name is a malformed REQUEST member rather
        // than a statement that failed, so it is refused as a whole.
        if input.statements.iter().any(|request| {
            !matches!(
                request.return_values_on_condition_check_failure.as_deref(),
                None | Some("NONE" | "ALL_OLD")
            )
        }) {
            return Err(crate::scaffolded::Scaffolded::validation(
                "ReturnValuesOnConditionCheckFailure can only be ALL_OLD or NONE",
            ));
        }

        // Every statement is parsed before any of them runs: what each statement IS
        // decides whether the BATCH is well formed, and a statement that does not parse
        // is one slot's failure rather than the request's.
        let parsed: Vec<Result<crate::state::PartiqlStatement, crate::state::RequestError>> =
            input.statements.iter().map(parse_slot).collect();

        // "The entire batch must consist of either read statements or write statements,
        // you cannot mix both in one batch" — a rule about the request, so a mixed
        // batch is refused whole and nothing of it is applied
        // ([`MIXED_BATCH_MESSAGE`] is the sentence it is refused with).
        let reads = parsed
            .iter()
            .flatten()
            .filter(|statement| matches!(statement, crate::state::PartiqlStatement::Select(_)))
            .count();
        let writes = parsed.iter().flatten().count() - reads;
        if reads > 0 && writes > 0 {
            return Err(crate::scaffolded::Scaffolded::validation(MIXED_BATCH_MESSAGE));
        }

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);
        check_distinct_write_targets(&store, &region, now, &parsed)?;

        // Each statement is then served on its own, in order, and reports into its own
        // slot. A write lands before the next statement runs, so a later `SELECT` in
        // another batch — or a classic read — sees it.
        let mut responses: Vec<BatchStatementResponse> = Vec::with_capacity(parsed.len());
        let mut totals = Totals::default();
        for (request, statement) in input.statements.iter().zip(parsed) {
            let served = statement.map_err(SlotFailure::from).and_then(|statement| {
                run_slot(
                    &*self.engine,
                    &mut store,
                    &caller,
                    &region,
                    now,
                    request,
                    statement,
                )
            });
            match served {
                Ok((response, capacity)) => {
                    responses.push(response);
                    totals.record(&capacity);
                }
                Err(failure) => responses.push(failure.into_response()),
            }
        }
        // Every statement that changed an item owes its table's stream a record, in the
        // order the batch applied them.
        crate::state::publish_item_changes(&*self.engine, &mut store, now);

        Ok(BatchExecuteStatementOutput {
            responses: Some(responses),
            // "The capacity units consumed by the entire operation": one entry per TABLE
            // the batch reached, whose figure is the sum over the statements that reached
            // it, and no member at all when the request asked for no capacity detail
            // ([`Totals`]).
            consumed_capacity: totals.consumed_capacity(requested_capacity),
        })
    }
}

/// This service's store, as the slot paths below take it.
type Store = crate::state::ComAmazonawsDynamodbDynamodb20120810State;

/// How a batch that mixes reads and writes is refused: the fixed sentence DynamoDB
/// states that rule with, which is NOT the prose the operation's doc explains it in.
///
/// The doc says what the rule IS ("The entire batch must consist of either read statements
/// or write statements, you cannot mix both in one batch"); the refusal is one sentence
/// about the request, with no variable part — the trailing period included, because the
/// sentence is what a client matches.
const MIXED_BATCH_MESSAGE: &str = "Read and write requests together in the same batch is not \
                                   supported.";

/// How a read statement that does not address ONE item of the table by its primary key
/// is refused, in the fixed sentence DynamoDB states that rule with.
///
/// The rule is the operation's own doc: "Each read statement in a BatchExecuteStatement
/// must specify an equality condition on all key attributes. This enforces that each
/// SELECT statement in a batch returns at most a single item." A statement that pins
/// less than the whole primary key breaks it, and so does one whose source is a
/// secondary index — an index is not keyed by the table's primary key, so no equality
/// on that key addresses one item of it.
const KEY_EQUALITY_MESSAGE: &str = "Select statements within BatchExecuteStatement must specify \
                                    the primary key in the where clause.";

/// How an `INSERT` whose primary key is already stored is reported, the way the
/// single-statement path reports the same collision
/// ([`crate::state::DUPLICATE_ITEM_MESSAGE`] is the sentence, shared with the
/// transactional PartiQL surface).
const DUPLICATE_ITEM_MESSAGE: &str = crate::state::DUPLICATE_ITEM_MESSAGE;

// The `BatchStatementError$Code` values this operation reports, each one of
// `BatchStatementErrorCodeEnum` (`super::types::BATCH_STATEMENT_ERROR_CODE_ENUM_VALUES`).

/// The code for a statement this emulator refused on its own merits.
const VALIDATION_ERROR_CODE: &str = "ValidationError";

/// The code for a statement naming a table that is not there (or not yet serving
/// items).
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// The code for a statement the table's resource-based policy denies the caller.
const ACCESS_DENIED_CODE: &str = "AccessDenied";

/// The code for an `INSERT` whose primary key is already stored
/// ([`crate::state::DUPLICATE_ITEM_CODE`], shared with the transactional PartiQL
/// surface).
const DUPLICATE_ITEM_CODE: &str = crate::state::DUPLICATE_ITEM_CODE;

/// The code for a write whose `WHERE` clause the stored item does not satisfy.
const CONDITIONAL_CHECK_FAILED_CODE: &str = "ConditionalCheckFailed";


/// Parse one slot's statement; each `?` marker's value is held to the same rules a
/// stored value is by the parser that binds it
/// ([`crate::state::parse_statement`]).
fn parse_slot(
    request: &BatchStatementRequest,
) -> Result<crate::state::PartiqlStatement, crate::state::RequestError> {
    let parameters = request.parameters.clone().unwrap_or_default();
    crate::state::parse_statement(&request.statement, &parameters)
}

/// What one statement of the batch failed with, as its own slot reports it.
///
/// A failure here is NOT the request's: it becomes a `BatchStatementResponse` carrying
/// an `Error`, and the batch still answers 200 with every other slot served.
struct SlotFailure {
    /// The `BatchStatementErrorCodeEnum` value for what was wrong.
    code: &'static str,
    message: String,
    /// The table the statement REACHED: "The table name associated with a failed PartiQL
    /// batch statement" (`BatchStatementResponse$TableName`) — the table a statement that
    /// ran against its items failed on, whether it failed its condition, was refused by
    /// what the STORED item holds ([`at_table`]) or was throttled, and `None` for a
    /// statement refused for its own form before it read or wrote anything, which reached
    /// no table.
    table: Option<String>,
    /// "The item which caused the condition check to fail. This will be set if
    /// ReturnValuesOnConditionCheckFailure is specified as ALL_OLD"
    /// (`BatchStatementError$Item`).
    item: Option<crate::state::Item>,
}

impl SlotFailure {
    /// A failure under `code`.
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            table: None,
            item: None,
        }
    }

    /// A statement this emulator refused on its own merits.
    fn validation(message: impl Into<String>) -> Self {
        Self::new(VALIDATION_ERROR_CODE, message)
    }

    /// The same failure, reported against the table the statement reached.
    fn at(mut self, table: &str) -> Self {
        self.table = Some(table.to_string());
        self
    }

    /// The same failure, carrying the item the request asked to see.
    fn with_item(mut self, item: Option<crate::state::Item>) -> Self {
        self.item = item;
        self
    }

    /// The slot this failure reports.
    fn into_response(self) -> BatchStatementResponse {
        BatchStatementResponse {
            error: Some(BatchStatementError {
                code: Some(self.code.to_string()),
                message: Some(self.message),
                item: self.item,
            }),
            table_name: self.table,
            item: None,
        }
    }
}

/// A store refusal, as the slot that earned it reports it.
impl From<crate::state::RequestError> for SlotFailure {
    fn from(error: crate::state::RequestError) -> Self {
        match error {
            crate::state::RequestError::NotFound(message) => {
                Self::new(RESOURCE_NOT_FOUND_CODE, message)
            }
            crate::state::RequestError::Validation(message) => Self::validation(message),
        }
    }
}

/// A store refusal a statement earned once it was APPLIED to the item its table holds, or
/// once that table was resolved and inspected, as the slot that earned it reports it:
/// against the table the statement RAN against, which a name this endpoint resolved to no
/// table of its own is as well.
///
/// Such a refusal is one only the stored data can produce — what a clause does to the value
/// an attribute already holds, and what the item the statement leaves behind costs — so the
/// statement did reach its table, exactly as one that ran to completion and one whose
/// condition a stored item did not satisfy did. Only a statement refused for its own FORM,
/// before any item was read, reached no table and carries the error alone.
fn at_table(name: &str) -> impl Fn(crate::state::RequestError) -> SlotFailure + '_ {
    move |error| SlotFailure::from(error).at(name)
}

/// What one slot answers: its response, and what that statement consumed on the table it
/// ran against.
type SlotOutcome = (BatchStatementResponse, SlotCapacity);

/// What ONE statement of the batch consumed, and where.
///
/// A slot reports the figure it consumed rather than the `ConsumedCapacity` entry for it:
/// "The capacity units consumed by the entire operation" is reported per TABLE
/// (`ConsumedCapacityMultiple`), so what each statement spent is totalled into its
/// table's entry ([`Totals`]) and the response is projected once, from the totals.
struct SlotCapacity {
    /// The table the statement ran against, which the figure is totalled under.
    table: String,
    consumed: SlotConsumption,
}

/// The figure a slot consumed, in the kind of capacity its statement spends: a `SELECT`
/// reads and every other statement writes.
enum SlotConsumption {
    /// The read units a `SELECT` consumed.
    Read(f64),
    /// What a write consumed, broken down the way a write's `ConsumedCapacity` is.
    Write(crate::state::WriteCapacity),
}

/// What the whole batch consumed, totalled per table: one `ConsumedCapacity` entry per
/// table the batch reached, whose figure is the sum over the statements that reached it.
///
/// A batch is either all reads or all writes — a mixed one is refused
/// ([`MIXED_BATCH_MESSAGE`]) — so in practice one of these ledgers is filled; each is the
/// one the classic multi-item surfaces total the same answer with
/// ([`crate::state::ReadLedger`], [`crate::state::WriteLedger`]), and both are reported,
/// so nothing a batch consumed is left out whatever it consumed it on.
#[derive(Debug, Default)]
struct Totals {
    reads: crate::state::ReadLedger,
    writes: crate::state::WriteLedger,
}

impl Totals {
    /// Add what one slot consumed to its table's running total.
    fn record(&mut self, slot: &SlotCapacity) {
        match &slot.consumed {
            SlotConsumption::Read(units) => self.reads.record(&slot.table, *units),
            SlotConsumption::Write(capacity) => self.writes.record(&slot.table, capacity),
        }
    }

    /// The `ConsumedCapacity` list the operation reports, or `None` when the request asked
    /// for no capacity detail — or when no statement of the batch reached a table at all.
    ///
    /// Both ledgers are reported, so a batch that somehow consumed capacity of both kinds
    /// would under-report nothing: a read's figures and a write's are entries of the same
    /// list, each naming the table it was spent on.
    fn consumed_capacity(&self, requested: Option<&str>) -> Option<Vec<ConsumedCapacity>> {
        let reads = self
            .reads
            .consumed_capacity(requested, crate::state::KindReport::Itemised);
        let writes = self.writes.consumed_capacity(
            requested,
            crate::state::CapacityKind::Write,
            crate::state::KindReport::Itemised,
        );
        match (reads, writes) {
            (None, None) => None,
            (Some(entries), None) | (None, Some(entries)) => Some(entries),
            (Some(mut reads), Some(writes)) => {
                reads.extend(writes);
                Some(reads)
            }
        }
    }
}

/// Serve one statement of the batch.
// One parameter per piece of request context this slot is served against, as
// `write_transaction` in `execute_transaction` takes them; grouping them would only move
// the same list behind a struct built at the one call site.
#[allow(clippy::too_many_arguments)]
fn run_slot<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    store: &mut Store,
    caller: &crate::state::authorization::Caller,
    region: &str,
    now: ::std::time::SystemTime,
    request: &BatchStatementRequest,
    statement: crate::state::PartiqlStatement,
) -> Result<SlotOutcome, SlotFailure> {
    let name = crate::state::resolve_table_name_in(statement.table(), region)?;
    {
        // A table that is not there, and a table the policies in force do not let this
        // caller act on with this statement's own PartiQL action, are decided before the
        // statement reads or writes anything.
        let table = store
            .active_table(region, &name, now)
            .map_err(at_table(&name))?;
        // A statement of a batch performs its own PartiQL action INSIDE this operation,
        // which is what a statement admitting that action only as part of a given operation
        // tests ([`crate::state::authorization::EnclosingOperation`]).
        if let Some(denial) = crate::state::authorization::denial_indexed(
            caller,
            table,
            statement.index(),
            statement.action(),
            now,
            &crate::state::authorization::RequestKeys::of_statement(table, &statement)
                .within(crate::state::authorization::EnclosingOperation::BatchExecuteStatement),
        ) {
            return Err(SlotFailure::new(ACCESS_DENIED_CODE, denial).at(&name));
        }
        // The items are stored ENCRYPTED under the table's KMS key, so a key KMS will
        // not use leaves nothing this statement could read or encrypt: this slot reports
        // the KMS fault and writes NOTHING
        // ([`crate::state::check_table_key_accessible`]).
        crate::state::check_table_key_accessible(engine, table).map_err(at_table(&name))?;
        // A source keyed by several `HASH` or several `RANGE` attributes is addressed by
        // no equality a statement's `WHERE` clause can pin, so this slot reports that
        // before the clause it states is judged at all
        // ([`crate::state::MULTI_ATTRIBUTE_KEY_MESSAGE`]).
        crate::state::check_single_attribute_source_key(table, statement.index())?;
    }
    let return_old_on_failure =
        request.return_values_on_condition_check_failure.as_deref() == Some("ALL_OLD");
    match statement {
        crate::state::PartiqlStatement::Select(select) => read_item(
            store,
            region,
            &name,
            now,
            &select,
            request.consistent_read.unwrap_or(false),
        ),
        crate::state::PartiqlStatement::Insert(insert) => {
            insert_item(store, region, &name, now, &insert)
        }
        crate::state::PartiqlStatement::Update(update) => update_item(
            store,
            region,
            &name,
            now,
            &update,
            return_old_on_failure,
        ),
        crate::state::PartiqlStatement::Delete(delete) => delete_item(
            store,
            region,
            &name,
            now,
            &delete,
            return_old_on_failure,
        ),
        // A check guards a write it is applied WITH, and a batch applies each statement
        // on its own, so this slot has nothing for one to guard
        // ([`crate::state::EXISTS_ONLY_IN_TRANSACTION_MESSAGE`]).
        crate::state::PartiqlStatement::Exists(_) => Err(SlotFailure::validation(
            crate::state::EXISTS_ONLY_IN_TRANSACTION_MESSAGE,
        )),
    }
    // A slot reports the table its statement RAN against: "The table name associated
    // with a failed PartiQL batch statement" (`BatchStatementResponse$TableName`) is what
    // a slot that reached the table answers, so a caller that batched statements over
    // several tables can attribute each slot's result to its table — and a statement
    // refused on its own merits before it read or wrote anything reached no table, so its
    // slot carries the error alone (the paths below name the table on the failures that
    // DID reach it).
    .map(|(response, capacity)| {
        (
            BatchStatementResponse {
                table_name: Some(name.clone()),
                ..response
            },
            capacity,
        )
    })
}

/// Refuse a WRITE batch that addresses one item twice.
///
/// Each statement's write lands before the next one runs, so what a batch addressing
/// one item twice would leave depends on which statement ran first — the same reason
/// the classic multi-item write path refuses it
/// ([`crate::state::DUPLICATE_BATCH_KEYS_MESSAGE`]). A statement whose table or key does
/// not resolve addresses no item to collide on: it is left to fail in its own slot.
fn check_distinct_write_targets(
    store: &Store,
    region: &str,
    now: ::std::time::SystemTime,
    parsed: &[Result<crate::state::PartiqlStatement, crate::state::RequestError>],
) -> Result<(), BatchExecuteStatementError> {
    let mut addressed: ::std::collections::BTreeSet<(String, crate::state::ItemKey)> =
        ::std::collections::BTreeSet::new();
    for statement in parsed.iter().flatten() {
        let Ok(name) = crate::state::resolve_table_name_in(statement.table(), region) else {
            continue;
        };
        let Ok(table) = store.active_table(region, &name, now) else {
            continue;
        };
        let key = match statement {
            // A read addresses an item without changing it, so two reads of one item
            // are two reads of one item — and a check this surface does not serve at
            // all writes nothing either.
            crate::state::PartiqlStatement::Select(_)
            | crate::state::PartiqlStatement::Exists(_) => continue,
            crate::state::PartiqlStatement::Insert(insert) => table.key_of_item(&insert.item),
            crate::state::PartiqlStatement::Update(update) => {
                crate::state::singleton_key(table, &update.filter)
            }
            crate::state::PartiqlStatement::Delete(delete) => {
                crate::state::singleton_key(table, &delete.filter)
            }
        };
        let Ok(key) = key else {
            continue;
        };
        if !addressed.insert((name, key)) {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::DUPLICATE_BATCH_KEYS_MESSAGE,
            ));
        }
    }
    Ok(())
}

/// `SELECT`: the ONE item the statement's `WHERE` clause pins by equality on every key
/// attribute, when the stored item satisfies the whole clause.
fn read_item(
    store: &mut Store,
    region: &str,
    name: &str,
    now: ::std::time::SystemTime,
    select: &crate::state::SelectStatement,
    consistent_read: bool,
) -> Result<SlotOutcome, SlotFailure> {
    // An index is not keyed by the table's primary key, so no `WHERE` clause over it
    // addresses one item of the table — which is what a batch read must do.
    let source = store.active_table(region, name, now)?;
    // A clause whose `OR` branches each ask for their own range of the source's keys asks
    // for ranges that overlap, which no one range read answers: that is a defect of the
    // CLAUSE, reported ahead of the rule about the key it pins
    // ([`crate::state::overlapping_range_key_conditions`]).
    if let (Some(filter), Some(sort_key)) = (
        select.filter.as_ref(),
        crate::state::source_sort_key_name(source, select.index.as_deref()),
    ) {
        if crate::state::overlapping_range_key_conditions(filter, &sort_key) {
            return Err(SlotFailure::validation(
                crate::state::OVERLAPPING_RANGE_KEY_MESSAGE,
            ));
        }
    }
    if select.index.is_some() {
        return Err(SlotFailure::validation(KEY_EQUALITY_MESSAGE));
    }
    // A projection item that is no document path is refused once the source it reads has
    // been resolved ([`crate::state::projection_defect`]).
    if let Some(defect) = crate::state::projection_defect(select.projection.as_ref()) {
        return Err(SlotFailure::from(defect));
    }
    // `ORDER BY` orders a result set; a batch read reports at most one item, so a
    // statement that asks for an order asks for something no batch read can serve.
    if select.order_by.is_some() {
        return Err(SlotFailure::validation(
            "ORDER BY is not supported for a read statement in a BatchExecuteStatement, which \
             reports at most one item",
        ));
    }
    let Some(filter) = select.filter.as_ref() else {
        return Err(SlotFailure::validation(KEY_EQUALITY_MESSAGE));
    };
    // "Each read statement in a BatchExecuteStatement must specify an equality condition
    // on all key attributes. This enforces that each SELECT statement in a batch returns
    // at most a single item" (the operation's doc): the clause states equality conditions,
    // so a term that is no equality — a boolean function, a comparison that is not `=`, a
    // membership test, a range, a branch of an `OR` — is no equality condition on a key
    // attribute and the statement asks for a read no single-item plan serves. It is
    // reported in this slot, with the sentence the rule is stated in
    // ([`crate::state::states_equalities_only`]), rather than applied as a filter over the
    // item the equalities beside it address: a statement this batch cannot serve must not
    // read as one that matched nothing.
    if !crate::state::states_equalities_only(filter) {
        return Err(SlotFailure::validation(KEY_EQUALITY_MESSAGE));
    }
    let condition = crate::state::parsed_condition(filter)?;
    let table = store.active_table(region, name, now)?;
    let key = addressed_key(table, filter)?;
    // The whole `WHERE` clause still decides: the key equalities address the item, and
    // any further predicate the stored item does not satisfy reports no item, exactly
    // as the single-statement path reports none.
    let found = match table.items.get(&key) {
        Some(item) if condition.evaluate(Some(item)).map_err(at_table(name))? => Some(item.clone()),
        _ => None,
    };
    let units = crate::state::read_capacity_units(
        found
            .as_ref()
            .map_or(0, |item| table.metered_item_size(item)),
        consistent_read,
    );
    // The read is ADMITTED against the read capacity the table is provisioned for, the
    // same as every other read surface: a statement past that capacity reports the
    // throttling code in its OWN slot rather than failing the batch as a whole, which is
    // how a batch reports a per-statement refusal ([`admit_write`] does it for a write).
    store
        .active_table_mut(region, name, now)?
        .admit_read(units, now)
        .map_err(|throttle| SlotFailure::new(throttle.code(), throttle.message()).at(name))?;
    Ok((
        BatchStatementResponse {
            item: found.map(|item| crate::state::project_item(&item, select.projection.as_ref())),
            ..BatchStatementResponse::default()
        },
        SlotCapacity {
            table: name.to_string(),
            consumed: SlotConsumption::Read(units),
        },
    ))
}

/// The one item a batch read addresses: the primary key its `WHERE` clause pins by
/// equality on EVERY key attribute, which is what makes a batch `SELECT` report at most
/// one item.
fn addressed_key(
    table: &crate::state::Table,
    filter: &crate::state::Predicate,
) -> Result<crate::state::ItemKey, SlotFailure> {
    let pinned = crate::state::pinned_equalities(filter);
    let mut addressed = crate::state::Item::new();
    let key_attributes = ::std::iter::once(table.partition_key_name().to_string())
        .chain(table.sort_key_name().map(ToString::to_string));
    for attribute in key_attributes {
        let Some(value) = pinned.get(&attribute) else {
            return Err(SlotFailure::validation(KEY_EQUALITY_MESSAGE));
        };
        addressed.insert(attribute, value.clone());
    }
    Ok(table.key_of_key_map(&addressed)?)
}

/// `INSERT`: the item the statement carries, which must not already be stored.
fn insert_item(
    store: &mut Store,
    region: &str,
    name: &str,
    now: ::std::time::SystemTime,
    insert: &crate::state::InsertStatement,
) -> Result<SlotOutcome, SlotFailure> {
    let mut item = insert.item.clone();
    for (attribute, value) in &item {
        crate::state::check_attribute_value(attribute, value)?;
    }
    crate::state::canonicalize_item(&mut item)?;
    if crate::state::item_size(&item) > crate::state::MAX_ITEM_SIZE_BYTES {
        return Err(SlotFailure::validation(
            crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
        ));
    }
    let key = {
        let table = store.active_table(region, name, now)?;
        // The half of the item-size quota that needs the table: the entry a local
        // secondary index of it would hold for this item is charged beside the item
        // ([`crate::state::Table::item_size_exceeded`]).
        if table.item_size_exceeded(&item) {
            return Err(SlotFailure::validation(
                crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
            ));
        }
        table.check_index_keys(&item)?;
        table.key_of_item(&item)?
    };
    if store.active_table(region, name, now)?.items.contains_key(&key) {
        return Err(
            SlotFailure::new(DUPLICATE_ITEM_CODE, DUPLICATE_ITEM_MESSAGE).at(name),
        );
    }
    let capacity = store
        .active_table(region, name, now)?
        .write_capacity(None, Some(&item));
    admit_write(store, region, name, now, &key, &capacity)?;
    store
        .put_item(region, name, now, key, item.clone())
        .map_err(at_table(name))?;
    Ok((
        // An `INSERT` states no `RETURNING` clause, so its slot reports the write and
        // no item.
        BatchStatementResponse::default(),
        SlotCapacity {
            table: name.to_string(),
            consumed: SlotConsumption::Write(capacity),
        },
    ))
}

/// `UPDATE`: the one item the `WHERE` clause pins, changed by the statement's clauses.
fn update_item(
    store: &mut Store,
    region: &str,
    name: &str,
    now: ::std::time::SystemTime,
    update: &crate::state::UpdateStatement,
    return_old_on_failure: bool,
) -> Result<SlotOutcome, SlotFailure> {
    let condition = crate::state::parsed_condition(&update.filter)?;
    let clauses = crate::state::parsed_update(&update.clauses)?;
    let (key, partition_key, sort_key) = {
        let table = store.active_table(region, name, now)?;
        (
            crate::state::singleton_key(table, &update.filter)?,
            table.partition_key_name().to_string(),
            table.sort_key_name().map(ToString::to_string),
        )
    };
    let before = store
        .active_table(region, name, now)?
        .items
        .get(&key)
        .cloned();
    // The `WHERE` clause is read against the item as it stands BEFORE this write, so a
    // predicate the stored item does not satisfy — and a key that holds no item at all
    // — is the condition failure rather than a write.
    let satisfied = before
        .as_ref()
        .map(|item| condition.evaluate(Some(item)))
        .transpose()
        .map_err(at_table(name))?
        .unwrap_or(false);
    if !satisfied {
        // The statement is refused and nothing is written, but its slot is METERED like the
        // write it would have made: a conditional write that fails consumes the write
        // capacity the stored item costs
        // ([`crate::state::charge_refused_conditional_write`]).
        crate::state::charge_refused_conditional_write(
            store,
            region,
            name,
            &key.partition,
            before.as_ref(),
            now,
        );
        return Err(condition_failed(
            if return_old_on_failure { before } else { None },
        )
        .at(name));
    }

    let mut item = before.clone().unwrap_or_default();
    // What a clause DOES is decided against the value the stored item already holds — an
    // operand of the wrong type for it is the stored data's answer to the statement — so
    // from here every refusal is one the statement earned at its table ([`at_table`]).
    let touched = clauses
        .apply(&mut item, crate::state::UpdateTarget::of(before.as_ref()))
        .map_err(at_table(name))?;
    for attribute in touched.attributes() {
        if attribute.as_str() == partition_key.as_str()
            || Some(attribute.as_str()) == sort_key.as_deref()
        {
            return Err(SlotFailure::validation(format!(
                "One or more parameter values were invalid: Cannot update attribute \
                 {attribute}. This attribute is part of the key"
            ))
            .at(name));
        }
    }
    // The item the statement leaves is held to exactly what a `PutItem` of it would be,
    // with the size refusal the UPDATE path states
    // ([`crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE`]).
    for (attribute, value) in &item {
        crate::state::check_attribute_value(attribute, value)
            .map_err(crate::state::stated_for_an_update)
            .map_err(at_table(name))?;
    }
    crate::state::canonicalize_item(&mut item).map_err(at_table(name))?;
    // A statement states its change without an expression, so nothing is charged on top of
    // the item it leaves ([`crate::state::update_size_exceeded`]): no clause frame and no
    // floor under any part of it.
    {
        let table = store.active_table(region, name, now)?;
        if crate::state::update_size_exceeded(
            table,
            &item,
            &crate::state::UpdateRequestShape::StatedWithoutAnExpression,
        ) {
            return Err(SlotFailure::validation(
                crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE,
            )
            .at(name));
        }
        table
            .check_index_keys_of_update(&item)
            .map_err(at_table(name))?;
    }
    let capacity = store
        .active_table(region, name, now)?
        .write_capacity(before.as_ref(), Some(&item));
    admit_write(store, region, name, now, &key, &capacity)?;
    store
        .put_item(region, name, now, key, item.clone())
        .map_err(at_table(name))?;

    let reported = crate::state::projected_return_values(
        update
            .returning
            .map(crate::state::ReturnedValues::as_return_value),
        before.as_ref(),
        Some(&item),
        &touched,
    );
    Ok((
        BatchStatementResponse {
            item: reported,
            ..BatchStatementResponse::default()
        },
        SlotCapacity {
            table: name.to_string(),
            consumed: SlotConsumption::Write(capacity),
        },
    ))
}

/// `DELETE`: the one item the `WHERE` clause pins, removed from the table.
fn delete_item(
    store: &mut Store,
    region: &str,
    name: &str,
    now: ::std::time::SystemTime,
    delete: &crate::state::DeleteStatement,
    return_old_on_failure: bool,
) -> Result<SlotOutcome, SlotFailure> {
    let (key, condition) = {
        let table = store.active_table(region, name, now)?;
        (
            crate::state::singleton_key(table, &delete.filter)?,
            crate::state::parsed_condition_beside_the_key(table, &delete.filter)?,
        )
    };
    let before = store
        .active_table(region, name, now)?
        .items
        .get(&key)
        .cloned();
    // See [`crate::ops::execute_statement`]: the key the clause pins ADDRESSES the item,
    // so only what the clause states besides it is the condition this delete is held to —
    // held to the addressed item, and a key that holds none is nothing to remove.
    let satisfied = crate::state::delete_condition_satisfied(condition.as_ref(), before.as_ref())
        .map_err(at_table(name))?;
    if !satisfied {
        // See [`update_item`]: the refused slot writes nothing and is metered all the same
        // ([`crate::state::charge_refused_conditional_write`]).
        crate::state::charge_refused_conditional_write(
            store,
            region,
            name,
            &key.partition,
            before.as_ref(),
            now,
        );
        return Err(condition_failed(
            if return_old_on_failure { before } else { None },
        )
        .at(name));
    }
    let capacity = store
        .active_table(region, name, now)?
        .write_capacity(before.as_ref(), None);
    admit_write(store, region, name, now, &key, &capacity)?;
    let removed = store
        .delete_item(region, name, now, &key)
        .map_err(at_table(name))?;

    // A delete removes every attribute of the item, so both OLD forms of a `RETURNING`
    // clause report the item as it stood.
    let reported = delete
        .returning
        .and(removed)
        .filter(|item| !item.is_empty());
    Ok((
        BatchStatementResponse {
            item: reported,
            ..BatchStatementResponse::default()
        },
        SlotCapacity {
            table: name.to_string(),
            consumed: SlotConsumption::Write(capacity),
        },
    ))
}

/// Admit a write of `capacity` against both ceilings a write is held to — the capacity
/// the table is provisioned for and the per-partition-key ceiling beneath it — or report
/// the slot failure a throttled statement earns.
///
/// A statement whose write neither budget covers is not applied, and the slot says so
/// with the throttling code rather than the batch failing as a whole: the other
/// statements of the batch are served ([`crate::state::Table::admit_item_write`]).
fn admit_write(
    store: &mut Store,
    region: &str,
    name: &str,
    now: ::std::time::SystemTime,
    key: &crate::state::ItemKey,
    capacity: &crate::state::WriteCapacity,
) -> Result<(), SlotFailure> {
    store
        .active_table_mut(region, name, now)?
        .admit_item_write(&key.partition, capacity, now)
        .map_err(|throttle| SlotFailure::new(throttle.code(), throttle.message()).at(name))
}

/// The slot failure a write whose `WHERE` clause the stored item does not satisfy
/// reports, carrying that item when the request asked for it.
fn condition_failed(item: Option<crate::state::Item>) -> SlotFailure {
    SlotFailure::new(
        CONDITIONAL_CHECK_FAILED_CODE,
        crate::state::CONDITION_FAILED_MESSAGE,
    )
    .with_item(item)
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> BatchExecuteStatementError {
    BatchExecuteStatementErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}
