// SPDX-License-Identifier: CC0-1.0

//! @op BatchWriteItem
//! IMPLEMENTED: applies every `PutRequest` and `DeleteRequest` in `RequestItems` to the
//! stored tables, so a put REPLACES whatever its key held and a delete REMOVES it — "if
//! you perform a BatchWriteItem operation on an existing item, that item's values will be
//! overwritten". Each operation lands through the same store write sites `PutItem` and
//! `DeleteItem` use ([`crate::state::ComAmazonawsDynamodbDynamodb20120810State::put_item`]
//! and `delete_item`), so numbers are canonicalized exactly as they are on the
//! single-item path, each write is charged the capacity it consumed from the same
//! per-table budget, and so is the record each change publishes on its table's stream
//! ([`crate::state::publish_item_changes`]). What this path does NOT share with the
//! single-item one is the admission decision for a write the budget is short of: a
//! single-item write has nowhere to report a request it did not apply, so it is refused,
//! while a batch reports it in `UnprocessedItems` — see the admission paragraph below.
//!
//! The whole request is resolved and validated BEFORE any item lands — the tables, the key
//! schema, every value's own rules, the documented item size and the index key limits — so
//! a refused batch writes nothing. An unknown, or not yet `ACTIVE`, table is
//! `ResourceNotFoundException`; a table whose resource-based policy denies the caller
//! `dynamodb:BatchWriteItem` is `AccessDeniedException`.
//!
//! "You can only request one of these operations, not both, in a single WriteRequest" (the
//! `WriteRequest` doc), and "you cannot put and delete the same item in the same
//! BatchWriteItem request" (this operation's own doc): a `WriteRequest` carrying neither or
//! both, a batch past the documented 25 operations, and a key addressed twice are each a
//! `ValidationException`. No condition is read and no item is echoed, because
//! "you cannot specify conditions on individual put and delete requests, and BatchWriteItem
//! does not return deleted items in the response".
//!
//! Each operation is then ADMITTED on its own against the write capacity its table is
//! provisioned for and against the per-partition-key ceiling beneath it
//! ([`crate::state::Table::admit_item_write`]): one the throughput component does not
//! admit is handed back in `UnprocessedItems`, in the form it was sent, and stores
//! nothing — so a hot partition key is throttled while the same rate spread over many
//! keys is served. How much capacity a table has left decides HOW MANY of a batch's
//! requests land, not WHETHER the call gets anything done: while nothing has landed, the
//! request in hand may be served from capacity BORROWED against the budget's own refill,
//! and once something has landed the internal per-partition-key ceiling may still lend
//! ([`crate::state::Table::admit_item_write_with_progress`]), so a batch of small writes
//! aimed at one partition key lands whole with an empty `UnprocessedItems` and every call
//! answers at once with a residue its caller can resend, so the documented drain loop
//! converges.
//! Borrowed units are charged in full and repay at the provisioned rate, and the
//! borrowing is bounded, so a caller that keeps writing past what its table is
//! provisioned for runs that allowance out: from then on a batch of writes the budget
//! cannot cover gets nothing done and is refused with
//! `ProvisionedThroughputExceededException`, as is one none of whose requests the
//! ceilings could ever cover. The refusal carries the
//! `ThrottlingReasons` entry for the ceiling that bound and the ARN of the table it
//! bound on. `ReturnConsumedCapacity`
//! reports one `ConsumedCapacity` per table written and `ReturnItemCollectionMetrics=SIZE`
//! one `ItemCollectionMetrics` list per table, both totalled from stored state by
//! [`crate::state::WriteLedger`].
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `BatchWriteItem::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `BatchWriteItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! The BatchWriteItem operation puts or deletes multiple items in one or more tables. A
//! single call to BatchWriteItem can transmit up to 16MB of data over the network,
//! consisting of up to 25 item put or delete operations. While individual items can be up
//! to 400 KB once stored, it's important to note that an item's representation might be
//! greater than 400KB while being sent in DynamoDB's JSON format for the API call. For more
//! details on this distinction, see Naming Rules and Data Types. BatchWriteItem cannot
//! update items. If you perform a BatchWriteItem operation on an existing item, that item's
//! values will be overwritten by the operation and it will appear like it was updated. To
//! update items, we recommend you use the UpdateItem action. The individual PutItem and
//! DeleteItem operations specified in BatchWriteItem are atomic; however BatchWriteItem as
//! a whole is not. If any requested operations fail because the table's provisioned
//! throughput is exceeded or an internal processing failure occurs, the failed operations
//! are returned in the UnprocessedItems response parameter. You can investigate and
//! optionally resend the requests. Typically, you would call BatchWriteItem in a loop. Each
//! iteration would check for unprocessed items and submit a new BatchWriteItem request with
//! those unprocessed items until all items have been processed. If BatchWriteItem cannot
//! process any items due to throttling (for example, insufficient provisioned throughput on
//! the tables in the request, or partition-level or account-level limits), it returns a
//! ProvisionedThroughputExceededException or a ThrottlingException. Both indicate that the
//! request was throttled; check the ThrottlingReason field in the returned exception for
//! details. If DynamoDB returns any unprocessed items, you should retry the batch operation
//! on those items. However, we strongly recommend that you use an exponential backoff
//! algorithm. If you retry the batch operation immediately, the underlying read or write
//! requests can still fail due to throttling on the individual tables. If you delay the
//! batch operation using exponential backoff, the individual requests in the batch are much
//! more likely to succeed. For more information, see Batch Operations and Error Handling in
//! the Amazon DynamoDB Developer Guide. With BatchWriteItem, you can efficiently write or
//! delete large amounts of data, such as from Amazon EMR, or copy data from another
//! database into DynamoDB. In order to improve performance with these large-scale
//! operations, BatchWriteItem does not behave in the same way as individual PutItem and
//! DeleteItem calls would. For example, you cannot specify conditions on individual put and
//! delete requests, and BatchWriteItem does not return deleted items in the response. If
//! you use a programming language that supports concurrency, you can use threads to write
//! items in parallel. Your application must include the necessary logic to manage the
//! threads. With languages that don't support threading, you must update or delete …
//!
//! ## Input
//!
//! * `RequestItems` (BatchWriteItemRequestMap, len 1..25) **required** — A map of one or more
//!   table names or table ARNs and, for each table, a list of operations to be performed
//!   (DeleteRequest or PutRequest). Each element in the map consists of the following:
//!   DeleteRequest - Perform a DeleteItem operation on the specified item. The item to be
//!   deleted is identified by a Key subelement: Key - A map of primary key attribute values
//!   that uniquely identify the item. Each entry in this map consists of an attribute name
//!   and an attribute value. For each primary key, you must provide all of the key
//!   attributes. For example, with a simple primary key, you only need to provide a value for
//!   the partition key. For a composite primary key, you must provide values for both the …
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
//!   includes statistics about item collections, if any, that were modified during the
//!   operation are returned in the response. If set to NONE (the default), no statistics are
//!   returned.
//!
//! ## Output
//!
//! * `ConsumedCapacity` (ConsumedCapacityMultiple) — The capacity units consumed by the
//!   entire BatchWriteItem operation. Each element consists of: TableName - The table that
//!   consumed the provisioned throughput. CapacityUnits - The total number of capacity units
//!   consumed. If the table has vector indexes, each element also includes a VectorIndexes
//!   field with VectorWriteRequestBytes consumed for each affected vector index.
//! * `ItemCollectionMetrics` (ItemCollectionMetricsPerTable) — A list of tables that were
//!   processed by BatchWriteItem and, for each table, information about any item collections
//!   that were affected by individual DeleteItem or PutItem operations. Each entry consists
//!   of the following subelements: ItemCollectionKey - The partition key value of the item
//!   collection. This is the same as the partition key value of the item. SizeEstimateRangeGB
//!   - An estimate of item collection size, expressed in GB. This is a two-element array
//!   containing a lower bound and an upper bound for the estimate. The estimate includes the
//!   size of all the items in the table, plus the size of all attributes projected into all
//!   of the local secondary indexes on the table. Use this estimate …
//! * `UnprocessedItems` (BatchWriteItemRequestMap, len 1..25) — A map of tables and requests
//!   against those tables that were not processed. The UnprocessedItems value is in the same
//!   form as RequestItems, so you can provide this value directly to a subsequent
//!   BatchWriteItem operation. For more information, see RequestItems in the Request
//!   Parameters section. Each UnprocessedItems entry consists of a table name or table ARN
//!   and, for that table, a list of operations to perform (DeleteRequest or PutRequest).
//!   DeleteRequest - Perform a DeleteItem operation on the specified item. The item to be
//!   deleted is identified by a Key subelement: Key - A map of primary key attribute values
//!   that uniquely identify the item. Each entry in this map consists of an attribute …
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidEndpointException` (HTTP 421)
//! * `ItemCollectionSizeLimitExceededException` (HTTP 400) — An item collection is too large.
//!   This exception is only returned for tables that have one or more local secondary
//!   indexes.
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The request was denied due to
//!   request throttling. For detailed information about why the request was throttled and the
//!   ARN of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   The Amazon Web Services SDKs for DynamoDB automatically retry requests that receive this
//!   exception. Your request is eventually successful, unless your retry queue is too large
//!   to finish. Reduce the frequency of requests and use exponential backoff. For more
//!   information, go to Error Retries and Exponential Backoff in the Amazon DynamoDB
//!   Developer Guide.
//! * `ReplicatedWriteConflictException` (HTTP 400) — The request was rejected because one or
//!   more items in the request are being modified by a request in another Region.
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

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchWriteItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchWriteItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#BatchWriteItem")]
#[derive(Debug)]
pub enum BatchWriteItemErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ItemCollectionSizeLimitExceededException(ItemCollectionSizeLimitExceededException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    ReplicatedWriteConflictException(ReplicatedWriteConflictException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_write_item::BatchWriteItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchWriteItemError = crate::scaffolded::Scaffolded<BatchWriteItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchWriteItemErrorModeled> for BatchWriteItemError {
    fn from(modeled: BatchWriteItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchWriteItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchWriteItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchWriteItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchWriteItem<Eng>
{
    type Input = BatchWriteItemInput;
    type Output = BatchWriteItemOutput;
    type Error = BatchWriteItemError;

    const NAME: &'static str = "BatchWriteItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_WRITE_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_WRITE_ITEM_OUTPUT_SCHEMA;
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

        // "up to 25 item put or delete operations", and "a map of ONE OR MORE table
        // names": the `@length` the model declares is on the write-request LIST one table
        // was given — a bound on the map's VALUES — while the ceiling of 25 is on the
        // OPERATIONS counted across every table the request grouped them under. So a batch
        // that spreads 26 writes over two tables is refused for the CALL's quota, one that
        // gives a single table 26 (or gives any table none) is refused for the value
        // bound, and one that names no table at all is refused for the map's own minimum
        // ([`crate::state::batch_write_requests_outside_bound`]).
        let requested: usize = input.request_items.values().map(Vec::len).sum();
        if let Some(refusal) =
            crate::state::batch_write_requests_outside_bound(&input.request_items, requested)
        {
            return Err(lift(refusal));
        }

        // The tables are walked in name order, so which refusal a bad batch earns does
        // not depend on the request map's iteration order.
        let mut addressed: Vec<(&String, &Vec<WriteRequest>)> =
            input.request_items.iter().collect();
        addressed.sort_by(|left, right| left.0.cmp(right.0));

        // Every value the items of this batch carry is held to the documented value
        // rules (the nesting depth, this emulator's collection ceilings, the numeric
        // domain) and every item a put stores to the documented 400 KB ceiling, over the
        // WHOLE request and before any table is looked up: none of those rules depends on
        // a table, so a batch that cannot be served however the store looks is refused as
        // the malformed request it is rather than reported as whichever table one of its
        // entries named and this account has none of.
        //
        // The documented NESTING ceiling is asked ahead of the other rules for each put,
        // and its clause is stated on its own: the item of one `RequestItems` entry is a
        // member of that entry rather than of the request the validator was handed, which
        // is how every refusal about an element of a multi-item call is stated
        // ([`crate::state::unenveloped`]).
        //
        // It is asked of the KEY a delete addresses as well as of the item a put stores:
        // both are `AttributeValue` maps a caller chose the nesting of, and every rule below
        // walks the tree they carry — the value checks, the canonicalisation, the key
        // resolution and the item-collection measure — so the depth they may be walked to is
        // bounded before any of those walks starts, whichever request carried the map.
        for (_, requests) in &addressed {
            for request in requests.iter() {
                let carried = request
                    .put_request
                    .as_ref()
                    .map(|put| &put.item)
                    .into_iter()
                    .chain(request.delete_request.as_ref().map(|delete| &delete.key));
                for map in carried {
                    for value in map.values() {
                        if crate::state::attribute_depth_exceeds(
                            value,
                            crate::state::MAX_ATTRIBUTE_DEPTH,
                        ) {
                            return Err(lift(crate::state::unenveloped(
                                crate::state::nesting_levels_exceeded(),
                            )));
                        }
                    }
                }
                let Some(put) = request.put_request.as_ref() else {
                    continue;
                };
                let mut item = put.item.clone();
                for (attribute, value) in &item {
                    crate::state::check_attribute_value(attribute, value).map_err(lift)?;
                }
                crate::state::canonicalize_item(&mut item).map_err(lift)?;
                if crate::state::item_size(&item) > crate::state::MAX_ITEM_SIZE_BYTES {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
                    ));
                }
            }
        }

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);

        // Pass one resolves and validates the whole request without writing anything,
        // so a batch refused for any reason leaves the store exactly as it was.
        let mut pending: Vec<(String, PendingWrite, WriteRequest)> = Vec::new();
        let mut keyed: std::collections::BTreeSet<(String, crate::state::ItemKey)> =
            std::collections::BTreeSet::new();
        // `RequestItems` is keyed by TABLE, and the key accepts a table's name or its ARN —
        // "Each table name or ARN can be used only once per BatchGetItem request"
        // (`BatchGetItem$RequestItems`) — so two keys that resolve to ONE table address one
        // table twice and the whole request is refused, exactly as the batched READ refuses
        // it ([`crate::state::DUPLICATE_BATCH_TABLE_MESSAGE`], the one sentence both
        // batched operations state for this rule). The keys are walked in order, so which
        // request earns the refusal does not depend on the request map's iteration order.
        let mut entered: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (table_name, requests) in addressed {
            // The clause a refused key earns quotes the map that key sat in, in the
            // rendering the refusals of this member state
            // ([`crate::state::rendered_batch_write_request_items`]).
            let name = crate::state::batch_table_key(table_name, &region, || {
                Some(crate::state::rendered_batch_write_request_items(
                    &input.request_items,
                ))
            })
            .map_err(lift)?;
            if !entered.insert(name.clone()) {
                return Err(crate::scaffolded::Scaffolded::validation(
                    crate::state::DUPLICATE_BATCH_TABLE_MESSAGE,
                ));
            }
            // The table and the caller's authorization for it are decided BEFORE any of
            // its requests is read: a table that is not there, or that the policies in
            // force do not let this caller write, is answered with that rather than with
            // whatever is wrong with the items it was sent. The decision is made against
            // the partitions this table's own writes address.
            let table = store.active_table(&region, &name, now).map_err(lift)?;
            // The item a put stores, or the key a delete removes, for each of this table's
            // own requests: the partitions they address, and the attributes they name.
            let maps: Vec<&crate::state::Item> = requests
                .iter()
                .filter_map(|request| {
                    request
                        .put_request
                        .as_ref()
                        .map(|put| &put.item)
                        .or_else(|| request.delete_request.as_ref().map(|delete| &delete.key))
                })
                .collect();
            let addressed =
                crate::state::authorization::AddressedKeys::of_maps(table, maps.iter().copied());
            if let Some(denial) = crate::state::authorization::denial_keys(
                &caller,
                table,
                "dynamodb:BatchWriteItem",
                now,
                &crate::state::authorization::RequestKeys::storing_maps(
                    addressed,
                    maps.iter().copied(),
                ),
            ) {
                return Err(crate::scaffolded::Scaffolded::access_denied(denial));
            }
            // The items are stored ENCRYPTED under this table's KMS key, so a key KMS
            // will not use leaves nothing this batch could encrypt its writes with: the
            // KMS fault is reported and NOTHING is written
            // ([`crate::state::check_table_key_accessible`]).
            crate::state::check_table_key_accessible(&*self.engine, table).map_err(lift)?;
            for request in requests {
                let write = match (&request.put_request, &request.delete_request) {
                    (Some(put), None) => {
                        // The item's own values and its size were settled with the rest
                        // of the request above; canonicalizing is idempotent, so what is
                        // stored below is the item in the one form the store keeps.
                        let mut item = put.item.clone();
                        crate::state::canonicalize_item(&mut item).map_err(lift)?;
                        let table = store.active_table(&region, &name, now).map_err(lift)?;
                        // The HALF of the item-size quota that needs the table: what the
                        // item is charged includes the entry a local secondary index of
                        // this table would hold for it, which the quota counts beside the
                        // item ([`crate::state::Table::item_size_exceeded`]). The bytes
                        // the request carried were bounded above, before any table was
                        // looked up.
                        if table.item_size_exceeded(&item) {
                            return Err(crate::scaffolded::Scaffolded::validation(
                                crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
                            ));
                        }
                        table.check_index_keys(&item).map_err(lift)?;
                        PendingWrite::Put {
                            key: table.key_of_multi_item(&item).map_err(lift)?,
                            item,
                        }
                    }
                    (None, Some(delete)) => {
                        for (attribute, value) in &delete.key {
                            crate::state::check_attribute_value(attribute, value).map_err(lift)?;
                        }
                        let table = store.active_table(&region, &name, now).map_err(lift)?;
                        PendingWrite::Delete {
                            key: table.key_of_key_map(&delete.key).map_err(lift)?,
                            addressed: delete.key.clone(),
                        }
                    }
                    _ => {
                        return Err(lift(crate::state::exactly_one_operation(
                            "requestItems",
                            "PutRequest or DeleteRequest",
                        )))
                    }
                };
                // "You cannot put and delete the same item in the same BatchWriteItem
                // request": one key is addressed by at most one operation.
                if !keyed.insert((name.clone(), write.key().clone())) {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        crate::state::DUPLICATE_BATCH_KEYS_MESSAGE,
                    ));
                }
                pending.push((name.clone(), write, request.clone()));
            }
        }

        // Pass two applies them. Both figures are read back from the table each write
        // landed in, so the capacity fan-out and the item-collection size are computed
        // from stored state rather than from the request.
        let mut ledger = crate::state::WriteLedger::default();
        // "If any requested operations fail because the table's provisioned throughput
        // is exceeded ... the failed operations are returned in the UnprocessedItems
        // response parameter" (the operation's doc): a write the throughput component
        // does not admit is handed back for the caller to resend, in the form it was
        // sent in, and nothing of it is stored.
        let mut unprocessed: std::collections::HashMap<String, Vec<WriteRequest>> =
            std::collections::HashMap::new();
        let mut throttled: Option<(String, crate::state::WriteThrottle)> = None;
        let mut processed = 0usize;
        // Written as a closure's result rather than with `?`, so the changes the batch
        // already made are handed to the streams service on EVERY exit: a write that
        // landed is a change the stream carries whether or not the write after it did.
        let applied = (|| -> Result<(), Self::Error> {
            for (name, write, request) in pending {
                // Each operation of the batch is admitted on its own — "the individual
                // PutItem and DeleteItem operations specified in BatchWriteItem are
                // atomic; however BatchWriteItem as a whole is not" — from the capacity
                // the write would consume against the two ceilings a write is held to.
                let admitted = {
                    let table = store
                        .active_table_mut(&region, &name, now)
                        .map_err(lift)?;
                    let before = table.items.get(write.key()).cloned();
                    let capacity = match &write {
                        PendingWrite::Put { item, .. } => {
                            table.write_capacity(before.as_ref(), Some(item))
                        }
                        PendingWrite::Delete { .. } => {
                            table.write_capacity(before.as_ref(), None)
                        }
                    };
                    // The capacity a table has left decides how many of the batch's
                    // requests land, not whether the call got anything done: a request
                    // the budgets are merely SHORT for is served from capacity borrowed
                    // against their own refill — charged in full, bounded, and repaid at
                    // the rate in force. While the batch has stored NOTHING the request
                    // in hand may borrow against BOTH ceilings, because it is the one
                    // that makes the call make progress; once something has landed the
                    // rest of the batch is held to the capacity the table is actually
                    // provisioned for, and only the internal per-partition-key ceiling
                    // may still lend — which is what makes a batch of small writes aimed
                    // at one partition key land whole and hand nothing back
                    // ([`crate::state::Table::admit_item_write_with_progress`]).
                    let borrowing = if processed == 0 {
                        crate::state::Borrowing::BothCeilings
                    } else {
                        crate::state::Borrowing::PartitionCeiling
                    };
                    table.admit_item_write_with_progress(
                        &write.key().partition,
                        &capacity,
                        now,
                        borrowing,
                    )
                };
                if let Err(throttle) = admitted {
                    throttled = Some((name.clone(), throttle));
                    unprocessed.entry(name).or_default().push(request);
                    continue;
                }
                processed += 1;
                match write {
                    PendingWrite::Put { key, item } => {
                        let replaced = store
                            .put_item(&region, &name, now, key, item.clone())
                            .map_err(lift)?;
                        let table = store.active_table(&region, &name, now).map_err(lift)?;
                        ledger
                            .record(&name, &table.write_capacity(replaced.as_ref(), Some(&item)));
                        if report_item_collection {
                            ledger.record_metrics(&name, table.item_collection_metrics(&item));
                        }
                    }
                    PendingWrite::Delete { key, addressed } => {
                        let removed =
                            store.delete_item(&region, &name, now, &key).map_err(lift)?;
                        let table = store.active_table(&region, &name, now).map_err(lift)?;
                        ledger.record(&name, &table.write_capacity(removed.as_ref(), None));
                        if report_item_collection {
                            ledger
                                .record_metrics(&name, table.item_collection_metrics(&addressed));
                        }
                    }
                }
            }
            Ok(())
        })();
        // Every change this batch made reaches its table's stream before the response
        // does, in the order the batch applied them.
        crate::state::publish_item_changes(&*self.engine, &mut store, now);
        applied?;

        // "If none of the items CAN be processed due to insufficient provisioned
        // throughput on all of the tables in the request, then BatchWriteItem returns a
        // ProvisionedThroughputExceededException": a batch that got no work done is one
        // whose first request the ceilings could not cover even by borrowing against
        // their refill — because it asks for more than they can ever hold, or because
        // the caller has already borrowed its allowance — so that is what is refused,
        // naming the ceiling that bound and the table it bound on. Everything else
        // answers 200 and hands the rest back for the caller's retry loop to drain.
        if processed == 0 {
            if let Some((name, throttle)) = throttled {
                return Err(
                    BatchWriteItemErrorModeled::ProvisionedThroughputExceededException(
                        crate::state::throughput_exceeded(
                            &crate::state::table_arn(&region, &name),
                            throttle,
                        ),
                    )
                    .into(),
                );
            }
        }

        Ok(BatchWriteItemOutput {
            // "A map of tables and requests against those tables that were not
            // processed": the operations the throughput component did not admit, in the
            // form they were sent, so the value can be handed straight to a later
            // BatchWriteItem — and an empty map when every one of them landed, which is
            // what lets a client's retry loop terminate.
            unprocessed_items: Some(unprocessed),
            item_collection_metrics: ledger.item_collection_metrics(),
            consumed_capacity: ledger.consumed_capacity(
                input.return_consumed_capacity.as_deref(),
                crate::state::CapacityKind::Write,
                crate::state::KindReport::Itemised,
            ),
        })
    }
}

/// One put or delete this batch has resolved against its table and not yet applied.
///
/// The whole request becomes a list of these before any of it lands, which is what lets
/// a batch be refused — for a key that is not the table's, an item past the documented
/// size, a key addressed twice — without having written part of itself.
enum PendingWrite {
    Put {
        key: crate::state::ItemKey,
        item: crate::state::Item,
    },
    Delete {
        key: crate::state::ItemKey,
        /// The `Key` map as the request spelled it, which is what the item-collection
        /// metrics of a delete are reported for.
        addressed: crate::state::Item,
    },
}

impl PendingWrite {
    /// The primary key this operation addresses — what the duplicate check compares.
    fn key(&self) -> &crate::state::ItemKey {
        match self {
            Self::Put { key, .. } | Self::Delete { key, .. } => key,
        }
    }
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> BatchWriteItemError {
    BatchWriteItemErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> BatchWriteItemError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            BatchWriteItemErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}
