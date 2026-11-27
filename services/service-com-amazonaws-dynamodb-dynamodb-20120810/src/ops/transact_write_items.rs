// SPDX-License-Identifier: CC0-1.0

//! @op TransactWriteItems
//! IMPLEMENTED: applies every `TransactItems` action — `Put`, `Update`, `Delete` and
//! `ConditionCheck` — as ONE all-or-nothing write: "the entire transaction must consist of
//! [...] actions, all of which must either succeed or fail". Each action's condition is
//! read against the item as it stood BEFORE the transaction, and only when every one of
//! them holds does any write land; otherwise nothing is written and the call is
//! `TransactionCanceledException`, whose `CancellationReasons` carry one reason per action
//! in request order — `ConditionalCheckFailed` for each condition that was not met (with
//! that action's item when its `ReturnValuesOnConditionCheckFailure` is `ALL_OLD`),
//! `ValidationError` with the sentence it earned for each action that failed when it was
//! read against the stored item (an `UpdateExpression` operand the stored value has the
//! wrong type for, a document path that item cannot hold, an update that moves a key
//! attribute) and `None` for the rest. Every action is resolved before the answer is built,
//! so a transaction carrying both classes reports both — which is the only way a caller
//! can attribute what failed to the action that failed.
//!
//! The order the two classes are found in is the service's own. An action is first BOUND to
//! its table — the table it names, the policies in force, the values its map carries and
//! the key that map states against the table's own key schema — and only then READ against
//! the item its key holds: its condition, and the update it applies where that condition
//! held. So a key the table's schema does not admit is found before any stored item has
//! been read, and the transaction is cancelled there with every OTHER action reporting the
//! null code, having never been evaluated; and an action whose condition was not met
//! reports `ConditionalCheckFailed` alone, since its `UpdateExpression` is never applied to
//! the item that would have failed it.
//!
//! The writes land through the same store write sites `PutItem`, `UpdateItem` and
//! `DeleteItem` use, so numbers are canonicalized and capacity is accounted the same way
//! on this path as on the single-item one, and an `Update` applies the same
//! `UpdateExpression` language ([`crate::state::ParsedUpdate`]). A transaction that
//! COMMITS publishes one record per change it made on the stream of the table that
//! change was made to ([`crate::state::publish_item_changes`]); a cancelled one made no
//! modification, so its tables' streams carry none.
//!
//! "Transaction requests cannot include multiple operations on one item" and each entry
//! carries exactly one action: both are a `ValidationException`, as are an item a `Put`
//! carries past the documented size, a VALUE an action carries that no item could hold —
//! both are facts of the REQUEST, settled before any table is looked up — and a key that is
//! not the table's. An `Update` that would leave the item past that size is answered by WHOSE
//! failure the crossing is. The item the statement ASKS FOR — the key it addresses plus what
//! its own clauses write, which no stored item can make smaller
//! ([`update_asks_for_an_oversized_item`]) — is a fact of the REQUEST, so a statement that
//! cannot fit in an item at all refuses the CALL before the transaction is staged, in the
//! class and the sentence the single-item `UpdateItem` of the same expression is refused with.
//! Only where the statement itself fits can the crossing be the transaction's own doing, and
//! then it is answered by [`crossed_the_item_quota`]: an item the change LEAVES that is
//! past the QUOTA in the bytes it STORES really became too large — "an item size becomes too
//! large (larger than 400 KB) [...] or a similar validation error occurs because of changes
//! made by the transaction", one of the circumstances `TransactionCanceledException`
//! documents — so it CANCELS the transaction, with that action's reason carrying the sentence
//! the update path words that quota with, like every other failure an action is found to have
//! by applying it against the item its key holds. An item that FITS the quota and crosses only
//! the CHARGED ceiling — what the request's own accounting adds on top of the stored bytes: the
//! untouched part of the item charged at its floor, the frame each clause costs — asks for more
//! than the
//! update path admits, which is a fact of the request, so the CALL is refused in the class and
//! the sentence the single-item `UpdateItem` of the same expression is refused with. The
//! documented ceiling of 100 action requests is the model's own
//! `@length` on `TransactItems`, refused before this handler runs. An unknown — or not yet
//! `ACTIVE` — table is `ResourceNotFoundException`, and
//! a table whose resource-based policy denies the caller the action it performs
//! (`dynamodb:PutItem`, `dynamodb:UpdateItem`, `dynamodb:DeleteItem` or
//! `dynamodb:ConditionCheckItem`) is `AccessDeniedException`.
//!
//! `ClientRequestToken` is honoured: "providing a ClientRequestToken makes the call to
//! TransactWriteItems idempotent, meaning that multiple identical calls have the same
//! effect as one single call", so a repeat of a token inside the documented 10-minute
//! window applies NOTHING and answers what the first call answered, and a repeat carrying
//! different actions is `IdempotentParameterMismatchException`. What a token stands for is
//! [`crate::state::CompletedTransaction`], remembered per REGION — a table name resolves
//! inside one region, so a token names the transaction it ran there and nowhere else.
//!
//! A transactional write costs twice what the same single-item write costs
//! ([`crate::state::TRANSACTIONAL_CAPACITY_FACTOR`]); `ReturnConsumedCapacity` reports it
//! per table and `ReturnItemCollectionMetrics=SIZE` reports the item collections the
//! transaction wrote into, both totalled from stored state by
//! [`crate::state::WriteLedger`].
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `TransactWriteItems::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `TransactWriteItems`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! TransactWriteItems is a synchronous write operation that groups up to 100 action
//! requests. These actions can target items in different tables, but not in different
//! Amazon Web Services accounts or Regions, and no two actions can target the same item.
//! For example, you cannot both ConditionCheck and Update the same item. The aggregate size
//! of the items in the transaction cannot exceed 4 MB. The actions are completed atomically
//! so that either all of them succeed, or all of them fail. They are defined by the
//! following objects: Put — Initiates a PutItem operation to write a new item. This
//! structure specifies the primary key of the item to be written, the name of the table to
//! write it in, an optional condition expression that must be satisfied for the write to
//! succeed, a list of the item's attributes, and a field indicating whether to retrieve the
//! item's attributes if the condition is not met. Update — Initiates an UpdateItem
//! operation to update an existing item. This structure specifies the primary key of the
//! item to be updated, the name of the table where it resides, an optional condition
//! expression that must be satisfied for the update to succeed, an expression that defines
//! one or more attributes to be updated, and a field indicating whether to retrieve the
//! item's attributes if the condition is not met. Delete — Initiates a DeleteItem operation
//! to delete an existing item. This structure specifies the primary key of the item to be
//! deleted, the name of the table where it resides, an optional condition expression that
//! must be satisfied for the deletion to succeed, and a field indicating whether to
//! retrieve the item's attributes if the condition is not met. ConditionCheck — Applies a
//! condition to an item that is not being modified by the transaction. This structure
//! specifies the primary key of the item to be checked, the name of the table where it
//! resides, a condition expression that must be satisfied for the transaction to succeed,
//! and a field indicating whether to retrieve the item's attributes if the condition is not
//! met. DynamoDB rejects the entire TransactWriteItems request if any of the following is
//! true: A condition in one of the condition expressions is not met. An ongoing operation
//! is in the process of updating the same item. There is insufficient provisioned capacity
//! for the transaction to be completed. An item size becomes too large (bigger than 400
//! KB), a local secondary index (LSI) becomes too large, or a similar validation error
//! occurs because of changes made by the transaction. The aggregate size of the items in
//! the transaction exceeds 4 MB. There is a user error, such as an invalid data format.
//!
//! ## Input
//!
//! * `ClientRequestToken` (ClientRequestToken, len 1..36, @idempotencyToken) — Providing a
//!   ClientRequestToken makes the call to TransactWriteItems idempotent, meaning that
//!   multiple identical calls have the same effect as one single call. Although multiple
//!   identical calls using the same client request token produce the same result on the
//!   server (no side effects), the responses to the calls might not be the same. If the
//!   ReturnConsumedCapacity parameter is set, then the initial TransactWriteItems call
//!   returns the amount of write capacity units consumed in making the changes. Subsequent
//!   TransactWriteItems calls with the same client token return the number of read capacity
//!   units consumed in reading the item. A client request token is valid for 10 minutes after
//!   the first …
//! * `ReturnConsumedCapacity` (ReturnConsumedCapacity, one of: INDEXES | NONE | TOTAL) —
//!   Determines the level of detail about either provisioned or on-demand throughput
//!   consumption that is returned in the response: INDEXES - The response includes the
//!   aggregate ConsumedCapacity for the operation, together with ConsumedCapacity for each
//!   table and secondary index that was accessed. Note that some operations, such as GetItem
//!   and BatchGetItem, do not access any indexes at all. In these cases, specifying INDEXES
//!   will only return ConsumedCapacity information for table(s). TOTAL - The response
//!   includes only the aggregate ConsumedCapacity for the operation. NONE - No
//!   ConsumedCapacity details are included in the response.
//! * `ReturnItemCollectionMetrics` (ReturnItemCollectionMetrics, one of: NONE | SIZE) —
//!   Determines whether item collection metrics are returned. If set to SIZE, the response
//!   includes statistics about item collections (if any), that were modified during the
//!   operation and are returned in the response. If set to NONE (the default), no statistics
//!   are returned.
//! * `TransactItems` (TransactWriteItemList, len 1..100) **required** — An ordered array of
//!   up to 100 TransactWriteItem objects, each of which contains a ConditionCheck, Put,
//!   Update, or Delete object. These can operate on items in different tables, but the tables
//!   must reside in the same Amazon Web Services account and Region, and no two of them can
//!   operate on the same item.
//!
//! ## Output
//!
//! * `ConsumedCapacity` (ConsumedCapacityMultiple) — The capacity units consumed by the
//!   entire TransactWriteItems operation. The values of the list are ordered according to the
//!   ordering of the TransactItems request parameter. If the table has vector indexes, each
//!   element also includes a VectorIndexes field with VectorWriteRequestBytes consumed for
//!   each affected vector index.
//! * `ItemCollectionMetrics` (ItemCollectionMetricsPerTable) — A list of tables that were
//!   processed by TransactWriteItems and, for each table, information about any item
//!   collections that were affected by individual UpdateItem, PutItem, or DeleteItem
//!   operations.
//!
//! ## Errors
//!
//! * `IdempotentParameterMismatchException` (HTTP 400) — DynamoDB rejected the request
//!   because you retried a request with a different payload but with an idempotent token that
//!   was already used.
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidEndpointException` (HTTP 421)
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
// `TransactWriteItemsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TransactWriteItemsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#TransactWriteItems")]
#[derive(Debug)]
pub enum TransactWriteItemsErrorModeled {
    IdempotentParameterMismatchException(IdempotentParameterMismatchException),
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    TransactionCanceledException(TransactionCanceledException),
    TransactionInProgressException(TransactionInProgressException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::transact_write_items::TransactWriteItemsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TransactWriteItemsError = crate::scaffolded::Scaffolded<TransactWriteItemsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TransactWriteItemsErrorModeled> for TransactWriteItemsError {
    fn from(modeled: TransactWriteItemsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TransactWriteItems` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TransactWriteItems<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TransactWriteItems<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TransactWriteItems<Eng>
{
    type Input = TransactWriteItemsInput;
    type Output = TransactWriteItemsOutput;
    type Error = TransactWriteItemsError;

    const NAME: &'static str = "TransactWriteItems";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TRANSACT_WRITE_ITEMS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TRANSACT_WRITE_ITEMS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;

        let report_item_collection = crate::state::returns_item_collection_metrics(
            input.return_item_collection_metrics.as_deref(),
        )
        .map_err(lift)?;

        // Every entry states exactly one action. The canonical form built from them is
        // the REQUEST's own content — nothing of what the store holds — which is what
        // makes it comparable across the retries a `ClientRequestToken` covers.
        let mut actions: Vec<Requested<'_>> = Vec::with_capacity(input.transact_items.len());
        for entry in &input.transact_items {
            actions.push(Requested::of(entry).map_err(lift)?);
        }
        // The fingerprint covers the WHOLE request a token stands for, so the members
        // that choose what the response reports are encoded beside the actions
        // ([`crate::state::canonical_reporting_options`]): a retry under the same token
        // asking for different capacity or item-collection detail is a different request.
        let canonical = crate::state::action_fingerprint(&format!(
            "{}{}",
            crate::state::canonical_reporting_options(&[
                input.return_consumed_capacity.as_deref(),
                input.return_item_collection_metrics.as_deref(),
            ]),
            canonical_actions(&actions),
        ));
        let token = input
            .client_request_token
            .as_deref()
            .filter(|token| !token.is_empty());

        // Every fact of the REQUEST is settled here, over EVERY element, before any
        // table is looked up: the name each element addresses its table by, the
        // substitution tables its expressions dereference, the values those expressions
        // carry, the value its `ReturnValuesOnConditionCheckFailure` states and the
        // grammar of the update it applies. None of it depends on the store, so a
        // transaction that cannot be served however the store looks is refused as the
        // malformed request it is — rather than as whatever the FIRST element's table
        // turns out to be — and an element's defect is reported whichever earlier
        // element names a table that is not there.
        let names: Vec<String> = actions
            .iter()
            .enumerate()
            .map(|(at, action)| check_element_shape(at, action, &region))
            .collect::<Result<_, _>>()?;

        // "The aggregate size of the items in the transaction cannot exceed 4 MB"
        // ([`crate::state::MAX_TRANSACTION_SIZE_BYTES`]): a transaction is applied as one
        // unit, so the ceiling is on the whole call and not on any one action — a hundred
        // actions each inside the item limit can still ask for more than one transaction
        // may carry. What each action contributes is the data it carries: the item or
        // `Key` map it addresses, plus the values its expressions substitute, which is
        // where an `Update` action's payload sits. It is a fact of the REQUEST, so it is
        // settled with every other one above, before any table is looked up, and it
        // refuses the call rather than cancelling it: nothing of the transaction was
        // applicable to begin with.
        let aggregate: i64 = actions
            .iter()
            .map(|action| {
                crate::state::item_size(action.addressed())
                    + action
                        .expression_attribute_values()
                        .map_or(0, crate::state::item_size)
            })
            .sum();
        if aggregate > crate::state::MAX_TRANSACTION_SIZE_BYTES {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::transaction_size_exceeded_message(aggregate),
            ));
        }

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);

        // A token still inside its window stands for the transaction it was first used
        // for IN THIS REGION — a table name resolves inside one region, so a token can
        // only stand for what it ran against there: identical actions are answered from
        // what that call DID, and different actions are the documented mismatch. Both
        // decisions precede every condition below, because a retry must not be judged
        // against the state its own first attempt left.
        if let Some(token) = token {
            if let Some(completed) = store.recorded_transaction(&region, token).cloned() {
                if completed.actions != canonical {
                    return Err(
                        TransactWriteItemsErrorModeled::IdempotentParameterMismatchException(
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
                return Ok(TransactWriteItemsOutput {
                    // A repeat under the token APPLIES NOTHING, so it is billed as the
                    // read it was: "subsequent TransactWriteItems calls with the same
                    // client token return the number of read capacity units consumed in
                    // reading the item" (`TransactWriteItems$ClientRequestToken`) — the
                    // figures the first call recorded, reported in the read arm.
                    consumed_capacity: completed.ledger.consumed_capacity(
                        input.return_consumed_capacity.as_deref(),
                        crate::state::CapacityKind::Read,
                        crate::state::KindReport::Named,
                    ),
                    item_collection_metrics: report_item_collection
                        .then(|| completed.ledger.item_collection_metrics())
                        .flatten(),
                });
            }
        }

        // Pass one BINDS every action to its table, writing NOTHING and reading no stored
        // item: the table it names, the policies in force, the values its map carries and
        // the key that map states against the table's own key schema
        // ([`bind_element`]).
        //
        // An action the transaction cannot bind does not stop the pass: "all of which
        // must either succeed or fail", and the reasons report what EACH action's own
        // outcome was in request order, so every action is bound and the failures are
        // collected BESIDE the actions that bound — one entry per action, in request
        // order, which is what keeps every reason aligned with the action it is about.
        // Only a defect of the REQUEST refuses the call outright ([`ElementRefusal`]).
        let mut bound: Vec<Result<Bound, String>> = Vec::with_capacity(actions.len());
        for (at, action) in actions.iter().enumerate() {
            match bind_element(
                &*self.engine,
                &store,
                &caller,
                &region,
                now,
                action,
                &names[at],
            ) {
                Ok(element) => bound.push(Ok(element)),
                Err(ElementRefusal::Call(error)) => return Err(error),
                Err(ElementRefusal::Element(message)) => bound.push(Err(message)),
            }
        }

        // "Transaction requests cannot include multiple operations on one item": a rule
        // about the whole transaction, so it is answered only once every action has been
        // bound to the table it names ([`repeats_an_item`]) — a transaction naming a table
        // that is not there is refused for the missing table, whichever action names it.
        if repeats_an_item(&bound) {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::DUPLICATE_TRANSACT_ITEM_MESSAGE,
            ));
        }

        // A defect an action is found to have while it is BOUND is found before any
        // stored item has been read, so the transaction is cancelled there: that action
        // reports its [`crate::state::CANCELLATION_CODE_VALIDATION_ERROR`] with the
        // sentence the same defect earns on the single-item path, and every OTHER action
        // reports the null code with no message — "if no error occurred for the
        // associated item an error with a Null code and Null message will be present"
        // (`CancellationReason`) — because nothing was ever read for it: no condition of
        // its was evaluated and no update of its was applied. Nothing was attempted, so
        // nothing is metered either.
        if bound.iter().any(Result::is_err) {
            let codes: Vec<&str> = bound
                .iter()
                .map(|element| match element {
                    Ok(_) => crate::state::CANCELLATION_CODE_NONE,
                    Err(_) => crate::state::CANCELLATION_CODE_VALIDATION_ERROR,
                })
                .collect();
            let cancellation_reasons: Vec<CancellationReason> = bound
                .iter()
                .zip(&codes)
                .map(|(element, code)| CancellationReason {
                    code: Some((*code).to_string()),
                    message: element.as_ref().err().cloned(),
                    item: None,
                })
                .collect();
            return Err(TransactWriteItemsErrorModeled::TransactionCanceledException(
                TransactionCanceledException {
                    message: Some(crate::state::transaction_cancelled_message(&codes)),
                    cancellation_reasons: Some(cancellation_reasons),
                },
            )
            .into());
        }

        // Pass two reads every action against the item its key holds: its condition
        // first, and the update it states only where that condition held
        // ([`resolve_element`]). A transaction only lands once all of it is known to be
        // applicable, and every condition speaks about the item as it stood before the
        // transaction.
        let mut outcomes: Vec<Outcome> = Vec::with_capacity(actions.len());
        for (action, element) in actions.iter().zip(bound) {
            // One outcome per action, whichever pass decided it: the cancellation above
            // already answered a transaction carrying an action that did not bind, and an
            // action that did not bind still reports its own reason here, so `outcomes`
            // cannot be shorter than `TransactItems` and no reason can slide onto another
            // action.
            let element = match element {
                Ok(element) => element,
                Err(message) => {
                    outcomes.push(Outcome::Failed {
                        code: crate::state::CANCELLATION_CODE_VALIDATION_ERROR,
                        message,
                    });
                    continue;
                }
            };
            match resolve_element(&store, &region, now, action, element) {
                Ok(planned) => outcomes.push(Outcome::Applicable(Box::new(planned))),
                Err(ElementRefusal::Call(error)) => return Err(error),
                Err(ElementRefusal::Element(message)) => outcomes.push(Outcome::Failed {
                    code: crate::state::CANCELLATION_CODE_VALIDATION_ERROR,
                    message,
                }),
            }
        }

        // "All of which must either succeed or fail": one action the transaction could
        // not apply cancels the whole request — a condition that was not met, or a
        // failure the action earned when it was read against the stored item — and the
        // reasons report what each action's own outcome was, in request order.
        if outcomes.iter().any(Outcome::cancels) {
            // Each element whose condition was not met is METERED, at twice what the same
            // single-item write would cost: a transactional conditional write that fails
            // consumes capacity all the same
            // ([`crate::state::charge_refused_transactional_write`]). Nothing is applied and
            // the cancellation reports no capacity in band.
            let refused: Vec<(String, crate::state::AttributeValueKey, Option<crate::state::Item>)> =
                outcomes
                    .iter()
                    .filter_map(Outcome::unsatisfied)
                    .map(|action| {
                        (
                            action.table.clone(),
                            action.key.partition.clone(),
                            action.before.clone(),
                        )
                    })
                    .collect();
            for (table, partition, before) in &refused {
                crate::state::charge_refused_transactional_write(
                    &mut store,
                    &region,
                    table,
                    partition,
                    before.as_ref(),
                    now,
                );
            }
            let codes: Vec<&str> = outcomes.iter().map(Outcome::cancellation_code).collect();
            let cancellation_reasons: Vec<CancellationReason> = outcomes
                .iter()
                .map(Outcome::cancellation_reason)
                .collect();
            return Err(TransactWriteItemsErrorModeled::TransactionCanceledException(
                TransactionCanceledException {
                    message: Some(crate::state::transaction_cancelled_message(&codes)),
                    cancellation_reasons: Some(cancellation_reasons),
                },
            )
            .into());
        }
        // Nothing of the transaction failed, so every action is one it applies.
        let planned: Vec<Planned> = outcomes
            .into_iter()
            .filter_map(Outcome::into_planned)
            .collect();

        // Pass two applies them. Each figure is read back from the table the write
        // landed in, so the capacity fan-out and the item-collection size are computed
        // from stored state rather than from the request.
        let mut ledger = crate::state::WriteLedger::default();
        let mut undo: Vec<(String, crate::state::ItemKey, Option<crate::state::Item>)> = Vec::new();
        let mut refused: Option<Self::Error> = None;
        // Where the change feeds of the tables this transaction writes stood before it
        // started: a transaction that is rolled back below made no modification, so its
        // queued changes go back with the items.
        let change_mark = store.queued_stream_record_count();
        for action in &planned {
            let after = match &action.write {
                PlannedWrite::Put(item) | PlannedWrite::Update(item) => Some(item.clone()),
                PlannedWrite::Check | PlannedWrite::Delete | PlannedWrite::Nothing => None,
            };
            if matches!(action.write, PlannedWrite::Check | PlannedWrite::Nothing) {
                // A `ConditionCheck`, and an `Update` that adds no item, write nothing,
                // so neither lands in an item collection; both are still part of the
                // transactional write and both read one item, so the table is charged for
                // that item and no index is.
                ledger.record(
                    &action.table,
                    &crate::state::WriteCapacity {
                        table: crate::state::write_capacity_units(
                            action.before.as_ref().map_or(0, crate::state::item_size),
                        ),
                        ..crate::state::WriteCapacity::default()
                    },
                );
                continue;
            }
            undo.push((
                action.table.clone(),
                action.key.clone(),
                action.before.clone(),
            ));
            let landed = match &after {
                Some(item) => store
                    .put_item(&region, &action.table, now, action.key.clone(), item.clone())
                    .map(|_| ()),
                None => store
                    .delete_item(&region, &action.table, now, &action.key)
                    .map(|_| ()),
            };
            if let Err(error) = landed {
                refused = Some(lift(error));
                break;
            }
            // Reading the table back is on the same all-or-nothing path as the write
            // itself: a failure here must roll back too, never return early past the
            // undo below.
            match store.active_table(&region, &action.table, now) {
                Ok(table) => {
                    ledger.record(
                        &action.table,
                        &table.write_capacity(action.before.as_ref(), after.as_ref()),
                    );
                    if report_item_collection {
                        ledger.record_metrics(
                            &action.table,
                            table.item_collection_metrics(&action.addressed),
                        );
                    }
                }
                Err(error) => {
                    refused = Some(lift(error));
                    break;
                }
            }
        }
        if let Some(refused) = refused {
            // A transaction is all-or-nothing, so a refusal from the store's own
            // budgets part-way through puts back exactly what stood at every key it had
            // already reached — each of which fitted the store a moment ago.
            for (table, key, before) in undo.into_iter().rev() {
                let restored = match before {
                    Some(item) => store.put_item(&region, &table, now, key, item).map(|_| ()),
                    None => store.delete_item(&region, &table, now, &key).map(|_| ()),
                };
                if let Err(error) = restored {
                    // Nothing a caller can be told makes this right, so it is LOGGED:
                    // a store that could not be put back is observable to whoever runs
                    // the emulator rather than silent.
                    ::tracing::error!(
                        table = %table,
                        error = %error.message(),
                        "a cancelled transaction could not be rolled back"
                    );
                }
            }
            // The restoring writes above queued changes of their own, so the queue is
            // put back where it stood: the feeds of these tables carry neither the
            // cancelled modifications nor their undo.
            store.rewind_stream_records(change_mark);
            return Err(refused);
        }

        // "Transactional write requests require two write capacity units": the whole
        // transaction is billed at twice what the same single-item writes would cost.
        ledger.scale_table_units(crate::state::TRANSACTIONAL_CAPACITY_FACTOR);
        // What the transaction consumed is also what this service publishes as those tables'
        // throughput: the scaled ledger is the figure the response reports, so counting it
        // here is what puts the transactional cost — twice the single-item one — in the
        // series a client graphs and alarms on ([`crate::state::charge_reported_ledger`]).
        crate::state::charge_reported_ledger(
            &mut store,
            &region,
            &ledger,
            crate::state::Consumed::Write,
            now,
        );
        if let Some(token) = token {
            store.record_transaction(
                &region,
                token,
                crate::state::CompletedTransaction {
                    actions: canonical,
                    completed_at: now,
                    ledger: ledger.clone(),
                },
            );
        }
        // Every change the transaction made reaches its table's stream before the
        // response does, in the order the transaction applied them.
        crate::state::publish_item_changes(&*self.engine, &mut store, now);

        Ok(TransactWriteItemsOutput {
            // The transaction was APPLIED, so what it consumed is write capacity: "the
            // initial TransactWriteItems call returns the amount of write capacity units
            // consumed in making the changes" (`TransactWriteItems$ClientRequestToken`).
            consumed_capacity: ledger.consumed_capacity(
                input.return_consumed_capacity.as_deref(),
                crate::state::CapacityKind::Write,
                crate::state::KindReport::Named,
            ),
            item_collection_metrics: report_item_collection
                .then(|| ledger.item_collection_metrics())
                .flatten(),
        })
    }
}

/// One action a transaction states: exactly one of the four a `TransactWriteItem` may
/// carry ("a list of requests that can perform update, put, delete, or check operations
/// on multiple items in one or more tables atomically").
enum Requested<'a> {
    Check(&'a ConditionCheck),
    Put(&'a Put),
    Delete(&'a Delete),
    Update(&'a Update),
}

impl<'a> Requested<'a> {
    /// The one action `entry` states, or the refusal for an entry that states none of
    /// them — or more than one.
    ///
    /// The two defects are two violations: an entry that states NO action violates the
    /// constraint every element of this list is held to
    /// ([`crate::state::transact_item_states_no_action`]), which is a clause the request's
    /// own constraint refusal counts beside every violation the model's traits find
    /// (`crate::service` collects it off the request), while an entry naming SEVERAL
    /// states no single action.
    fn of(entry: &'a TransactWriteItem) -> Result<Self, crate::state::RequestError> {
        let stated = [
            entry.condition_check.as_ref().map(Self::Check),
            entry.put.as_ref().map(Self::Put),
            entry.delete.as_ref().map(Self::Delete),
            entry.update.as_ref().map(Self::Update),
        ];
        let mut stated = stated.into_iter().flatten();
        match (stated.next(), stated.next()) {
            (Some(one), None) => Ok(one),
            (None, _) => Err(crate::state::transact_item_states_no_action()),
            _ => Err(crate::state::exactly_one_operation(
                "transactItems",
                "ConditionCheck, Put, Delete or Update",
            )),
        }
    }

    /// The table this action addresses.
    fn table_name(&self) -> &'a str {
        match self {
            Self::Check(check) => &check.table_name,
            Self::Put(put) => &put.table_name,
            Self::Delete(delete) => &delete.table_name,
            Self::Update(update) => &update.table_name,
        }
    }

    /// The condition this action states, which a `ConditionCheck` always does.
    fn condition_expression(&self) -> Option<&'a str> {
        match self {
            Self::Check(check) => Some(check.condition_expression.as_str()),
            Self::Put(put) => put.condition_expression.as_deref(),
            Self::Delete(delete) => delete.condition_expression.as_deref(),
            Self::Update(update) => update.condition_expression.as_deref(),
        }
    }

    /// The update this action applies, which only an `Update` states.
    fn update_expression(&self) -> Option<&'a str> {
        match self {
            Self::Update(update) => Some(update.update_expression.as_str()),
            Self::Check(_) | Self::Put(_) | Self::Delete(_) => None,
        }
    }

    /// The attribute-name substitutions its expressions dereference.
    fn expression_attribute_names(
        &self,
    ) -> Option<&'a std::collections::HashMap<String, String>> {
        match self {
            Self::Check(check) => check.expression_attribute_names.as_ref(),
            Self::Put(put) => put.expression_attribute_names.as_ref(),
            Self::Delete(delete) => delete.expression_attribute_names.as_ref(),
            Self::Update(update) => update.expression_attribute_names.as_ref(),
        }
    }

    /// The attribute-value substitutions its expressions dereference.
    fn expression_attribute_values(&self) -> Option<&'a crate::state::Item> {
        match self {
            Self::Check(check) => check.expression_attribute_values.as_ref(),
            Self::Put(put) => put.expression_attribute_values.as_ref(),
            Self::Delete(delete) => delete.expression_attribute_values.as_ref(),
            Self::Update(update) => update.expression_attribute_values.as_ref(),
        }
    }

    /// Whether this action reports its item when its condition is not met.
    fn return_values_on_condition_check_failure(&self) -> Option<&'a str> {
        match self {
            Self::Check(check) => check.return_values_on_condition_check_failure.as_deref(),
            Self::Put(put) => put.return_values_on_condition_check_failure.as_deref(),
            Self::Delete(delete) => delete.return_values_on_condition_check_failure.as_deref(),
            Self::Update(update) => update.return_values_on_condition_check_failure.as_deref(),
        }
    }

    /// The `Item` or `Key` map the action carries — what it addresses, and what the
    /// canonical form below encodes.
    fn addressed(&self) -> &'a crate::state::Item {
        match self {
            Self::Check(check) => &check.key,
            Self::Put(put) => &put.item,
            Self::Delete(delete) => &delete.key,
            Self::Update(update) => &update.key,
        }
    }

    /// The IAM action this one performs, which is the action a table's resource-based
    /// policy is read for: a transaction performs the item actions of the operations it
    /// is made of, and `dynamodb:ConditionCheckItem` for a `ConditionCheck`.
    fn policy_action(&self) -> &'static str {
        match self {
            Self::Check(_) => "dynamodb:ConditionCheckItem",
            Self::Put(_) => "dynamodb:PutItem",
            Self::Delete(_) => "dynamodb:DeleteItem",
            Self::Update(_) => "dynamodb:UpdateItem",
        }
    }

    /// This action's name, as the canonical form tags it.
    fn kind(&self) -> &'static str {
        match self {
            Self::Check(_) => "ConditionCheck",
            Self::Put(_) => "Put",
            Self::Delete(_) => "Delete",
            Self::Update(_) => "Update",
        }
    }

    /// The member of `TransactWriteItem` this action arrived in, as the request
    /// validator spells a member in the paths it reports (lowerCamel).
    fn member(&self) -> &'static str {
        match self {
            Self::Check(_) => "conditionCheck",
            Self::Put(_) => "put",
            Self::Delete(_) => "delete",
            Self::Update(_) => "update",
        }
    }
}

/// The member list this operation's elements arrive in, as the request validator spells
/// it in the paths it reports.
const TRANSACT_ITEMS_MEMBER: &str = "transactItems";

/// Settle every fact of ONE element the request itself decides, and answer the table
/// name it addresses.
///
/// Called over every element BEFORE any table is looked up, so which defect a
/// malformed transaction is refused with does not depend on what the store holds (see
/// the call site).
///
/// A clause about a member of an element names that member's FULL path — the list, the
/// element's 1-based position in it, the collection step the walk takes into one
/// element, the action the element states and the leaf member — so a caller can find
/// WHICH element of a transaction of up to 100 was refused.
fn check_element_shape(
    at: usize,
    action: &Requested<'_>,
    region: &str,
) -> Result<String, TransactWriteItemsError> {
    let name = crate::state::resolve_table_name_in_member_clause(
        &format!(
            "{TRANSACT_ITEMS_MEMBER}.{}.member.{}.{}",
            at + 1,
            action.member(),
            crate::state::TABLE_NAME_MEMBER_PATH
        ),
        action.table_name(),
        region,
    )
    .map_err(lift)?;
    let substitutions = crate::state::Substitutions::new(
        action.expression_attribute_names(),
        action.expression_attribute_values(),
    );
    // Every token this element DEFINES must be one of its own expressions uses: an
    // unused entry of either substitution table makes the whole request invalid. It is
    // the ELEMENT's tables that are checked, so the refusal is stated on its own
    // ([`crate::state::Substitutions::check_all_referenced_in_element`]).
    substitutions
        .check_all_referenced_in_element(&[
            ("ConditionExpression", action.condition_expression()),
            ("UpdateExpression", action.update_expression()),
        ])
        .map_err(lift)?;
    // A value a condition compares against, or one an update writes, is held to the
    // same limits a stored value is: the documented nesting depth and this emulator's
    // ceiling on a collection's members. The values are this ELEMENT's members, so what
    // they are refused for is stated on its own, like every other refusal about one
    // element's own members ([`crate::state::unenveloped`]).
    //
    // BINDING a value stores nothing, so the documented item ceiling is not one of those
    // limits here ([`crate::state::check_substituted_value`]): a value only compared, or
    // one an action subtracts rather than stores, never becomes item bytes. The ceiling is
    // stated over the values an `Update` would STORE, where its expression is parsed below,
    // and over the item each action leaves behind. What bounds the element's substitution
    // table as a whole is its own total cap.
    for (placeholder, value) in action.expression_attribute_values().into_iter().flatten() {
        crate::state::check_substituted_value(placeholder, value)
            .map_err(crate::state::unenveloped)
            .map_err(lift)?;
    }
    // The cap on the MEMBER's total, which in a transaction is this element's own; what
    // bounds the whole CALL is the documented 4 MB aggregate over every element's data, the
    // values its expressions substitute among them
    // ([`crate::state::MAX_TRANSACTION_SIZE_BYTES`], asked of the request once each element
    // has been read).
    crate::state::check_substitution_total(action.expression_attribute_values())
        .map_err(crate::state::unenveloped)
        .map_err(lift)?;
    // "For ReturnValuesOnConditionCheckFailure, the valid values are: NONE and ALL_OLD."
    if !matches!(
        action.return_values_on_condition_check_failure(),
        None | Some("NONE" | "ALL_OLD")
    ) {
        return Err(crate::scaffolded::Scaffolded::validation(format!(
            "ReturnValuesOnConditionCheckFailure can only be ALL_OLD or NONE, got {}",
            action
                .return_values_on_condition_check_failure()
                .unwrap_or_default()
        )));
    }
    // The grammar of the condition any of the four actions may state is a fact of the
    // request: whether it READS as a condition depends on its own text and this element's
    // substitution tables alone, so it is settled in this pass with every other fact of the
    // request — which is what makes the element a transaction's refusal names the first
    // LISTED one that carries a defect, rather than whichever element the preparation below
    // reached first. Evaluating it still waits for the stored item, which is where
    // [`crate::state::write_condition_satisfied`] reads it.
    //
    // Read BEFORE the update an `Update` states, for the reason the single-item write reads
    // them in that order: an element states its gate and its change through two expression
    // members, and the defect reported when both carry one is the CONDITION's.
    if let Some(condition) = action.condition_expression() {
        crate::state::ParsedCondition::parse("ConditionExpression", condition, substitutions)
            .map_err(crate::state::refused_in_element)
            .map_err(lift)?;
    }
    // The grammar of the update an `Update` applies is a fact of the request too, so a
    // malformed one is refused here rather than when the element is prepared. It is the
    // ELEMENT's own expression, so what it is refused for is stated outside the request
    // envelope, like the refusals its substitution tables earn above
    // ([`crate::state::refused_in_element`]).
    if let Requested::Update(update) = action {
        crate::state::ParsedUpdate::parse(
            "UpdateExpression",
            &update.update_expression,
            substitutions,
        )
        .map_err(crate::state::refused_in_element)
        .map_err(lift)?;
    }
    // The documented ceiling on how deeply a stored value may nest is a QUOTA, and the
    // fixed sentence AWS states for a quota an ELEMENT of a transaction crossed is stated
    // as the sentence it is — like the item-size sentence beside it below, and like every
    // other refusal about one element's own members
    // ([`crate::state::refused_in_element`]) — rather than inside the
    // `<N> validation error(s) detected: ` envelope that counts violations of the
    // REQUEST's own members. Asked before the value rules below, which state the same
    // ceiling inside that envelope for the single-item writes.
    for value in action.addressed().values() {
        if crate::state::attribute_depth_exceeds(value, crate::state::MAX_ATTRIBUTE_DEPTH) {
            return Err(lift(crate::state::unenveloped(
                crate::state::nesting_levels_exceeded(),
            )));
        }
    }
    // Every value the map this element ADDRESSES carries — the item a `Put` writes, the
    // `Key` every other action names — is held to the documented value rules (the nesting
    // depth, this emulator's ceiling on a collection's members, the numeric domain), and
    // the item a `Put` writes to the documented item size. Both are facts of the request,
    // so the element is held to them before any table is looked up; the item an `Update`
    // LEAVES is read against the stored one, so it is held to the same rules where it is
    // computed.
    // The clause is stated as the sentence it is, out of the
    // `<N> validation error(s) detected: ` envelope: that envelope counts violations of the
    // REQUEST's own members, and the item or the `Key` of one `TransactItems` entry is a
    // member of that entry — which is the rule this service states for every refusal about
    // an element's own members ([`crate::state::refused_in_element`], and
    // [`crate::state::unenveloped`] for a clause about a value).
    for (attribute, value) in action.addressed() {
        crate::state::check_attribute_value(attribute, value)
            .map_err(crate::state::unenveloped)
            .map_err(lift)?;
    }
    if let Requested::Put(put) = action {
        let mut item = put.item.clone();
        crate::state::canonicalize_item(&mut item)
            .map_err(crate::state::unenveloped)
            .map_err(lift)?;
        if crate::state::item_size(&item) > crate::state::MAX_ITEM_SIZE_BYTES {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
            ));
        }
    }
    Ok(name)
}

/// `AccessDeniedException` when the policies in force do not let `caller` perform the
/// action `requested` performs on `table`, so authorization is decided before any item is
/// read or keyed — against the partition the action addresses and, for a `Put`, the
/// attributes the item it stores names. An `Update` names the attributes its expression
/// writes as well, which is not enumerated here, so it answers for that key with nothing
/// ([`crate::state::authorization::RequestKeys`]). The action is performed INSIDE this
/// operation, which is what a statement admitting it only as part of a transaction tests
/// ([`crate::state::authorization::EnclosingOperation`]).
fn deny(
    caller: &crate::state::authorization::Caller,
    table: &crate::state::Table,
    requested: &Requested<'_>,
    now: ::std::time::SystemTime,
) -> Result<(), TransactWriteItemsError> {
    let addressed =
        crate::state::authorization::AddressedKeys::of_maps(table, [requested.addressed()]);
    let keys = match requested {
        Requested::Put(put) => {
            crate::state::authorization::RequestKeys::storing(addressed, &put.item)
        }
        _ => crate::state::authorization::RequestKeys::addressing(addressed),
    }
    .within(crate::state::authorization::EnclosingOperation::TransactWriteItems);
    match crate::state::authorization::denial_keys(
        caller,
        table,
        requested.policy_action(),
        now,
        &keys,
    ) {
        Some(denial) => Err(crate::scaffolded::Scaffolded::access_denied(denial)),
        None => Ok(()),
    }
}

/// The request's actions, canonically encoded: what a repeated `ClientRequestToken` is
/// compared against (through [`crate::state::action_fingerprint`], which is what the
/// store keeps), because "if you submit a request with the same client token but a
/// change in other parameters within the 10-minute idempotency window, DynamoDB returns
/// an IdempotentParameterMismatch exception".
///
/// Everything that decides what the transaction DOES is encoded, in request order:
/// the action, its table, the item or key it addresses, its expressions and their
/// substitutions. Maps and sets go through [`crate::state::canonical_item_text`], so two
/// identical calls encode identically whatever order their JSON arrived in.
fn canonical_actions(actions: &[Requested<'_>]) -> String {
    let mut text = String::new();
    for action in actions {
        text.push_str(action.kind());
        text.push('|');
        // The RESOLVED table name and the CANONICAL form of every value: a retry that
        // addresses the same table by ARN instead of by name, or spells the same number
        // differently, is the same transaction, so it fingerprints the same way the
        // handler itself reads those members.
        text.push_str(
            &crate::state::resolve_table_name(action.table_name())
                .unwrap_or_else(|_| action.table_name().to_string()),
        );
        text.push('|');
        text.push_str(&crate::state::canonical_item_text(&canonicalized(
            action.addressed(),
        )));
        text.push('|');
        text.push_str(action.condition_expression().unwrap_or_default());
        text.push('|');
        text.push_str(action.update_expression().unwrap_or_default());
        text.push('|');
        for (placeholder, name) in action
            .expression_attribute_names()
            .into_iter()
            .flatten()
            .collect::<std::collections::BTreeMap<_, _>>()
        {
            text.push_str(placeholder);
            text.push('=');
            text.push_str(name);
            text.push(';');
        }
        text.push('|');
        if let Some(values) = action.expression_attribute_values() {
            text.push_str(&crate::state::canonical_item_text(&canonicalized(values)));
        }
        text.push('\n');
    }
    text
}

/// `item` with its numbers in the one form the store keeps them in, for the fingerprint
/// above. A value no number can be read out of is left as it came: the request is
/// refused later, by the handler, with the refusal that names what was wrong with it.
fn canonicalized(item: &crate::state::Item) -> crate::state::Item {
    let mut canonical = item.clone();
    if crate::state::canonicalize_item(&mut canonical).is_err() {
        return item.clone();
    }
    canonical
}

/// One action BOUND to its table: the key it addresses and what it would write, settled
/// without reading the item that key holds ([`bind_element`]).
struct Bound {
    /// The resolved name of the table it addresses.
    table: String,
    /// Whether it reports its item when its condition is not met.
    returns_old_on_failure: bool,
    prepared: Prepared,
}

/// One action resolved against its table: the key it addresses and the write it would
/// land, computed before any of the transaction is applied.
struct Prepared {
    key: crate::state::ItemKey,
    addressed: crate::state::Item,
    write: BoundWrite,
}

/// What one action writes, as far as the REQUEST and the table's schema settle it: an
/// `Update`'s own result is left to the pass that reads the item its key holds
/// ([`resolve_element`]), because only that item can say what the update leaves.
enum BoundWrite {
    /// A `ConditionCheck`, which writes nothing.
    Check,
    /// The item a `Put` stores, replacing whatever its key held.
    Put(crate::state::Item),
    /// A `Delete`, which removes whatever its key held.
    Delete,
    /// An `Update`, parsed once here and applied to the stored item — which is what
    /// decides its result — in the pass that reads it.
    Update(crate::state::ParsedUpdate),
}

/// One action of the transaction, ready to apply.
struct Planned {
    /// The resolved name of the table it writes to.
    table: String,
    key: crate::state::ItemKey,
    /// The item or `Key` map it addressed, which the item-collection metrics of the
    /// write are reported for.
    addressed: crate::state::Item,
    write: PlannedWrite,
    /// The item that stood at `key` before the transaction: what the condition was read
    /// against, what the capacity weighs, and what an unmet condition reports.
    before: Option<crate::state::Item>,
    satisfied: bool,
    returns_old_on_failure: bool,
}

impl Planned {
    /// The `CancellationReason$Code` this action reports when the transaction is
    /// cancelled: the condition it failed, or `None` for an action nothing was wrong
    /// with.
    fn cancellation_code(&self) -> &'static str {
        if self.satisfied {
            crate::state::CANCELLATION_CODE_NONE
        } else {
            crate::state::CANCELLATION_CODE_CONDITION_FAILED
        }
    }
}

/// What one action writes once every condition has held.
enum PlannedWrite {
    /// A `ConditionCheck`, which writes nothing.
    Check,
    /// The item a `Put` stores, replacing whatever its key held.
    Put(crate::state::Item),
    /// A `Delete`, which removes whatever its key held.
    Delete,
    /// The item an `Update` leaves at its key.
    Update(crate::state::Item),
    /// An `Update` that wrote nothing into a key that held no item, which adds none.
    Nothing,
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> TransactWriteItemsError {
    TransactWriteItemsErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> TransactWriteItemsError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            TransactWriteItemsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}

/// This service's store, as [`prepare_element`] reads it.
type Store = crate::state::ComAmazonawsDynamodbDynamodb20120810State;

/// How the preparation of ONE action failed, and therefore WHO the failure belongs to.
///
/// "All of which must either succeed or fail": a transaction answers a failure of one of
/// its actions as the cancellation of the whole request, naming that action in the reason
/// list — but only a failure that is the ACTION's. A defect of the REQUEST is not any
/// action's outcome, so it refuses the call and cancels nothing.
enum ElementRefusal {
    /// A defect of the REQUEST: this error is the whole call's answer.
    ///
    /// A key attribute the element supplied no value for, a value that is no value of its
    /// own type, an empty key value, an item addressed twice, a table that is not there —
    /// decided about what the request carried rather than about the stored item
    /// ([`crate::state::KeyRefusal`] is which of the two a bad key is).
    Call(TransactWriteItemsError),
    /// A failure of THIS action, which cancels the transaction: the action reports
    /// [`crate::state::CANCELLATION_CODE_VALIDATION_ERROR`] with this sentence and every
    /// other action still reports its own outcome.
    ///
    /// It is the failures the action can only be found to have by reading it against the
    /// stored item — applying its `UpdateExpression`, and holding the item that update
    /// would leave to the documented value and item-size rules — and those its item
    /// carries against the TABLE's own schema.
    Element(String),
}

/// A refusal of the REQUEST, answered as the whole call's.
fn refused_call(error: crate::state::RequestError) -> ElementRefusal {
    ElementRefusal::Call(lift(error))
}

/// A refusal of ONE ACTION, which cancels the transaction.
///
/// The sentence is carried VERBATIM — the one the same defect earns on the single-item
/// path — because a reason is not a second wording
/// ([`crate::state::cancelled_by_element_reasons`] states the same rule for the surfaces
/// that report one failing element on its own). A lookup failure is no action's doing and
/// stays the call's.
fn refused_element(error: crate::state::RequestError) -> ElementRefusal {
    match error {
        crate::state::RequestError::Validation(message) => ElementRefusal::Element(message),
        other => refused_call(other),
    }
}

/// How an `Update` element states a refusal about the item its change would LEAVE.
///
/// Every rule about that item can only be found by reading the change against the stored item
/// (an operand of the wrong type, a document path the stored item cannot hold, an attribute it
/// does not carry, and the item-SIZE clamp the update language applies while it BUILDS the
/// item, which trips exactly when the item the change would leave is past the quota in its own
/// bytes), so it stays this ACTION's failure and cancels the transaction
/// ([`refused_element`]). A VALUE the request CARRIES that no item could hold is not one of
/// them: it is a fact of the request, refused before any table is looked up
/// ([`check_element_shape`]). Whether a crossing of the CHARGED ceiling is this action's or the
/// request's is [`crossed_the_item_quota`].
fn refused_update_outcome(error: crate::state::RequestError) -> ElementRefusal {
    refused_element(error)
}

/// How an action answers a bad key: the cancellation its table's key schema decided, or
/// the request-level refusal its own shape earned ([`crate::state::KeyRefusal`] owns
/// which is which).
fn refused_key(refusal: crate::state::KeyRefusal) -> ElementRefusal {
    if refusal.cancels() {
        return refused_element(refusal.into_error());
    }
    refused_call(refusal.into_error())
}

/// WHOSE failure a crossing of the item-size quota is, for the item an `Update` LEAVES.
///
/// Reached only where the statement itself fits in an item
/// ([`update_asks_for_an_oversized_item`] refuses the call before this is asked), so what is
/// weighed here is the item the change leaves behind over the bytes the STORED item already
/// held.
///
/// What an update is weighed against is not the quota alone: on top of the bytes the item
/// stores it adds what the REQUEST's own shape costs — the part of the item the change does
/// not address charged at a floor, so an item short of it buys no headroom, and the frame each
/// clause of the expression costs
/// ([`crate::state::update_charged_size`]). A crossing of that charged ceiling is therefore
/// one of two different things, and the bytes the item itself STORES are what tell them
/// apart:
///
/// * The item the change leaves is past the quota in its own bytes
///   ([`crate::state::Table::item_size_exceeded`] — the quota's own count, the entry a local
///   secondary index holds for the item included). "An item size becomes too large (larger
///   than 400 KB), or a local secondary index (LSI) becomes too large, or a similar
///   validation error occurs because of changes made by the transaction" is a circumstance
///   `TransactionCanceledException` documents, and this is it: the change really did make the
///   item too large, so the transaction is CANCELLED and the reason list names THIS action,
///   every other action reporting its own outcome beside it.
/// * The item FITS the quota and what crossed the ceiling is only what the request's own
///   accounting charges on top of it. Then nothing became too large; the request asks for
///   more than the update path admits, which is a fact of the request — so the CALL is
///   refused in the class and the sentence the single-item `UpdateItem` of this very
///   expression is refused with, and the caller is told to shrink the write rather than to
///   retry the transaction.
///
/// The sentence is the same either way, because the quota is the same quota; what this
/// decides is only who answers for it.
///
/// Reads the table and the item it is handed and writes nothing.
fn crossed_the_item_quota(
    table: &crate::state::Table,
    left: &crate::state::Item,
) -> ElementRefusal {
    if table.item_size_exceeded(left) {
        return ElementRefusal::Element(
            crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE.to_string(),
        );
    }
    ElementRefusal::Call(crate::scaffolded::Scaffolded::validation(
        crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE,
    ))
}

/// Whether the item an `Update` ASKS FOR is already past the item-size quota — which is a
/// fact of the REQUEST, since no item the addressed key holds can make the statement smaller.
///
/// What the statement asks for is the `Key` map it addresses plus what its own clauses write,
/// which is exactly the item the update builds for a key that holds nothing — the same
/// starting point [`resolve_element`] uses for an absent item — charged the way every update
/// surface charges the item it leaves ([`crate::state::update_size_exceeded`]), the key
/// portion's floor and the frame each clause costs included. A statement whose own payload
/// cannot fit is unsatisfiable however the store looks, so the CALL is refused in the class
/// and the sentence the single-item `UpdateItem` of the same expression is refused with, and
/// nothing of the transaction is staged or graded.
///
/// Only what the REQUEST states is read: the clamp the update language applies while it
/// BUILDS the item says the payload does not fit on its own, and any OTHER failure of this
/// reading — an operand whose type the absent item cannot satisfy, a document path no item
/// holds, an attribute nothing carries — is not a fact of the request at all, so it is left
/// to the pass that reads the stored item and decides this action's own outcome.
///
/// Reads the table and the request it is handed and writes nothing.
fn update_asks_for_an_oversized_item(
    table: &crate::state::Table,
    key: &crate::state::Item,
    applied: &crate::state::ParsedUpdate,
) -> bool {
    let mut asked = key.clone();
    // Applied as the item the statement would ADD at a key that holds none
    // ([`crate::state::UpdateTarget::NewItem`]): there is no stored image for a document
    // path to be resolved against, which is exactly the reading this question wants — only
    // what the request itself states.
    match applied.apply(&mut asked, crate::state::UpdateTarget::NewItem) {
        Ok(_) => {
            // Canonicalized before it is sized, exactly as the single-item path sizes the
            // item it stores; a form this reading cannot canonicalize is a defect of a value
            // the request carried, which the request's own pass answers for.
            if crate::state::canonicalize_item(&mut asked).is_err() {
                return false;
            }
            // Sized over NO pre-image, for the same reason the update was applied over none:
            // an arithmetic clause whose operands the stored item would supply states no
            // operand this reading can charge
            // ([`crate::state::ParsedUpdate::request_shape`]), which is the reading this
            // question wants. The pass that reads the stored item charges it there.
            crate::state::update_size_exceeded(table, &asked, &applied.request_shape(None))
        }
        Err(crate::state::RequestError::Validation(message)) => {
            message == crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE
        }
        Err(_) => false,
    }
}


/// What ONE action of the transaction turned out to be, in request order: the write it
/// would land, or the failure it reported.
///
/// Both are needed before anything is written, because the answer to a cancelled
/// transaction is the whole ordered reason list: "the cancellation reason codes [...] are
/// ordered according to the ordering of the TransactItems request parameter".
enum Outcome {
    /// An action that can be applied, once every other action can be too. Boxed because an
    /// action that FAILED carries only its reason, and a transaction of a hundred actions
    /// holds one of these per action.
    Applicable(Box<Planned>),
    /// An action the transaction could not apply, with the reason code and sentence it
    /// reports.
    Failed {
        code: &'static str,
        message: String,
    },
}

impl Outcome {
    /// Whether THIS action is one the transaction has to be cancelled for.
    fn cancels(&self) -> bool {
        match self {
            Self::Applicable(planned) => !planned.satisfied,
            Self::Failed { .. } => true,
        }
    }

    /// The action whose CONDITION was not met, which is the one a cancelled transaction
    /// still meters ([`crate::state::charge_refused_transactional_write`]): an action that
    /// failed before its condition could be read performed no write to be charged for.
    fn unsatisfied(&self) -> Option<&Planned> {
        match self {
            Self::Applicable(planned) => (!planned.satisfied).then_some(planned),
            Self::Failed { .. } => None,
        }
    }

    /// The `CancellationReason$Code` this action reports.
    fn cancellation_code(&self) -> &'static str {
        match self {
            Self::Applicable(planned) => planned.cancellation_code(),
            Self::Failed { code, .. } => code,
        }
    }

    /// The whole `CancellationReason` this action reports: its code, the sentence a
    /// failure carries, and — for a condition that was not met — the item the action
    /// asked to see ("the item which caused the condition check to fail").
    fn cancellation_reason(&self) -> CancellationReason {
        match self {
            Self::Applicable(planned) => CancellationReason {
                code: Some(planned.cancellation_code().to_string()),
                message: (!planned.satisfied)
                    .then(|| crate::state::CONDITION_FAILED_MESSAGE.to_string()),
                item: (!planned.satisfied && planned.returns_old_on_failure)
                    .then(|| planned.before.clone())
                    .flatten(),
            },
            Self::Failed { code, message } => CancellationReason {
                code: Some((*code).to_string()),
                message: Some(message.clone()),
                item: None,
            },
        }
    }

    /// The write this action lands, for the pass that applies a transaction every action
    /// of which is applicable.
    fn into_planned(self) -> Option<Planned> {
        match self {
            Self::Applicable(planned) => Some(*planned),
            Self::Failed { .. } => None,
        }
    }
}

/// Whether two of the BOUND actions came to one item of one table — "transaction requests
/// cannot include multiple operations on one item".
///
/// Asked of the whole transaction once every action has been bound, which is what puts the
/// rule BEHIND every refusal an action earns while it is bound: a table this region does not
/// hold refuses the call wherever in the list it is named, so a transaction that repeats an
/// item AND names a missing table is answered for the table, exactly as the single-item call
/// on that table would be.
///
/// Only the actions that BOUND address an item at all: one the transaction already found a
/// defect in never reached a key of its table's own schema, so it is no part of this rule.
fn repeats_an_item(bound: &[Result<Bound, String>]) -> bool {
    let mut keyed: std::collections::BTreeSet<(&str, &crate::state::ItemKey)> =
        std::collections::BTreeSet::new();
    bound
        .iter()
        .flatten()
        .any(|element| !keyed.insert((element.table.as_str(), &element.prepared.key)))
}

/// BIND ONE action to its table, writing NOTHING and reading no stored item.
///
/// `name` is the resolved table name the element addresses, settled with the rest of the
/// request ([`check_element_shape`]). The rule that no two actions may address one item is
/// about the TRANSACTION rather than about any one action, so it is NOT asked here: the
/// caller asks it once every action has been bound to the table it names ([`repeats_an_item`]).
///
/// Everything an action can be found to be wrong about WITHOUT the item its key holds is
/// decided here: the table it names, the policies in force, the values the map it carries
/// holds, and the key that map states against the table's own KEY SCHEMA. What the item its
/// key holds decides — its condition, and the update it applies — is read afterwards, over
/// the actions that bound ([`resolve_element`]).
///
/// Which answer each refusal earns is [`ElementRefusal`]: a defect of the REQUEST refuses
/// the call, and a failure the action is found to have against the table's own schema
/// cancels the transaction with this action named in the reason list.
fn bind_element<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    store: &Store,
    caller: &crate::state::authorization::Caller,
    region: &str,
    now: ::std::time::SystemTime,
    action: &Requested<'_>,
    name: &str,
) -> Result<Bound, ElementRefusal> {
    let name = name.to_string();
    let substitutions = crate::state::Substitutions::new(
        action.expression_attribute_names(),
        action.expression_attribute_values(),
    );
    let returns_old_on_failure =
        action.return_values_on_condition_check_failure() == Some("ALL_OLD");

    // The items are stored ENCRYPTED under this table's KMS key, so a key KMS
    // will not use leaves nothing this element could read or encrypt — and "the
    // entire transaction is rejected" when any part of it cannot be served, so
    // NOTHING is written ([`crate::state::check_table_key_accessible`]).
    crate::state::check_table_key_accessible(
        engine,
        store
            .active_table(region, &name, now)
            .map_err(refused_call)?,
    )
    .map_err(refused_call)?;

    let prepared = match action {
        Requested::Put(put) => {
            // The item's own values and its size were settled with the rest of
            // the request ([`check_element_shape`]); canonicalizing is idempotent,
            // so what is stored below is the item in the one form the store keeps.
            let mut item = put.item.clone();
            crate::state::canonicalize_item(&mut item)
                .map_err(crate::state::unenveloped)
                .map_err(refused_call)?;
            let table = store
                .active_table(region, &name, now)
                .map_err(refused_call)?;
            deny(caller, table, action, now).map_err(ElementRefusal::Call)?;
            // The half of the item-size quota that needs the table: the entry a local
            // secondary index of it would hold for this item is charged beside the item
            // ([`crate::state::Table::item_size_exceeded`]). The bytes the element
            // CARRIED were bounded with the rest of the request, before any table was
            // looked up, and both refuse the call in the same sentence: a `Put` states the
            // whole item, so an item that cannot be stored is the request's own defect.
            if table.item_size_exceeded(&item) {
                return Err(ElementRefusal::Call(
                    crate::scaffolded::Scaffolded::validation(
                        crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
                    ),
                ));
            }
            table
                .check_index_key_types(&item)
                .map_err(refused_element)?;
            table
                .check_index_key_values(&item)
                .map_err(refused_call)?;
            table
                .check_vector_attributes(&item)
                .map_err(refused_call)?;
            Prepared {
                key: table.key_of_transact_item(&item).map_err(refused_key)?,
                addressed: item.clone(),
                write: BoundWrite::Put(item),
            }
        }
        Requested::Delete(delete) => {
            let table = store
                .active_table(region, &name, now)
                .map_err(refused_call)?;
            deny(caller, table, action, now).map_err(ElementRefusal::Call)?;
            Prepared {
                key: table
                    .key_of_transact_key_map(&delete.key)
                    .map_err(refused_key)?,
                addressed: delete.key.clone(),
                write: BoundWrite::Delete,
            }
        }
        Requested::Check(check) => {
            let table = store
                .active_table(region, &name, now)
                .map_err(refused_call)?;
            deny(caller, table, action, now).map_err(ElementRefusal::Call)?;
            Prepared {
                key: table
                    .key_of_transact_key_map(&check.key)
                    .map_err(refused_key)?,
                addressed: check.key.clone(),
                write: BoundWrite::Check,
            }
        }
        Requested::Update(update) => {
            // Every refusal this element's own expressions earn is stated
            // outside the request envelope, like the refusals its substitution
            // tables earn above
            // ([`crate::state::refused_in_element`]). The grammar is the REQUEST's, so it
            // is read here and carried to the pass that applies it.
            let applied = crate::state::ParsedUpdate::parse(
                "UpdateExpression",
                &update.update_expression,
                substitutions,
            )
            .map_err(crate::state::refused_in_element)
            .map_err(refused_call)?;
            let table = store
                .active_table(region, &name, now)
                .map_err(refused_call)?;
            deny(caller, table, action, now).map_err(ElementRefusal::Call)?;
            // The item the statement ASKS FOR is a fact of the request — the key it
            // addresses plus what its own clauses write — so a statement that cannot fit in
            // an item at all refuses the CALL here, before any stored item is read and
            // before any action is graded ([`update_asks_for_an_oversized_item`]).
            if update_asks_for_an_oversized_item(table, &update.key, &applied) {
                return Err(ElementRefusal::Call(
                    crate::scaffolded::Scaffolded::validation(
                        crate::state::ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE,
                    ),
                ));
            }
            Prepared {
                key: table
                    .key_of_transact_key_map(&update.key)
                    .map_err(refused_key)?,
                // The item an `Update` LEAVES can only be computed from the one its key
                // holds, so what it writes — and what the write is reported for — is
                // settled in the pass that reads that item ([`resolve_element`]). Until
                // then the action addresses the `Key` map it named, which is also where
                // an update of a key that holds no item starts from.
                addressed: update.key.clone(),
                write: BoundWrite::Update(applied),
            }
        }
    };

    Ok(Bound {
        table: name,
        returns_old_on_failure,
        prepared,
    })
}

/// Read ONE bound action against the item its key holds: its CONDITION first, and the
/// change it states only where that condition held.
///
/// That order is the service's own: an action's `ConditionExpression` "must be satisfied in
/// order for the conditional operation to succeed" (`Update$ConditionExpression`), so an
/// action whose condition the stored item does not satisfy has already failed and reports
/// `ConditionalCheckFailed` alone — the runtime evaluation of its `UpdateExpression`, which
/// only the stored item can fail, is never reached, exactly as it is never reached by a
/// single-item `UpdateItem` whose condition is not met. Only where the condition holds is
/// the update applied, and only then can it earn a failure of its own.
///
/// Which answer each refusal earns is [`ElementRefusal`], as it is while the action is
/// bound: a defect of the REQUEST refuses the call, and a failure the action is found to
/// have against the stored item cancels the transaction with this action named in the
/// reason list.
fn resolve_element(
    store: &Store,
    region: &str,
    now: ::std::time::SystemTime,
    action: &Requested<'_>,
    bound: Bound,
) -> Result<Planned, ElementRefusal> {
    let Bound {
        table: name,
        returns_old_on_failure,
        prepared,
    } = bound;
    let substitutions = crate::state::Substitutions::new(
        action.expression_attribute_names(),
        action.expression_attribute_values(),
    );
    let before = store
        .active_table(region, &name, now)
        .map_err(refused_call)?
        .items
        .get(&prepared.key)
        .cloned();
    // The element's `ConditionExpression` is this element's member too, so what it
    // is refused for is stated outside the request envelope
    // ([`crate::state::refused_in_element`]).
    let satisfied = crate::state::write_condition_satisfied(
        action.condition_expression(),
        None,
        None,
        substitutions,
        before.as_ref(),
    )
    .map_err(crate::state::refused_in_element)
    .map_err(refused_call)?;
    let (addressed, write) = match prepared.write {
        BoundWrite::Check => (prepared.addressed, PlannedWrite::Check),
        BoundWrite::Put(item) => (prepared.addressed, PlannedWrite::Put(item)),
        BoundWrite::Delete => (prepared.addressed, PlannedWrite::Delete),
        // An action whose condition was not met writes nothing: the transaction it is part
        // of is cancelled for it, and its update is never applied — so the stored item
        // decides nothing else about this action.
        BoundWrite::Update(_) if !satisfied => (prepared.addressed, PlannedWrite::Nothing),
        // The ONE thing the stored item decides besides the condition: what the update
        // leaves at its key. It is read only where the condition held.
        BoundWrite::Update(applied) => {
            let table = store
                .active_table(region, &name, now)
                .map_err(refused_call)?;
            let partition_key = table.partition_key_name().to_string();
            let sort_key = table.sort_key_name().map(ToString::to_string);
            // The `Key` map this action addresses: where an update of a key that holds no
            // item starts from.
            let addressed_key = prepared.addressed;
            // An update of a key that holds no item starts from the key itself,
            // so the created item carries the primary key it was addressed by — which is
            // the map the action was bound as addressing.
            let mut item = before.clone().unwrap_or_else(|| addressed_key.clone());
            // APPLYING the update is where the stored item decides: an operand the stored
            // value has the wrong type for, a document path the stored item cannot hold,
            // an attribute it does not carry. None of it can be settled from the request
            // alone, so it is this ACTION that failed and the transaction is cancelled with
            // that sentence against this action ([`ElementRefusal::Element`]) — the documented
            // item-SIZE quota among them, because the clamp the update language applies while
            // it BUILDS the item trips exactly when the item the change would leave is past
            // the quota in its own bytes ([`refused_update_outcome`]).
            let touched = applied
                .apply(&mut item, crate::state::UpdateTarget::of(before.as_ref()))
                .map_err(crate::state::refused_in_element)
                .map_err(refused_update_outcome)?;
            for attribute in touched.attributes() {
                if attribute.as_str() == partition_key.as_str()
                    || Some(attribute.as_str()) == sort_key.as_deref()
                {
                    // What the update may change is decided against the TABLE's
                    // own key schema, so it is this element the transaction
                    // refuses and the answer is that element's cancellation,
                    // carrying the sentence the single-item `UpdateItem` states
                    // for the same defect.
                    return Err(ElementRefusal::Element(format!(
                        "{}: Cannot update attribute {attribute}. This attribute is \
                         part of the key",
                        crate::state::INVALID_PARAMETER_CLAUSE
                    )));
                }
            }
            // The item this update LEAVES is read against the stored one, so every rule
            // about it is this action's outcome too ([`refused_update_outcome`]): a value the
            // update COMPUTES from what the stored item held cannot be settled from the
            // request alone, and an item carrying one no item may hold is one the
            // transaction's own change made too large.
            for (attribute, value) in &item {
                crate::state::check_attribute_value(attribute, value)
                    .map_err(crate::state::stated_for_an_update)
                    .map_err(crate::state::unenveloped)
                    .map_err(refused_update_outcome)?;
            }
            crate::state::canonicalize_item(&mut item)
                .map_err(crate::state::unenveloped)
                .map_err(refused_element)?;
            // Sized over what this element ASKED FOR, exactly as the single-item
            // `UpdateItem` of the same expression is: the bytes of the attributes the change
            // addresses plus what the part of the item it leaves alone is charged, plus the
            // entry a local secondary index of the table holds for it
            // ([`crate::state::update_size_exceeded`]) plus the frame each clause of the
            // update costs ([`crate::state::ParsedUpdate::request_shape`]).
            //
            // WHO answers for a crossing of that CHARGED ceiling follows whether the item
            // this change LEAVES is past the quota in its OWN bytes
            // ([`crossed_the_item_quota`]): an item that really became too large is the
            // documented cancel circumstance and is THIS ACTION's failure, while an item that
            // fits the quota and crosses only what the request's own accounting charges on
            // top of it refuses the CALL, as the single-item `UpdateItem` of this very
            // expression is refused. The item a `Put` CARRIES is the request's own and is
            // refused before any of the transaction is applied, worded for an item that was
            // carried.
            if crate::state::update_size_exceeded(
                table,
                &item,
                &applied.request_shape(before.as_ref()),
            ) {
                return Err(crossed_the_item_quota(table, &item));
            }
            table
                .check_index_key_types(&item)
                .map_err(refused_element)?;
            table
                .check_index_key_values_of_update(&item)
                .map_err(refused_call)?;
            table
                .check_vector_attributes(&item)
                .map_err(refused_call)?;
            // An update that wrote nothing into a key that holds no item adds no
            // item, exactly as the single-item `UpdateItem` of the same
            // expression does: what the key holds afterwards is what one of the
            // update's clauses gave it a value for, and the key attributes it
            // started from are not that.
            let adds_nothing = !applied.states_no_action()
                && before.is_none()
                && item.keys().all(|attribute| {
                    attribute.as_str() == partition_key.as_str()
                        || Some(attribute.as_str()) == sort_key.as_deref()
                });
            (
                item.clone(),
                if adds_nothing {
                    PlannedWrite::Nothing
                } else {
                    PlannedWrite::Update(item)
                },
            )
        }
    };
    Ok(Planned {
        table: name,
        key: prepared.key,
        addressed,
        write,
        before,
        satisfied,
        returns_old_on_failure,
    })
}
