// SPDX-License-Identifier: CC0-1.0

//! @op ExecuteTransaction
//! IMPLEMENTED: runs a list of PartiQL statements as ONE transaction against the SAME
//! item store the classic item API, `ExecuteStatement` and `TransactWriteItems` read
//! and write, so what a transaction leaves is visible to every later read and what an
//! earlier call wrote is what its statements are read against. A write transaction that
//! COMMITS publishes one record per change it made on the stream of the table that
//! change was made to ([`crate::state::publish_item_changes`]); a cancelled one made no
//! modification, so its tables' streams carry none.
//!
//! "The entire transaction must consist of either read statements or write statements,
//! you cannot mix both in one transaction": a mixed list is refused as a whole, and the
//! two shapes a well-formed transaction takes are
//!
//! * a READ transaction — every statement a `SELECT` that addresses ONE item by an
//!   equality on every key attribute, reporting it (projected as the statement asks)
//!   through `Responses` in statement order and nothing for a key that holds no item.
//!   It writes nothing at all.
//! * a WRITE transaction — `INSERT`, `UPDATE` and `DELETE`, applied all-or-nothing.
//!   Every statement's `WHERE` clause is read against the item as it stood BEFORE the
//!   transaction, and only when every one of them holds does any write land; otherwise
//!   NOTHING is written and the call is `TransactionCanceledException`, whose
//!   `CancellationReasons` carry one reason per statement in request order —
//!   `ConditionalCheckFailed` for each predicate that was not met (with that statement's
//!   item when its `ReturnValuesOnConditionCheckFailure` is `ALL_OLD`) and `None` for
//!   the rest. An `INSERT` is a put that must not replace, so a primary key that already
//!   holds an item is a collision on that key and reports `DuplicateItem` — the code the
//!   batch surface reports for the same collision — rather than a condition the statement
//!   never stated.
//!
//! The statements are parsed by [`crate::state::partiql`] and the writes land through
//! the same store write sites `PutItem`, `UpdateItem` and `DeleteItem` use, so the
//! dialect, number canonicalization, the documented item-size and index-key limits and
//! the capacity accounting have ONE implementation in this crate, shared with
//! `ExecuteStatement`, `BatchExecuteStatement` and the classic item API. A statement
//! naming a table that is absent — or not yet `ACTIVE` — is a failure of THAT statement,
//! so the transaction is cancelled and that statement's `CancellationReasons` entry
//! carries `ResourceNotFound` ([`cancelled_absent_table`]); a table whose resource-based
//! policy denies the caller the statement's own PartiQL action is `AccessDeniedException`,
//! and a transaction addressing one item twice is a `ValidationException` — as is a
//! statement whose change would leave an item past the documented item-size ceiling, which
//! is the ITEM's ceiling and so is refused in the class and the sentence a single-item
//! update of it is refused with. A statement run
//! here performs its action INSIDE this operation, which is what a policy admitting an
//! action only as part of a transaction tests
//! ([`crate::state::authorization::EnclosingOperation`]).
//!
//! `ClientRequestToken` makes a write transaction idempotent: a repeat of a token
//! inside the documented 10-minute window
//! ([`crate::state::TRANSACTION_IDEMPOTENCY_WINDOW`]) applies NOTHING and answers from
//! what that call DID, and a repeat carrying different statements is the documented
//! `IdempotentParameterMismatchException`. What a token stands for is
//! [`crate::state::CompletedTransaction`], remembered per REGION — a table name
//! resolves inside one region — and keyed by a fingerprint of the statements the first
//! call carried, so the store keeps a constant size per token. A read transaction
//! changes nothing, so it records no token.
//!
//! A transactional read or write costs twice what the same single-item one costs
//! ([`crate::state::TRANSACTIONAL_CAPACITY_FACTOR`]); `ReturnConsumedCapacity` reports
//! it as `ConsumedCapacityMultiple` does — ONE entry per table the transaction reached,
//! whose figure is the SUM over the statements that reached it, each figure read back
//! from that table.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ExecuteTransaction::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `ExecuteTransaction`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! This operation allows you to perform transactional reads or writes on data stored in
//! DynamoDB, using PartiQL. The entire transaction must consist of either read statements
//! or write statements, you cannot mix both in one transaction. The EXISTS function is an
//! exception and can be used to check the condition of specific attributes of the item in a
//! similar manner to ConditionCheck in the TransactWriteItems API.
//!
//! ## Input
//!
//! * `ClientRequestToken` (ClientRequestToken, len 1..36, @idempotencyToken) — Set this value
//!   to get remaining results, if NextToken was returned in the statement response.
//! * `ReturnConsumedCapacity` (ReturnConsumedCapacity, one of: INDEXES | NONE | TOTAL) —
//!   Determines the level of detail about either provisioned or on-demand throughput
//!   consumption that is returned in the response. For more information, see TransactGetItems
//!   and TransactWriteItems.
//! * `TransactStatements` (ParameterizedStatements, len 1..100) **required** — The list of
//!   PartiQL statements representing the transaction to run.
//!
//! ## Output
//!
//! * `ConsumedCapacity` (ConsumedCapacityMultiple) — The capacity units consumed by the
//!   entire operation. The values of the list are ordered according to the ordering of the
//!   statements.
//! * `Responses` (ItemResponseList, len 1..100) — The response to a PartiQL transaction.
//!
//! ## Errors
//!
//! * `IdempotentParameterMismatchException` (HTTP 400) — DynamoDB rejected the request
//!   because you retried a request with a different payload but with an idempotent token that
//!   was already used.
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The request was denied due to
//!   request throttling. For detailed information about why the request was throttled and the
//!   ARN of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   The Amazon Web Services SDKs for DynamoDB automatically retry requests that receive this
//!   exception. Your request is eventually successful, unless your retry queue is too large
//!   to finish. Reduce the frequency of requests and use exponential backoff. For more
//!   information, go to Error Retries and Exponential Backoff in the Amazon DynamoDB
//!   Developer Guide.
//! * `RequestLimitExceeded` (HTTP 400) — Throughput exceeds the current throughput quota for
//!   your account. For detailed information about why the request was throttled and the ARN
//!   of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   Contact Amazon Web Services Support to request a quota increase.
//! * `ResourceNotFoundException` (HTTP 400) — The operation tried to access a nonexistent
//!   table or index. The resource might not be specified correctly, or its status might not
//!   be ACTIVE.
//! * `ThrottlingException` (HTTP 400) — The request was denied due to request throttling. For
//!   detailed information about why the request was throttled and the ARN of the impacted
//!   resource, find the ThrottlingReason field in the returned exception.
//! * `TransactionCanceledException` (HTTP 400) — The entire transaction request was canceled.
//!   DynamoDB cancels a TransactWriteItems request under the following circumstances: A
//!   condition in one of the condition expressions is not met. A table in the
//!   TransactWriteItems request is in a different account or region. More than one action in
//!   the TransactWriteItems operation targets the same item. There is insufficient
//!   provisioned capacity for the transaction to be completed. An item size becomes too large
//!   (larger than 400 KB), or a local secondary index (LSI) becomes too large, or a similar
//!   validation error occurs because of changes made by the transaction. There is a user
//!   error, such as an invalid data format. There is an ongoing …
//! * `TransactionInProgressException` (HTTP 400) — The transaction with the given request
//!   token is already in progress. Recommended Settings This is a general recommendation for
//!   handling the TransactionInProgressException. These settings help ensure that the client
//!   retries will trigger completion of the ongoing TransactWriteItems request. Set
//!   clientExecutionTimeout to a value that allows at least one retry to be processed after 5
//!   seconds have elapsed since the first attempt for the TransactWriteItems operation. Set
//!   socketTimeout to a value a little lower than the requestTimeout setting. requestTimeout
//!   should be set based on the time taken for the individual retries of a single HTTP
//!   request for your use case, but setting it to 1 second or …

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ExecuteTransactionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ExecuteTransactionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#ExecuteTransaction")]
#[derive(Debug)]
pub enum ExecuteTransactionErrorModeled {
    IdempotentParameterMismatchException(IdempotentParameterMismatchException),
    InternalServerError(InternalServerError),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    TransactionCanceledException(TransactionCanceledException),
    TransactionInProgressException(TransactionInProgressException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::execute_transaction::ExecuteTransactionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ExecuteTransactionError = crate::scaffolded::Scaffolded<ExecuteTransactionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ExecuteTransactionErrorModeled> for ExecuteTransactionError {
    fn from(modeled: ExecuteTransactionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ExecuteTransaction` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ExecuteTransaction<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ExecuteTransaction<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ExecuteTransaction<Eng>
{
    type Input = ExecuteTransactionInput;
    type Output = ExecuteTransactionOutput;
    type Error = ExecuteTransactionError;

    const NAME: &'static str = "ExecuteTransaction";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXECUTE_TRANSACTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXECUTE_TRANSACTION_OUTPUT_SCHEMA;
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

        // Every statement is parsed before any of them runs: what the statements ARE
        // decides whether the TRANSACTION is well formed, and a transaction is applied
        // only as a whole. A value a `?` marker stands for is held to the same rules a
        // stored value is — the documented nesting depth, and this emulator's ceiling on
        // a collection's members — before the statement is parsed.
        let mut parsed: Vec<crate::state::PartiqlStatement> =
            Vec::with_capacity(input.transact_statements.len());
        for (slot, entry) in input.transact_statements.iter().enumerate() {
            // "An optional parameter that returns the item attributes for a PartiQL
            // ParameterizedStatement operation that failed a condition check": the
            // member carries two values.
            if !matches!(
                entry.return_values_on_condition_check_failure.as_deref(),
                None | Some("NONE" | "ALL_OLD")
            ) {
                return Err(crate::scaffolded::Scaffolded::validation(
                    "ReturnValuesOnConditionCheckFailure can only be ALL_OLD or NONE",
                ));
            }
            let parameters = entry.parameters.clone().unwrap_or_default();
            for (position, value) in parameters.iter().enumerate() {
                crate::state::check_statement_value(&format!("Parameters[{position}]"), value)
                    .map_err(lift)?;
            }
            // A statement of a transaction is refused as the SLOT it was sent in: the
            // statement's own defect, behind the position the request stated it at, so a
            // caller carrying several statements can tell which of them is wrong.
            let statement = crate::state::parse_statement(&entry.statement, &parameters)
                .map_err(|error| lift(in_slot(slot, error)))?;
            // A read addressed at an INDEX is admitted by no transaction: the statement
            // is well formed and names the index's own key, and it is the index that a
            // transaction cannot read — so the refusal is the top-level validation of
            // this request, behind the slot that carried the statement, and nothing of
            // the transaction is attempted or cancelled.
            if matches!(
                &statement,
                crate::state::PartiqlStatement::Select(select) if select.index.is_some()
            ) {
                return Err(lift(in_slot(
                    slot,
                    crate::state::RequestError::Validation(
                        INDEX_READ_IN_TRANSACTION_MESSAGE.to_string(),
                    ),
                )));
            }
            // A statement of a transaction reports no item of its own: "the entire
            // transaction must succeed or fail", so a write of one commits with the rest
            // and there is no per-statement image for a `RETURNING` clause to project.
            // A clause that asks for one is refused BEFORE anything is applied, behind
            // the slot that carried it, so the transaction commits nothing.
            if statement.returning().is_some() {
                return Err(lift(in_slot(
                    slot,
                    crate::state::RequestError::Validation(
                        RETURNING_IN_TRANSACTION_MESSAGE.to_string(),
                    ),
                )));
            }
            parsed.push(statement);
        }

        // "The entire transaction must consist of either read statements or write
        // statements, you cannot mix both in one transaction" — a rule about the
        // REQUEST, so a mixed transaction is refused whole and nothing of it is applied.
        let reads = parsed
            .iter()
            .filter(|statement| matches!(statement, crate::state::PartiqlStatement::Select(_)))
            .count();
        if reads > 0 && reads < parsed.len() {
            return Err(crate::scaffolded::Scaffolded::validation(
                MIXED_TRANSACTION_MESSAGE,
            ));
        }

        // The canonical form is built from the REQUEST's own statements — nothing of
        // what the store holds — which is what makes it comparable across the retries a
        // `ClientRequestToken` covers, and it covers the WHOLE request the token stands
        // for: the member that chooses what capacity the response reports is encoded
        // beside the statements ([`crate::state::canonical_reporting_options`]), so a
        // retry that changes it is the documented mismatch rather than a replay.
        let canonical = crate::state::action_fingerprint(&format!(
            "{}{}",
            crate::state::canonical_reporting_options(&[input.return_consumed_capacity.as_deref()]),
            canonical_statements(&input.transact_statements),
        ));
        let token = input
            .client_request_token
            .as_deref()
            .filter(|token| !token.is_empty());

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);

        // A token still inside its window stands for the transaction it was first used
        // for IN THIS REGION: identical statements are answered from what that call DID
        // rather than applied again, and different statements are the documented
        // mismatch. Both decisions precede every predicate below, because a retry must
        // not be judged against the state its own first attempt left.
        if let Some(token) = token {
            if let Some(completed) = store.recorded_transaction(&region, token).cloned() {
                if completed.actions != canonical {
                    return Err(
                        ExecuteTransactionErrorModeled::IdempotentParameterMismatchException(
                            IdempotentParameterMismatchException {
                                message: Some(
                                    crate::state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE
                                        .to_string(),
                                ),
                            },
                        )
                        .into(),
                    );
                }
                // The repeat applies nothing, so it writes no capacity: "subsequent
                // calls with the same client token return the number of READ capacity
                // units consumed in reading the item", and "the responses to the calls
                // might not be the same" even though the effect on the server is. The
                // figure is the one the transaction's own items weigh, reported as the
                // read it was.
                // The repeat applied nothing and is billed as the read it was, so the read
                // half of the tables' throughput is what it counts
                // ([`crate::state::charge_reported_ledger`]).
                crate::state::charge_reported_ledger(
                    &mut store,
                    &region,
                    &completed.ledger,
                    crate::state::Consumed::Read,
                    now,
                );
                return Ok(ExecuteTransactionOutput {
                    // The repeat re-applies nothing, so no statement of it produced an
                    // item to report ([`reported_responses`]).
                    responses: reported_responses(parsed.iter().map(|_| None)),
                    consumed_capacity: completed.ledger.consumed_capacity(
                        requested_capacity,
                        crate::state::CapacityKind::Read,
                        crate::state::KindReport::Named,
                    ),
                });
            }
        }

        if reads == parsed.len() {
            return read_transaction(
                &*self.engine,
                &mut store,
                &caller,
                &region,
                now,
                &parsed,
                requested_capacity,
            );
        }
        let answered = write_transaction(
            &*self.engine,
            &mut store,
            &caller,
            &region,
            now,
            &input.transact_statements,
            &parsed,
            token,
            &canonical,
            requested_capacity,
        );
        // Every change the transaction made reaches its table's stream before the
        // response does; a cancelled one rolls its queued changes back with the items,
        // so there is nothing to hand over for it.
        crate::state::publish_item_changes(&*self.engine, &mut store, now);
        answered
    }
}

/// This service's store, as the two transaction paths below take it.
type Store = crate::state::ComAmazonawsDynamodbDynamodb20120810State;

/// How a transaction that mixes reads and writes is refused: the fixed sentence DynamoDB
/// states that rule with, which is NOT the prose the operation's doc explains it in.
///
/// The doc says what the rule IS ("The entire transaction must consist of either read
/// statements or write statements, you cannot mix both in one transaction"); the refusal
/// is one sentence about the REQUEST, naming the operation that does not support it and
/// nothing of the statements, so no part of it comes from what the request carried.
const MIXED_TRANSACTION_MESSAGE: &str =
    "ExecuteTransaction API does not support both read and write operations in the same request.";

/// How a `SELECT` that does not address ONE item of the table is refused.
///
/// A transactional read reports the item its statement names, so the `WHERE` clause must
/// pin every key attribute by equality. The sentence is the one this surface states that
/// rule with, named for the operation that enforces it exactly as `BatchExecuteStatement`
/// names itself in its own.
const SINGLE_ITEM_READ_MESSAGE: &str = "Select statements within ExecuteTransaction must specify \
                                        the primary key in the where clause.";

/// How a `SELECT` addressed at an INDEX is refused.
///
/// A transaction reads items of a TABLE, and an index holds a projection of them keyed by
/// its own schema, so an index is a source no statement of a transaction can read — which
/// is a different fact from a `WHERE` clause that pins no key ([`SINGLE_ITEM_READ_MESSAGE`]):
/// such a statement DOES pin the index's key. The sentence is fixed and names neither the
/// index nor the table; the slot it is stated behind ([`in_slot`]) is what locates the
/// statement in the request.
const INDEX_READ_IN_TRANSACTION_MESSAGE: &str =
    "Reads on indices are not supported within transactions.";

/// How a statement whose `RETURNING` clause asks for an item image is refused.
///
/// A transaction answers `Responses` for the statements it ran and nothing else — it is
/// applied as a whole, so no statement of it has an image of its own to project — and a
/// `RETURNING` clause asks for exactly such an image. The sentence is fixed and names
/// neither the clause nor the table; the slot it is stated behind ([`in_slot`]) is what
/// locates the statement in the request, and the refusal precedes every write, so a
/// transaction carrying one changes nothing.
const RETURNING_IN_TRANSACTION_MESSAGE: &str =
    "RETURNING clause is not supported in ExecuteTransaction.";

/// One statement's refusal, reported against the slot the request sent that statement
/// in.
///
/// `TransactStatements` is a list, so WHICH statement was refused is a fact of the
/// request: a defect of one statement's own text is stated behind its position in that
/// list, and the statement's refusal follows unchanged.
fn in_slot(slot: usize, error: crate::state::RequestError) -> crate::state::RequestError {
    match error {
        crate::state::RequestError::Validation(message) => {
            crate::state::RequestError::Validation(format!(
                "Validation failed in TransactStatements[{slot}]: {message}"
            ))
        }
        error => error,
    }
}

/// How a `SELECT` asking for an order is refused: `ORDER BY` orders a result set, and a
/// transactional read reports at most one item, so an order is something no statement of
/// a transaction can ask for.
const ORDER_BY_MESSAGE: &str =
    "ORDER BY is not supported for a read statement in a transaction, which reports at most one \
     item";

/// The request's statements, canonically encoded: what a repeated `ClientRequestToken`
/// is compared against (through [`crate::state::action_fingerprint`], which is what the
/// store keeps).
///
/// Everything that decides what the transaction DOES is encoded, in request order: the
/// statement text, the values its `?` markers stand for, and whether it reports its item
/// when a condition is not met. Each statement's text is length-prefixed and the
/// parameters go through [`crate::state::canonical_item_text`] under their POSITION, so
/// two identical calls encode identically whatever order their JSON maps — or their set
/// members — arrived in, and no two different requests encode the same way.
fn canonical_statements(entries: &[ParameterizedStatement]) -> String {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&format!("{}={};", entry.statement.len(), entry.statement));
        // The position is zero-padded so the name order the canonical encoder emits in
        // is the order the parameters were sent in; a statement cannot carry more
        // markers than it has characters ([`crate::state::MAX_STATEMENT_BYTES`]), so six
        // digits cover every position a request can reach.
        let positional: crate::state::Item = entry
            .parameters
            .iter()
            .flatten()
            .enumerate()
            .map(|(position, value)| (format!("{position:06}"), value.clone()))
            .collect();
        text.push_str(&crate::state::canonical_item_text(&positional));
        text.push_str(
            entry
                .return_values_on_condition_check_failure
                .as_deref()
                .unwrap_or("NONE"),
        );
        text.push('\n');
    }
    text
}

/// A READ transaction: each `SELECT` reports the ONE item it addresses, and the store is
/// not written at all.
fn read_transaction<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    store: &mut Store,
    caller: &crate::state::authorization::Caller,
    region: &str,
    now: ::std::time::SystemTime,
    parsed: &[crate::state::PartiqlStatement],
    requested_capacity: Option<&str>,
) -> Result<ExecuteTransactionOutput, ExecuteTransactionError> {
    let mut responses: Vec<ItemResponse> = Vec::with_capacity(parsed.len());
    let mut keyed: ::std::collections::BTreeSet<(String, crate::state::ItemKey)> =
        ::std::collections::BTreeSet::new();
    // What each table this transaction reads consumed, totalled per table: what the
    // response REPORTS ([`ConsumedCapacityMultiple`] carries one entry per table, summed
    // over the statements that reached it), and what the read is admitted
    // ALL-OR-NOTHING against below, once every table's share is known, because a
    // transaction that is refused consumed nothing anywhere.
    let mut per_table: ::std::collections::BTreeMap<String, f64> =
        ::std::collections::BTreeMap::new();
    for (at, statement) in parsed.iter().enumerate() {
        let crate::state::PartiqlStatement::Select(select) = statement else {
            return Err(crate::scaffolded::Scaffolded::validation(
                MIXED_TRANSACTION_MESSAGE,
            ));
        };
        // A table a statement names that this region does not hold is a failure OF THAT
        // STATEMENT, the same way it is for a write ([`cancelled_absent_table`]).
        let absent = |error| cancelled_absent_table(at, parsed.len(), error);
        let name = crate::state::resolve_table_name_in(&select.table, region).map_err(absent)?;
        let table = store.active_table(region, &name, now).map_err(absent)?;
        // A table the policies in force do not let this caller act on with this
        // statement's own action is not read at all. PartiQL has its own four actions,
        // and the partition the statement addresses — with whether it scans its whole
        // source — is what a conditioned statement is decided against. A statement of a
        // transaction performs that action INSIDE this operation, which is what a statement
        // admitting it only as part of one tests.
        if let Some(denial) = crate::state::authorization::denial_indexed(
            caller,
            table,
            statement.index(),
            statement.action(),
            now,
            &crate::state::authorization::RequestKeys::of_statement(table, statement)
                .within(crate::state::authorization::EnclosingOperation::ExecuteTransaction),
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }
        // The items are stored ENCRYPTED under the table's KMS key, so a key KMS will
        // not use leaves nothing this statement could decrypt — and a transaction is
        // served whole or not at all ([`crate::state::check_table_key_accessible`]).
        crate::state::check_table_key_accessible(engine, table).map_err(lift)?;
        // Every refusal from here on is about THIS statement, so each is stated behind
        // the slot the request sent it in ([`in_slot`]).
        let refused = |message: &str| {
            lift(in_slot(
                at,
                crate::state::RequestError::Validation(message.to_string()),
            ))
        };
        if select.order_by.is_some() {
            return Err(refused(ORDER_BY_MESSAGE));
        }
        // A projection item that is no document path is refused once the source it reads
        // has been resolved ([`crate::state::projection_defect`]).
        if let Some(defect) = crate::state::projection_defect(select.projection.as_ref()) {
            return Err(lift(in_slot(at, defect)));
        }
        let Some(filter) = select.filter.as_ref() else {
            return Err(refused(SINGLE_ITEM_READ_MESSAGE));
        };
        let condition = crate::state::parsed_condition(filter)
            .map_err(|error| lift(in_slot(at, error)))?;
        let key = crate::state::singleton_key(table, filter).map_err(|error| {
            // A `WHERE` clause that leaves a key attribute free addresses no ONE item of
            // the table, which this operation states in the sentence above rather than in
            // the one a single-statement write earns.
            if error.message() == crate::state::MANDATORY_KEY_EQUALITY_MESSAGE {
                return refused(SINGLE_ITEM_READ_MESSAGE);
            }
            lift(in_slot(at, error))
        })?;
        // "Transaction request cannot include multiple operations on one item."
        if !keyed.insert((name.clone(), key.clone())) {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::DUPLICATE_TRANSACT_ITEM_MESSAGE,
            ));
        }
        // The key equalities address the item, and any further predicate the stored item
        // does not satisfy reports no item — exactly as the single-statement path reports
        // none.
        let found = match table.items.get(&key) {
            Some(item) if condition.evaluate(Some(item)).map_err(lift)? => Some(item.clone()),
            _ => None,
        };
        // A key that holds no item still consumed the read it asked for, which is why
        // capacity is charged for every statement and only the ones that found an item
        // report one. "Transactional read requests require two read capacity units."
        let units = crate::state::read_capacity_units(
            found
                .as_ref()
                .map_or(0, |item| table.metered_item_size(item)),
            true,
        ) * crate::state::TRANSACTIONAL_CAPACITY_FACTOR;
        *per_table.entry(name.clone()).or_default() += units;
        responses.push(ItemResponse {
            // What a statement reports is the `ItemResponse` object, which carries the
            // projected pairs the read came to and states both of the facts that leave it
            // with none — no item at the key, and a projection that named nothing of the
            // item it found — as the object with nothing in it
            // ([`crate::state::item_response_item`]).
            item: crate::state::item_response_item(
                found.map(|item| crate::state::project_item(&item, select.projection.as_ref())),
            ),
        });
    }
    // The read capacity each table is provisioned for bounds a PartiQL read transaction
    // exactly as it bounds `TransactGetItems`: every table is asked whether it covers its
    // whole share BEFORE any of them is debited, so a refused transaction leaves every
    // budget where it was.
    for (name, units) in &per_table {
        if let Err(throttle) = store
            .active_table(region, name, now)
            .map_err(lift)?
            .read_admits(*units, now)
        {
            return Err(
                ExecuteTransactionErrorModeled::ProvisionedThroughputExceededException(
                    crate::state::read_throughput_exceeded(
                        &crate::state::table_arn(region, name),
                        throttle,
                    ),
                )
                .into(),
            );
        }
    }
    for (name, units) in &per_table {
        // Every table covered its share a moment ago, so this takes it.
        let _ = store
            .active_table_mut(region, name, now)
            .map_err(lift)?
            .admit_read(*units, now);
    }
    // "The capacity units consumed by the entire operation": `ConsumedCapacityMultiple`
    // carries ONE entry per table the transaction reached, whose figure is the sum over
    // the statements that reached it — not one entry per statement — and no member at all
    // when the request asked for no capacity detail.
    let consumed: Vec<ConsumedCapacity> = per_table
        .iter()
        .filter_map(|(name, units)| {
            crate::state::consumed_capacity(
                requested_capacity,
                name,
                *units,
                crate::state::KindReport::Named,
            )
        })
        .collect();
    Ok(ExecuteTransactionOutput {
        responses: Some(responses),
        consumed_capacity: (!consumed.is_empty()).then_some(consumed),
    })
}

/// A WRITE transaction: every statement resolved and read against the state as it stood
/// BEFORE the transaction, then applied all-or-nothing.
#[allow(clippy::too_many_arguments)]
fn write_transaction<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    store: &mut Store,
    caller: &crate::state::authorization::Caller,
    region: &str,
    now: ::std::time::SystemTime,
    entries: &[ParameterizedStatement],
    parsed: &[crate::state::PartiqlStatement],
    token: Option<&str>,
    canonical: &str,
    requested_capacity: Option<&str>,
) -> Result<ExecuteTransactionOutput, ExecuteTransactionError> {
    let elements = parsed.len();
    // Pass one resolves every statement against the stored items and reads its
    // predicate, writing NOTHING: a transaction only lands once all of it is known to be
    // applicable.
    let mut planned: Vec<Planned> = Vec::with_capacity(elements);
    let mut keyed: ::std::collections::BTreeSet<(String, crate::state::ItemKey)> =
        ::std::collections::BTreeSet::new();
    for (at, (entry, statement)) in entries.iter().zip(parsed).enumerate() {
        // A refusal the ITEM this statement writes carries against the TABLE's own
        // schema — a key attribute of a type the table does not declare for it, its own
        // key's or a secondary index's — is decided when the transaction prepares that
        // item, so it cancels the transaction. A refusal about the VALUE the request
        // carried needs no table to decide, so it stays a request-level refusal.
        let keying = |error| cancelled_by_element(at, elements, error);
        // Every refusal about the item a statement's change would LEAVE is that STATEMENT's
        // failure and cancels the transaction ([`keying`]) — the documented item-size ceiling
        // excepted ([`crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE`]), which refuses the
        // CALL: the change the statement spells is one the update path does not admit, so the
        // request is refused on its own terms and none of the transaction is applied.
        let update_outcome = |error: crate::state::RequestError| match error {
            crate::state::RequestError::Validation(message)
                if message == crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE =>
            {
                crate::scaffolded::Scaffolded::validation(message)
            }
            other => keying(other),
        };
        // A table a statement names that this region does not hold — or does not hold in a
        // status that serves items — is a failure OF THAT STATEMENT, so it cancels the
        // transaction with that statement's own reason ([`cancelled_absent_table`]).
        let absent = |error| cancelled_absent_table(at, elements, error);
        let name =
            crate::state::resolve_table_name_in(statement.table(), region).map_err(absent)?;
        let returns_old_on_failure =
            entry.return_values_on_condition_check_failure.as_deref() == Some("ALL_OLD");
        {
            let table = store.active_table(region, &name, now).map_err(absent)?;
            // A statement of a transaction performs its own PartiQL action INSIDE this
            // operation, which is what a statement admitting that action only as part of
            // one tests.
            if let Some(denial) = crate::state::authorization::denial_indexed(
                caller,
                table,
                statement.index(),
                statement.action(),
                now,
                &crate::state::authorization::RequestKeys::of_statement(table, statement)
                    .within(crate::state::authorization::EnclosingOperation::ExecuteTransaction),
            ) {
                return Err(crate::scaffolded::Scaffolded::access_denied(denial));
            }
            // The items are stored ENCRYPTED under the table's KMS key, so a key KMS
            // will not use leaves nothing this statement could read or encrypt — and a
            // transaction is served whole or not at all, so NOTHING is written
            // ([`crate::state::check_table_key_accessible`]).
            crate::state::check_table_key_accessible(engine, table).map_err(lift)?;
        }
        let prepared = match statement {
            // Guarded by the read/write split above; stated so this match names every
            // statement the dialect parses.
            crate::state::PartiqlStatement::Select(_) => {
                return Err(crate::scaffolded::Scaffolded::validation(
                    MIXED_TRANSACTION_MESSAGE,
                ))
            }
            crate::state::PartiqlStatement::Insert(insert) => {
                let mut item = insert.item.clone();
                for (attribute, value) in &item {
                    crate::state::check_attribute_value(attribute, value).map_err(lift)?;
                }
                crate::state::canonicalize_item(&mut item).map_err(lift)?;
                if crate::state::item_size(&item) > crate::state::MAX_ITEM_SIZE_BYTES {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
                    ));
                }
                let table = store.active_table(region, &name, now).map_err(lift)?;
                // The half of the item-size quota that needs the table: the entry a local
                // secondary index of it would hold for this item is charged beside the
                // item ([`crate::state::Table::item_size_exceeded`]).
                if table.item_size_exceeded(&item) {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
                    ));
                }
                table.check_index_key_types(&item).map_err(keying)?;
                table.check_index_key_values(&item).map_err(lift)?;
                table.check_vector_attributes(&item).map_err(lift)?;
                let key = table.key_of_item(&item).map_err(keying)?;
                let before = table.items.get(&key).cloned();
                // An `INSERT` is a put that must not replace, so a primary key that
                // already holds an item is a KEY COLLISION this statement cannot get
                // past — which cancels the transaction, reporting the duplicate key
                // rather than a condition the statement never stated
                // ([`Planned::cancellation_code`]), instead of overwriting that item.
                let satisfied = before.is_none();
                Planned {
                    table: name,
                    key,
                    write: if satisfied {
                        PlannedWrite::Put(item)
                    } else {
                        PlannedWrite::Nothing
                    },
                    before,
                    satisfied,
                    duplicate_key: !satisfied,
                    returns_old_on_failure,
                }
            }
            crate::state::PartiqlStatement::Update(update) => {
                let condition = crate::state::parsed_condition(&update.filter).map_err(lift)?;
                let clauses = crate::state::parsed_update(&update.clauses).map_err(lift)?;
                let (key, partition_key, sort_key) = {
                    let table = store.active_table(region, &name, now).map_err(lift)?;
                    (
                        crate::state::singleton_key(table, &update.filter).map_err(lift)?,
                        table.partition_key_name().to_string(),
                        table.sort_key_name().map(ToString::to_string),
                    )
                };
                let before = store
                    .active_table(region, &name, now)
                    .map_err(lift)?
                    .items
                    .get(&key)
                    .cloned();
                // The `WHERE` clause is read against the item as it stood BEFORE the
                // transaction, which is the only state it can speak about — so a
                // statement whose predicate the stored item does not satisfy, and one
                // whose key holds no item at all, both cancel the transaction rather
                // than write.
                let satisfied = before
                    .as_ref()
                    .map(|item| condition.evaluate(Some(item)))
                    .transpose()
                    .map_err(lift)?
                    .unwrap_or(false);
                if !satisfied {
                    Planned {
                        table: name,
                        key,
                        write: PlannedWrite::Nothing,
                        before,
                        satisfied,
                        duplicate_key: false,
                        returns_old_on_failure,
                    }
                } else {
                    let mut item = before.clone().unwrap_or_default();
                    // The change is APPLIED to the stored item here, so a defect found by
                    // applying it is this statement's own failure and cancels the
                    // transaction ([`cancelled_by_element`]), exactly as the same defect
                    // does in a `TransactWriteItems` `Update` — the documented 400 KB
                    // ceiling excepted, which refuses the call ([`update_outcome`]).
                    let touched = clauses
                        .apply(&mut item, crate::state::UpdateTarget::of(before.as_ref()))
                        .map_err(update_outcome)?;
                    for attribute in touched.attributes() {
                        if attribute.as_str() == partition_key.as_str()
                            || Some(attribute.as_str()) == sort_key.as_deref()
                        {
                            return Err(crate::scaffolded::Scaffolded::validation(format!(
                                "One or more parameter values were invalid: Cannot update \
                                 attribute {attribute}. This attribute is part of the key"
                            )));
                        }
                    }
                    // The item the statement leaves is held to exactly what a `PutItem`
                    // of it would be: the documented nesting, the 400 KB item size, and
                    // the key limits of every index it is projected into. The size
                    // refusal is the one the UPDATE path states
                    // ([`crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE`]), and it refuses
                    // the CALL wherever this statement crosses it, as the size check below
                    // does; every other rule the applied item breaks is this statement's own
                    // failure.
                    for (attribute, value) in &item {
                        crate::state::check_attribute_value(attribute, value)
                            .map_err(crate::state::stated_for_an_update)
                            .map_err(update_outcome)?;
                    }
                    // Canonicalising the applied item stays the CALL's refusal, unlike the
                    // rules above it: what is stored is already canonical (every write path
                    // canonicalises, idempotently, at its write site), so the only values
                    // this can refuse are the ones the STATEMENT spelled — a defect of the
                    // request rather than something the transaction's change did.
                    crate::state::canonicalize_item(&mut item).map_err(lift)?;
                    {
                        let table = store.active_table(region, &name, now).map_err(lift)?;
                        // Crossing the ceiling refuses the CALL, as it does on the other
                        // transactional surface where the item still fits the quota: the change
                        // the statement spells is one the update path does not admit, so the
                        // request is refused on its own terms and none of the transaction is
                        // applied.
                        if crate::state::update_size_exceeded(
                            table,
                            &item,
                            &crate::state::UpdateRequestShape::StatedWithoutAnExpression,
                        ) {
                            return Err(crate::scaffolded::Scaffolded::validation(
                                crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE,
                            ));
                        }
                        table.check_index_key_types(&item).map_err(keying)?;
                        table
                            .check_index_key_values_of_update(&item)
                            .map_err(lift)?;
                        table.check_vector_attributes(&item).map_err(lift)?;
                    }
                    Planned {
                        table: name,
                        key,
                        write: PlannedWrite::Put(item),
                        before,
                        satisfied,
                        duplicate_key: false,
                        returns_old_on_failure,
                    }
                }
            }
            crate::state::PartiqlStatement::Delete(delete) => {
                let (key, condition) = {
                    let table = store.active_table(region, &name, now).map_err(lift)?;
                    (
                        crate::state::singleton_key(table, &delete.filter).map_err(lift)?,
                        crate::state::parsed_condition_beside_the_key(table, &delete.filter)
                            .map_err(lift)?,
                    )
                };
                let before = store
                    .active_table(region, &name, now)
                    .map_err(lift)?
                    .items
                    .get(&key)
                    .cloned();
                // See [`crate::ops::execute_statement`]: the key the clause pins
                // ADDRESSES the item, so only what the clause states besides it is the
                // condition this delete is held to — held to the addressed item, and a
                // key that holds none is nothing to remove.
                let satisfied = crate::state::delete_condition_satisfied(
                    condition.as_ref(),
                    before.as_ref(),
                )
                .map_err(lift)?;
                Planned {
                    table: name,
                    key,
                    write: if satisfied {
                        PlannedWrite::Delete
                    } else {
                        PlannedWrite::Nothing
                    },
                    before,
                    satisfied,
                    duplicate_key: false,
                    returns_old_on_failure,
                }
            }
            // `EXISTS(<select>)`: the check holds when the ONE item its inner `SELECT`
            // addresses is stored and satisfies whatever else that clause states. It
            // writes nothing either way — it is the condition the writes beside it are
            // applied under, so a check that does not hold cancels the transaction like
            // any other unmet predicate.
            crate::state::PartiqlStatement::Exists(select) => {
                // A projection item that is no document path is refused once the source
                // it reads has been resolved ([`crate::state::projection_defect`]).
                if let Some(defect) = crate::state::projection_defect(select.projection.as_ref()) {
                    return Err(lift(in_slot(at, defect)));
                }
                // The check must name the ONE item it is about and state something
                // besides that item's key
                // ([`crate::state::EXISTS_SINGLE_ITEM_MESSAGE`]).
                let malformed = || {
                    lift(in_slot(
                        at,
                        crate::state::RequestError::Validation(
                            crate::state::EXISTS_SINGLE_ITEM_MESSAGE.to_string(),
                        ),
                    ))
                };
                let Some(filter) = select.filter.as_ref() else {
                    return Err(malformed());
                };
                let (key, condition) = {
                    let table = store.active_table(region, &name, now).map_err(lift)?;
                    (
                        crate::state::singleton_key(table, filter).map_err(|error| {
                            if error.message() == crate::state::MANDATORY_KEY_EQUALITY_MESSAGE {
                                return malformed();
                            }
                            lift(in_slot(at, error))
                        })?,
                        crate::state::parsed_condition_beside_the_key(table, filter)
                            .map_err(|error| lift(in_slot(at, error)))?,
                    )
                };
                let Some(condition) = condition else {
                    return Err(malformed());
                };
                let before = store
                    .active_table(region, &name, now)
                    .map_err(lift)?
                    .items
                    .get(&key)
                    .cloned();
                // The check HOLDS when the item it names is stored and satisfies the
                // condition the clause states besides its key; a key that holds nothing
                // satisfies no condition, so the check does not hold and the transaction
                // is cancelled like any other unmet predicate.
                let satisfied = match before.as_ref() {
                    Some(item) => condition.evaluate(Some(item)).map_err(lift)?,
                    None => false,
                };
                Planned {
                    table: name,
                    key,
                    write: PlannedWrite::Check,
                    before,
                    satisfied,
                    duplicate_key: false,
                    returns_old_on_failure,
                }
            }
        };

        // "Transaction request cannot include multiple operations on one item."
        if !keyed.insert((prepared.table.clone(), prepared.key.clone())) {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::DUPLICATE_TRANSACT_ITEM_MESSAGE,
            ));
        }
        planned.push(prepared);
    }

    // "All of which must either succeed or fail": one unmet predicate cancels the whole
    // transaction, and the reasons report what each statement's own outcome was, in
    // request order.
    if planned.iter().any(|statement| !statement.satisfied) {
        // See [`crate::ops::transact_write_items`]: each statement whose predicate was not
        // met is metered at twice what the same single-item write would cost
        // ([`crate::state::charge_refused_transactional_write`]), though nothing is applied.
        for statement in planned.iter().filter(|statement| !statement.satisfied) {
            crate::state::charge_refused_transactional_write(
                store,
                region,
                &statement.table,
                &statement.key.partition,
                statement.before.as_ref(),
                now,
            );
        }
        let codes: Vec<&str> = planned.iter().map(Planned::cancellation_code).collect();
        let cancellation_reasons: Vec<CancellationReason> = planned
            .iter()
            .map(|statement| CancellationReason {
                code: Some(statement.cancellation_code().to_string()),
                message: statement.cancellation_detail().map(ToString::to_string),
                item: (!statement.satisfied && statement.returns_old_on_failure)
                    .then(|| statement.before.clone())
                    .flatten(),
            })
            .collect();
        return Err(
            ExecuteTransactionErrorModeled::TransactionCanceledException(
                TransactionCanceledException {
                    message: Some(crate::state::transaction_cancelled_message(&codes)),
                    cancellation_reasons: Some(cancellation_reasons),
                },
            )
            .into(),
        );
    }

    // Pass two applies them. Each figure is read back from the table the write landed
    // in, so the capacity fan-out is computed from stored state rather than from the
    // request, and it is totalled PER TABLE: `ConsumedCapacityMultiple` carries one entry
    // per table the transaction reached ([`crate::state::WriteLedger`]).
    let mut recorded = crate::state::WriteLedger::default();
    let mut undo: Vec<(String, crate::state::ItemKey, Option<crate::state::Item>)> = Vec::new();
    let mut refused: Option<ExecuteTransactionError> = None;
    // Where the change feeds of the tables this transaction writes stood before it
    // started: a transaction that is rolled back below made no modification, so its
    // queued changes go back with the items.
    let change_mark = store.queued_stream_record_count();
    for statement in &planned {
        let after = match &statement.write {
            PlannedWrite::Put(item) => Some(item.clone()),
            PlannedWrite::Delete | PlannedWrite::Check | PlannedWrite::Nothing => None,
        };
        if matches!(statement.write, PlannedWrite::Check) {
            // A check writes nothing, so it lands in no item collection; it is still part
            // of the transactional write and it READ one item, so the table is charged
            // for that item and no index is — exactly as the classic transactional
            // condition check is charged ([`crate::ops::transact_write_items`]).
            let capacity = crate::state::WriteCapacity {
                table: crate::state::write_capacity_units(
                    statement.before.as_ref().map_or(0, crate::state::item_size),
                ),
                ..crate::state::WriteCapacity::default()
            };
            recorded.record(&statement.table, &capacity);
            continue;
        }
        undo.push((
            statement.table.clone(),
            statement.key.clone(),
            statement.before.clone(),
        ));
        let landed = match &after {
            Some(item) => store
                .put_item(
                    region,
                    &statement.table,
                    now,
                    statement.key.clone(),
                    item.clone(),
                )
                .map(|_| ()),
            None => store
                .delete_item(region, &statement.table, now, &statement.key)
                .map(|_| ()),
        };
        if let Err(error) = landed {
            refused = Some(lift(error));
            break;
        }
        // Reading the table back is on the same all-or-nothing path as the write itself:
        // a failure here must roll back too, never return early past the undo below.
        match store.active_table(region, &statement.table, now) {
            Ok(table) => {
                let capacity = table.write_capacity(statement.before.as_ref(), after.as_ref());
                // "Transactional write requests require two write capacity units": what
                // this statement consumed is twice the same single-item write, totalled
                // into its table's own entry below
                // ([`crate::state::WriteLedger::scale_table_units`]).
                recorded.record(&statement.table, &capacity);
            }
            Err(error) => {
                refused = Some(lift(error));
                break;
            }
        }
    }
    if let Some(refused) = refused {
        // A transaction is all-or-nothing, so a refusal from the store's own budgets
        // part-way through puts back exactly what stood at every key it had already
        // reached — each of which fitted the store a moment ago.
        for (table, key, before) in undo.into_iter().rev() {
            let restored = match before {
                Some(item) => store.put_item(region, &table, now, key, item).map(|_| ()),
                None => store.delete_item(region, &table, now, &key).map(|_| ()),
            };
            if let Err(error) = restored {
                // Nothing a caller can be told makes this right, so it is LOGGED: a
                // store that could not be put back is observable to whoever runs the
                // emulator rather than silent.
                ::tracing::error!(
                    table = %table,
                    error = %error.message(),
                    "a cancelled transaction could not be rolled back"
                );
            }
        }
        // The restoring writes above queued changes of their own, so the queue is put
        // back where it stood: the feeds of these tables carry neither the cancelled
        // modifications nor their undo.
        store.rewind_stream_records(change_mark);
        return Err(refused);
    }

    recorded.scale_table_units(crate::state::TRANSACTIONAL_CAPACITY_FACTOR);
    // The transactional cost this statement set consumed is what the tables' throughput
    // series carry for it, the same figures the entries above report
    // ([`crate::state::charge_reported_ledger`]).
    crate::state::charge_reported_ledger(
        store,
        region,
        &recorded,
        crate::state::Consumed::Write,
        now,
    );
    // "The capacity units consumed by the entire operation": ONE entry per table the
    // transaction wrote to, whose figure is the sum over the statements that reached it,
    // and no member at all when the request asked for no capacity detail. Projected here,
    // before the ledger is handed to the token's record below.
    let consumed_capacity = recorded.consumed_capacity(
        requested_capacity,
        crate::state::CapacityKind::Write,
        crate::state::KindReport::Named,
    );
    if let Some(token) = token {
        store.record_transaction(
            region,
            token,
            crate::state::CompletedTransaction {
                actions: canonical.to_string(),
                completed_at: now,
                ledger: recorded,
            },
        );
    }

    Ok(ExecuteTransactionOutput {
        // A WRITE statement of a transaction produces no item to report: the transaction is
        // applied as a whole, so no statement of it has an image of its own, and the clause
        // that would ask for one is refused before anything is applied
        // ([`RETURNING_IN_TRANSACTION_MESSAGE`]).
        responses: reported_responses(planned.iter().map(|_| None)),
        consumed_capacity,
    })
}

/// The `Responses` member a transaction reports, over the item each of its statements
/// PRODUCED.
///
/// `ItemResponse` carries a single optional `Item` and nothing else, so an entry for a
/// statement that produced no item states nothing a caller can read: the list holds one
/// entry per item a statement produced, and a transaction where none did reports an
/// empty list rather than a run of empty structures.
fn reported_responses(
    produced: impl IntoIterator<Item = Option<crate::state::Item>>,
) -> Option<Vec<ItemResponse>> {
    Some(
        produced
            .into_iter()
            .flatten()
            .map(|item| ItemResponse { item: Some(item) })
            .collect(),
    )
}

/// One statement of a write transaction, resolved against the state as it stood before
/// the transaction and ready to apply.
struct Planned {
    /// The resolved name of the table it writes to.
    table: String,
    key: crate::state::ItemKey,
    write: PlannedWrite,
    /// The item that stood at `key` before the transaction: what the predicate was read
    /// against, what the capacity weighs, and what an unmet predicate reports.
    before: Option<crate::state::Item>,
    satisfied: bool,
    /// Whether what this statement asked for was a key holding NO item — an `INSERT`,
    /// which is a put that must not replace — rather than a condition the stored item
    /// had to satisfy. The two are different failures and report different reasons.
    duplicate_key: bool,
    returns_old_on_failure: bool,
}

impl Planned {
    /// The `CancellationReason$Code` this statement reports when the transaction is
    /// cancelled: the failure it had, or `None` for a statement nothing was wrong with.
    ///
    /// A statement that would store an item where one already stands collided on the
    /// primary key, which is a different failure from a condition the stored item did not
    /// satisfy — so it reports the key collision's own code
    /// ([`crate::state::DUPLICATE_ITEM_CODE`]), the one the batch surface reports for the
    /// same collision.
    fn cancellation_code(&self) -> &'static str {
        match (self.satisfied, self.duplicate_key) {
            (true, _) => crate::state::CANCELLATION_CODE_NONE,
            (false, true) => crate::state::DUPLICATE_ITEM_CODE,
            (false, false) => crate::state::CANCELLATION_CODE_CONDITION_FAILED,
        }
    }

    /// The sentence that reason carries, or `None` for a statement nothing was wrong
    /// with: each failure is stated in its own words, the same words the single-statement
    /// and batch paths state it in.
    fn cancellation_detail(&self) -> Option<&'static str> {
        match (self.satisfied, self.duplicate_key) {
            (true, _) => None,
            (false, true) => Some(crate::state::DUPLICATE_ITEM_MESSAGE),
            (false, false) => Some(crate::state::CONDITION_FAILED_MESSAGE),
        }
    }
}

/// What one statement writes once every predicate has held.
enum PlannedWrite {
    /// The item an `INSERT` or an `UPDATE` leaves at its key.
    Put(crate::state::Item),
    /// A `DELETE`, which removes whatever its key held.
    Delete,
    /// An `EXISTS` check, which writes nothing: it only has to HOLD for the writes
    /// beside it to land.
    Check,
    /// A statement whose predicate was not met, which writes nothing because the
    /// transaction carrying it is cancelled.
    Nothing,
}

/// Report a refusal the transaction decided for ONE statement — the item it writes
/// carrying a key attribute of a type the table does not declare, for its own key schema
/// or for a secondary index's — as the cancellation it is.
///
/// "All of which must either succeed or fail": a statement the transaction itself
/// refuses when it prepares that statement's item against the TABLE's schema cancels the
/// whole request, so the answer is the `TransactionCanceledException`
/// [`crate::state::cancelled_by_element_reasons`] builds — the documented reason shape,
/// shared with `TransactWriteItems` — wrapped in this operation's error. A refusal about
/// the REQUEST rather than about the item as the table's schema sees it is still
/// answered directly, which is what [`lift`] does with it.
fn cancelled_by_element(
    at: usize,
    elements: usize,
    error: crate::state::RequestError,
) -> ExecuteTransactionError {
    let crate::state::RequestError::Validation(message) = error else {
        return lift(error);
    };
    let (message, cancellation_reasons) =
        crate::state::cancelled_by_element_reasons(at, elements, &message);
    ExecuteTransactionErrorModeled::TransactionCanceledException(TransactionCanceledException {
        message: Some(message),
        cancellation_reasons: Some(cancellation_reasons),
    })
    .into()
}

/// Report the table ONE statement names not being there as the cancellation it is.
///
/// A statement of a transaction resolves its own table, so a name this region holds no
/// table for — or holds one for in a status that serves no item — is a failure OF THAT
/// STATEMENT rather than of the request: the transaction is cancelled, and the statement's
/// `CancellationReasons` entry carries
/// [`crate::state::CANCELLATION_CODE_RESOURCE_NOT_FOUND`] with the sentence the lookup
/// itself words, while every other statement's reason is `None` — which is what tells a
/// caller WHICH of its statements could not be resolved. A top-level
/// `ResourceNotFoundException` would hide that index and put the failure on the whole call.
///
/// Anything else the lookup refuses is about the REQUEST — a value that is no table name or
/// ARN of this endpoint at all — so it is answered directly, which is what [`lift`] does
/// with it.
fn cancelled_absent_table(
    at: usize,
    elements: usize,
    error: crate::state::RequestError,
) -> ExecuteTransactionError {
    let crate::state::RequestError::NotFound(message) = error else {
        return lift(error);
    };
    let (message, cancellation_reasons) = crate::state::cancelled_element_reasons(
        at,
        elements,
        crate::state::CANCELLATION_CODE_RESOURCE_NOT_FOUND,
        &message,
    );
    ExecuteTransactionErrorModeled::TransactionCanceledException(TransactionCanceledException {
        message: Some(message),
        cancellation_reasons: Some(cancellation_reasons),
    })
    .into()
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> ExecuteTransactionError {
    ExecuteTransactionErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> ExecuteTransactionError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            ExecuteTransactionErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}
