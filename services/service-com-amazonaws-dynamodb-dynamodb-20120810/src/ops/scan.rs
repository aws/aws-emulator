// SPDX-License-Identifier: CC0-1.0

//! @op Scan
//! IMPLEMENTED: walks the stored table — "accessing every item in a table" — in the
//! order its KEY SPACE is laid out, reporting `Count`/`ScannedCount` and the
//! `LastEvaluatedKey` computed from the page it stopped on.
//!
//! The page stops at the caller's `Limit` ("the maximum number of items to
//! evaluate") and in any case at the store's 1 MB dataset budget
//! ([`crate::state::MAX_PAGE_BYTES`]), and `ExclusiveStartKey` resumes strictly after
//! the key a previous page reported. A page reports a `LastEvaluatedKey` when it
//! stopped with items still left to evaluate, which is what lets a caller tell the
//! last page from a full one — "If LastEvaluatedKey is not empty, it does not
//! necessarily mean that there is more data in the result set", so a page that ends
//! exactly on the last item is free to report none.
//!
//! What ORDER that walk has is the order the key space itself is laid out in: each
//! partition sits where the hash of its partition key puts it ([`partition_hash`]), and
//! the rows of one partition follow in the order the store holds them. A parallel scan
//! (`Segment`/`TotalSegments`) divides that same space into one contiguous block of it per
//! segment ([`Segmentation`]), so every item sharing a partition key is evaluated by one
//! segment, a segment reads a RUN of this walk, and the segments together evaluate every
//! item exactly once. A resume token belongs to the segment whose block holds it, so one
//! presented under another segment of the same scan is refused rather than resumed
//! ([`start_key_of_another_segment`]).
//!
//! `FilterExpression` is applied AFTER the page has been read — "a filter
//! expression is applied after a Scan finishes but before the results are returned"
//! — so the items it removes are still scanned and still charged for, and
//! `ScannedCount` therefore counts what the page evaluated while `Count` counts what
//! it answered. The expression is the `ConditionExpression` language, parsed by the
//! one parser this crate has for it ([`crate::state::ParsedCondition`]).
//!
//! A scan that names an `IndexName` walks that SECONDARY INDEX instead — "accessing
//! every item in a table or a secondary index" — over the index's own key space: its
//! partitions where the hash of each index partition key puts them, the entries of one of
//! them in index-key order with the table's primary key breaking the tie;
//! an item the index does not cover is not there to be scanned, and each entry carries
//! the attributes the index projects. A read that needs an attribute its index does not
//! project is served from the parent table, which only a LOCAL index can do.
//!
//! What each reported item carries is the request's `Select` and whichever projection
//! member it named — `ProjectionExpression` with its `ExpressionAttributeNames` aliases,
//! or the legacy `AttributesToGet` — applied AFTER the filter, so an attribute the
//! response does not carry can still narrow the scan. The legacy `ScanFilter` /
//! `ConditionalOperator` pair states the same filter the expression states, lowered onto
//! the one legacy comparison evaluator this crate has
//! ([`crate::state::legacy_conditions_match`]).
//!
//! `ConsistentRead` needs no handling beyond its effect on `ConsumedCapacity` — this
//! store holds one copy of each item, so a scan of it always sees every completed write
//! — except on a global secondary index, which does not support it and refuses the
//! request. What the scan is CHARGED is a read unit per 4 KB of each row it evaluated,
//! which `ConsistentRead` halves on the table's own items and never on the entries of a
//! secondary index ([`crate::state::walk_row_capacity_units`]). A `ProjectionExpression`
//! path that reaches
//! INSIDE an attribute (`a.b`, `a[1]`) is served like any other: it reports the value at
//! that path wrapped in the containers the path steps through
//! ([`crate::state::project_paths`]).
//!
//! Which index the request named, what each reported item is narrowed to and whether a
//! read has to go on to the parent table are decided by the rules `Query` reads an index
//! by too ([`crate::state::indexed_read`]).
//!
//! A table whose resource-based policy denies the caller `dynamodb:Scan` is
//! answered `AccessDeniedException` instead of its items.
//!
//! Every substitution token the request DEFINES must be one its expressions USE: an
//! `ExpressionAttributeNames`/`ExpressionAttributeValues` key no expression member
//! references makes the whole request invalid
//! ([`crate::state::Substitutions::check_all_referenced`]), the way a token supplied
//! beside no expression at all does.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `Scan::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `Scan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! The Scan operation returns one or more items and item attributes by accessing every item
//! in a table or a secondary index. To have DynamoDB return fewer items, you can provide a
//! FilterExpression operation. If the total size of scanned items exceeds the maximum
//! dataset size limit of 1 MB, the scan completes and results are returned to the user. The
//! LastEvaluatedKey value is also returned and the requestor can use the LastEvaluatedKey
//! to continue the scan in a subsequent operation. Each scan response also includes number
//! of items that were scanned (ScannedCount) as part of the request. If using a
//! FilterExpression, a scan result can result in no items meeting the criteria and the
//! Count will result in zero. If you did not use a FilterExpression in the scan request,
//! then Count is the same as ScannedCount. Count and ScannedCount only return the count of
//! items specific to a single scan request and, unless the table is less than 1MB, do not
//! represent the total number of items in the table. A single Scan operation first reads up
//! to the maximum number of items set (if using the Limit parameter) or a maximum of 1 MB
//! of data and then applies any filtering to the results if a FilterExpression is provided.
//! If LastEvaluatedKey is present in the response, pagination is required to complete the
//! full table scan. For more information, see Paginating the Results in the Amazon DynamoDB
//! Developer Guide. Scan operations proceed sequentially; however, for faster performance
//! on a large table or secondary index, applications can request a parallel Scan operation
//! by providing the Segment and TotalSegments parameters. For more information, see
//! Parallel Scan in the Amazon DynamoDB Developer Guide. By default, a Scan uses eventually
//! consistent reads when accessing the items in a table. Therefore, the results from an
//! eventually consistent Scan may not include the latest item changes at the time the scan
//! iterates through each item in the table. If you require a strongly consistent read of
//! each item as the scan iterates through the items in the table, you can set the
//! ConsistentRead parameter to true. Strong consistency only relates to the consistency of
//! the read at the item level. DynamoDB does not provide snapshot isolation for a scan
//! operation when the ConsistentRead parameter is set to true. Thus, a DynamoDB scan
//! operation does not guarantee that all reads in a scan see a consistent snapshot of the
//! table when the scan operation was requested.
//!
//! ## Input
//!
//! * `AttributesToGet` (AttributeNameList, len 1..) — This is a legacy parameter. Use
//!   ProjectionExpression instead. For more information, see AttributesToGet in the Amazon
//!   DynamoDB Developer Guide.
//! * `ConditionalOperator` (ConditionalOperator, one of: AND | OR) — This is a legacy
//!   parameter. Use FilterExpression instead. For more information, see ConditionalOperator
//!   in the Amazon DynamoDB Developer Guide.
//! * `ConsistentRead` (ConsistentRead) — A Boolean value that determines the read consistency
//!   model during the scan: If ConsistentRead is false, then the data returned from Scan
//!   might not contain the results from other recently completed write operations (PutItem,
//!   UpdateItem, or DeleteItem). If ConsistentRead is true, then all of the write operations
//!   that completed before the Scan began are guaranteed to be contained in the Scan
//!   response. The default setting for ConsistentRead is false. The ConsistentRead parameter
//!   is not supported on global secondary indexes. If you scan a global secondary index with
//!   ConsistentRead set to true, you will receive a ValidationException.
//! * `ExclusiveStartKey` (Key) — The primary key of the first item that this operation will
//!   evaluate. Use the value that was returned for LastEvaluatedKey in the previous
//!   operation. The data type for ExclusiveStartKey must be String, Number or Binary. No set
//!   data types are allowed. In a parallel scan, a Scan request that includes
//!   ExclusiveStartKey must specify the same segment whose previous Scan returned the
//!   corresponding value of LastEvaluatedKey.
//! * `ExpressionAttributeNames` (ExpressionAttributeNameMap) — One or more substitution
//!   tokens for attribute names in an expression. The following are some use cases for using
//!   ExpressionAttributeNames: To access an attribute whose name conflicts with a DynamoDB
//!   reserved word. To create a placeholder for repeating occurrences of an attribute name in
//!   an expression. To prevent special characters in an attribute name from being
//!   misinterpreted in an expression. Use the # character in an expression to dereference an
//!   attribute name. For example, consider the following attribute name: Percentile The name
//!   of this attribute conflicts with a reserved word, so it cannot be used directly in an
//!   expression. (For the complete list of reserved words, see Reserved Words …
//! * `ExpressionAttributeValues` (ExpressionAttributeValueMap) — One or more values that can
//!   be substituted in an expression. Use the : (colon) character in an expression to
//!   dereference an attribute value. For example, suppose that you wanted to check whether
//!   the value of the ProductStatus attribute was one of the following: Available |
//!   Backordered | Discontinued You would first need to specify ExpressionAttributeValues as
//!   follows: { ":avail":{"S":"Available"}, ":back":{"S":"Backordered"},
//!   ":disc":{"S":"Discontinued"} } You could then use these values in an expression, such as
//!   this: ProductStatus IN (:avail, :back, :disc) For more information on expression
//!   attribute values, see Condition Expressions in the Amazon DynamoDB Developer Guide.
//! * `FilterExpression` (ConditionExpression) — A string that contains conditions that
//!   DynamoDB applies after the Scan operation, but before the data is returned to you. Items
//!   that do not satisfy the FilterExpression criteria are not returned. A FilterExpression
//!   is applied after the items have already been read; the process of filtering does not
//!   consume any additional read capacity units. For more information, see Filter Expressions
//!   in the Amazon DynamoDB Developer Guide.
//! * `IndexName` (IndexName, len 3..255, pattern ^[a-zA-Z0-9_.-]+$) — The name of a secondary
//!   index to scan. This index can be any local secondary index or global secondary index.
//!   Note that if you use the IndexName parameter, you must also provide TableName.
//! * `Limit` (PositiveIntegerObject, range 1..) — The maximum number of items to evaluate
//!   (not necessarily the number of matching items). If DynamoDB processes the number of
//!   items up to the limit while processing the results, it stops the operation and returns
//!   the matching values up to that point, and a key in LastEvaluatedKey to apply in a
//!   subsequent operation, so that you can pick up where you left off. Also, if the processed
//!   dataset size exceeds 1 MB before DynamoDB reaches this limit, it stops the operation and
//!   returns the matching values up to the limit, and a key in LastEvaluatedKey to apply in a
//!   subsequent operation to continue the operation. For more information, see Working with
//!   Queries in the Amazon DynamoDB Developer Guide.
//! * `ProjectionExpression` (ProjectionExpression) — A string that identifies one or more
//!   attributes to retrieve from the specified table or index. These attributes can include
//!   scalars, sets, or elements of a JSON document. The attributes in the expression must be
//!   separated by commas. If no attribute names are specified, then all attributes will be
//!   returned. If any of the requested attributes are not found, they will not appear in the
//!   result. For more information, see Specifying Item Attributes in the Amazon DynamoDB
//!   Developer Guide.
//! * `ReturnConsumedCapacity` (ReturnConsumedCapacity, one of: INDEXES | NONE | TOTAL) —
//!   Determines the level of detail about either provisioned or on-demand throughput
//!   consumption that is returned in the response: INDEXES - The response includes the
//!   aggregate ConsumedCapacity for the operation, together with ConsumedCapacity for each
//!   table and secondary index that was accessed. Note that some operations, such as GetItem
//!   and BatchGetItem, do not access any indexes at all. In these cases, specifying INDEXES
//!   will only return ConsumedCapacity information for table(s). TOTAL - The response
//!   includes only the aggregate ConsumedCapacity for the operation. NONE - No
//!   ConsumedCapacity details are included in the response.
//! * `ScanFilter` (FilterConditionMap) — This is a legacy parameter. Use FilterExpression
//!   instead. For more information, see ScanFilter in the Amazon DynamoDB Developer Guide.
//! * `Segment` (ScanSegment, range 0..999999) — For a parallel Scan request, Segment
//!   identifies an individual segment to be scanned by an application worker. Segment IDs are
//!   zero-based, so the first segment is always 0. For example, if you want to use four
//!   application threads to scan a table or an index, then the first thread specifies a
//!   Segment value of 0, the second thread specifies 1, and so on. The value of
//!   LastEvaluatedKey returned from a parallel Scan request must be used as ExclusiveStartKey
//!   with the same segment ID in a subsequent Scan operation. The value for Segment must be
//!   greater than or equal to 0, and less than the value provided for TotalSegments. If you
//!   provide Segment, you must also provide TotalSegments.
//! * `Select` (Select, one of: ALL_ATTRIBUTES | ALL_PROJECTED_ATTRIBUTES | COUNT |
//!   SPECIFIC_ATTRIBUTES) — The attributes to be returned in the result. You can retrieve all
//!   item attributes, specific item attributes, the count of matching items, or in the case
//!   of an index, some or all of the attributes projected into the index. ALL_ATTRIBUTES -
//!   Returns all of the item attributes from the specified table or index. If you query a
//!   local secondary index, then for each matching item in the index, DynamoDB fetches the
//!   entire item from the parent table. If the index is configured to project all item
//!   attributes, then all of the data can be obtained from the local secondary index, and no
//!   fetching is required. ALL_PROJECTED_ATTRIBUTES - Allowed only when querying an index.
//!   Retrieves all attributes that …
//! * `TableName` (TableArn, len 1..1024) **required** — The name of the table containing the
//!   requested items or if you provide IndexName, the name of the table to which that index
//!   belongs. You can also provide the Amazon Resource Name (ARN) of the table in this
//!   parameter.
//! * `TotalSegments` (ScanTotalSegments, range 1..1000000) — For a parallel Scan request,
//!   TotalSegments represents the total number of segments into which the Scan operation will
//!   be divided. The value of TotalSegments corresponds to the number of application workers
//!   that will perform the parallel scan. For example, if you want to use four application
//!   threads to scan a table or an index, specify a TotalSegments value of 4. The value for
//!   TotalSegments must be greater than or equal to 1, and less than or equal to 1000000. If
//!   you specify a TotalSegments value of 1, the Scan operation will be sequential rather
//!   than parallel. If you specify TotalSegments, you must also specify Segment.
//!
//! ## Output
//!
//! * `ConsumedCapacity` (ConsumedCapacity) — The capacity units consumed by the Scan
//!   operation. The data returned includes the total provisioned throughput consumed, along
//!   with statistics for the table and any indexes involved in the operation.
//!   ConsumedCapacity is only returned if the ReturnConsumedCapacity parameter was specified.
//!   For more information, see Capacity unit consumption for read operations in the Amazon
//!   DynamoDB Developer Guide.
//! * `Count` (Integer, default 0) **required** — The number of items in the response. If you
//!   set ScanFilter in the request, then Count is the number of items returned after the
//!   filter was applied, and ScannedCount is the number of matching items before the filter
//!   was applied. If you did not use a filter in the request, then Count is the same as
//!   ScannedCount.
//! * `Items` (ItemList) — An array of item attributes that match the scan criteria. Each
//!   element in this array consists of an attribute name and the value for that attribute.
//! * `LastEvaluatedKey` (Key) — The primary key of the item where the operation stopped,
//!   inclusive of the previous result set. Use this value to start a new operation, excluding
//!   this value in the new request. If LastEvaluatedKey is empty, then the "last page" of
//!   results has been processed and there is no more data to be retrieved. If
//!   LastEvaluatedKey is not empty, it does not necessarily mean that there is more data in
//!   the result set. The only way to know when you have reached the end of the result set is
//!   when LastEvaluatedKey is empty.
//! * `ScannedCount` (Integer, default 0) **required** — The number of items evaluated, before
//!   any ScanFilter is applied. A high ScannedCount value with few, or no, Count results
//!   indicates an inefficient Scan operation. For more information, see Count and
//!   ScannedCount in the Amazon DynamoDB Developer Guide. If you did not use a filter in the
//!   request, then ScannedCount is the same as Count.
//!
//! ## Errors
//!
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

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ScanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ScanError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#Scan")]
#[derive(Debug)]
pub enum ScanErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::scan::ScanError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ScanError = crate::scaffolded::Scaffolded<ScanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ScanErrorModeled> for ScanError {
    fn from(modeled: ScanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Scan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Scan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Scan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Scan<Eng>
{
    type Input = ScanInput;
    type Output = ScanOutput;
    type Error = ScanError;

    const NAME: &'static str = "Scan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SCAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SCAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        // The table this read addresses, resolved in the Region the request reached. A
        // DATA-plane operation states a refusal about a FIELD of the ARN as the
        // table-name member's own constraint, the ARN echoed over the `tableName` path
        // ([`crate::state::resolve_table_name_in_clause`] holds both forms).
        let name =
            crate::state::resolve_table_name_in_clause(&input.table_name, &region).map_err(lift)?;

        // Everything that speaks only about the REQUEST is settled before any table is
        // looked up, so a request that cannot hold together is refused as the client
        // error it is whatever the store holds and whichever members it carries.
        //
        // A request states each of its asks ONCE: the expression language supersedes the
        // legacy members that narrow this read — its projection, its filter, the operator
        // that combines the filter map's entries — so any expression member beside one of
        // them is refused, and the ask each legacy member states orders the refusal, in the
        // order `smithy_query(query="operation", operation="Scan")` declares the expression
        // members that state those asks
        // ([`crate::state::check_expression_parameter_mix`]).
        crate::state::check_expression_parameter_mix(
            &[
                crate::state::stated_legacy(
                    "AttributesToGet",
                    input
                        .attributes_to_get
                        .as_ref()
                        .is_some_and(|names| !names.is_empty()),
                    PROJECTION_ASK,
                ),
                crate::state::stated_legacy(
                    "ScanFilter",
                    input.scan_filter.as_ref().is_some_and(|map| !map.is_empty()),
                    FILTER_ASK,
                ),
                crate::state::stated_legacy(
                    "ConditionalOperator",
                    input.conditional_operator.is_some(),
                    FILTER_ASK,
                ),
            ],
            &[
                (
                    "ProjectionExpression",
                    input.projection_expression.is_some(),
                ),
                ("FilterExpression", input.filter_expression.is_some()),
            ],
        )
        .map_err(lift)?;
        // "A filter expression is applied after a Scan finishes but before the results
        // are returned" (`Scan$FilterExpression`), stated either in that language or
        // through the legacy map it replaced — the one filter both readers of this
        // service state it through ([`crate::state::ReadFilter`]); it is read BEFORE the
        // store is, so a filter the service would refuse is refused without reading
        // anything.
        let filter = crate::state::ReadFilter::resolve(
            "ScanFilter",
            input.filter_expression.as_deref(),
            input.scan_filter.as_ref(),
            input.conditional_operator.as_deref(),
            crate::state::Substitutions::new(
                input.expression_attribute_names.as_ref(),
                input.expression_attribute_values.as_ref(),
            ),
        )
        .map_err(lift)?;
        let requested = crate::state::requested_projection(
            "ProjectionExpression",
            input.projection_expression.as_deref(),
            input.attributes_to_get.as_deref(),
            input.expression_attribute_names.as_ref(),
        )
        .map_err(lift)?;
        // Every token the request DEFINES must be one an expression uses, and a
        // substitution table the request has no expression to substitute into is refused
        // rather than silently ignored: both rules belong to the one checker every
        // operation with expression members shares
        // ([`crate::state::Substitutions::check_all_referenced`]).
        crate::state::Substitutions::new(
            input.expression_attribute_names.as_ref(),
            input.expression_attribute_values.as_ref(),
        )
        .check_all_referenced(&[
            ("FilterExpression", input.filter_expression.as_deref()),
            ("ProjectionExpression", input.projection_expression.as_deref()),
        ])
        .map_err(lift)?;
        // A value a filter compares against is held to the documented rules for its
        // type, exactly as a value a write stores is: the rules belong to the value,
        // not to the surface it arrived through. A value a READ only compares becomes no
        // item bytes at all, so the item ceiling is no bound on it
        // ([`crate::state::check_bare_substituted_value`]); the substitution table's own
        // total cap is.
        for (placeholder, value) in input.expression_attribute_values.iter().flatten() {
            crate::state::check_bare_substituted_value(placeholder, value).map_err(lift)?;
        }
        crate::state::check_substitution_total(input.expression_attribute_values.as_ref())
            .map_err(crate::state::unenveloped)
            .map_err(lift)?;
        // Which member names the index is a fact of the request, so what each reported
        // item carries is decided here too: the defaults differ between a table scan
        // and an index scan ([`resolve_selection`]).
        let selection = crate::state::resolve_selection(
            input.select.as_deref(),
            crate::state::stated_projection_member(
                input.projection_expression.as_deref(),
                input.attributes_to_get.as_deref(),
            ),
            requested,
            input.index_name.is_some(),
        )
        .map_err(lift)?;
        let segmentation = Segmentation::resolve(input.segment, input.total_segments)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        let consistent_read = input.consistent_read.unwrap_or(false);

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);
        let table = store.active_table(&region, &name, now).map_err(lift)?;
        // The one rule about the paths this scan names that the table settles: a key
        // attribute is declared as a scalar, so a path that reaches inside one addresses
        // no location this table can hold ([`crate::state::check_scalar_key_paths`]).
        // Asked of the projection and of the filter, the two members this scan addresses
        // document paths through, before anything else is decided about them.
        crate::state::check_scalar_key_paths(table, selection.projected_paths()).map_err(lift)?;
        // The FILTER's paths are held to the key schema this scan ADDRESSES — the table's
        // own, and the index's when the request names one — because a filter is decided over
        // the entries the scan walks ([`crate::state::check_scalar_filter_key_paths`]).
        crate::state::check_scalar_filter_key_paths(
            table,
            input.index_name.as_deref(),
            filter.document_paths(),
        )
        .map_err(lift)?;
        // A table the policies in force do not let this caller read answers the denial,
        // not its items: it is decided before anything is read. A scan addresses no
        // partition of its own, and asks for the attributes its selection names. A scan
        // THROUGH an index acts on that index's own ARN, which is what the decision and the
        // refusal name ([`crate::state::authorization::addressed_resource`]).
        if let Some(denial) = crate::state::authorization::denial_indexed(
            &caller,
            table,
            input.index_name.as_deref(),
            "dynamodb:Scan",
            now,
            &crate::state::authorization::RequestKeys::selected(
                crate::state::authorization::AddressedKeys::whole_table(),
                &selection,
            ),
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }
        // The items are stored ENCRYPTED under the table's KMS key, so a key KMS will not
        // use leaves nothing this scan could decrypt: the KMS fault is reported rather
        // than a page of items ([`crate::state::check_table_key_accessible`]).
        crate::state::check_table_key_accessible(&*self.engine, table).map_err(lift)?;
        // Which index this scan reads, with the two refusals that resolution carries —
        // all three facts of the request, decided before anything is read
        // ([`crate::state::resolve_index`]).
        let index = crate::state::resolve_index(
            table,
            OPERATION,
            input.index_name.as_deref(),
            consistent_read,
            &selection,
            now,
        )
        .map_err(lift)?;
        // "If you query or scan a global secondary index, you can only request
        // attributes that are projected into the index" (`Scan$Select`): a scan whose
        // projection names an attribute the index does not hold cannot be served from
        // the index, so it is refused rather than answered with a narrowed item. Its
        // FILTER is judged against the entry the index holds, so an attribute only the
        // filter names simply matches nothing
        // ([`crate::state::unprojected_projection_attributes`]).
        if let Some(index) = index.as_ref() {
            let unprojected = crate::state::unprojected_projection_attributes(
                index,
                &table.key_schema,
                &selection,
            );
            if !unprojected.is_empty() {
                return Err(lift(crate::state::unprojected_projection_refusal(
                    index.name,
                    &unprojected,
                )));
            }
        }
        // "If any of the requested attributes are not projected into the local secondary
        // index, DynamoDB fetches each of these attributes from the parent table"
        // (`Scan$Select`) — which is also what makes that read cost more, since what it
        // read is then the whole item rather than the index's entry.
        let fetches_from_table = index.as_ref().is_some_and(|index| {
            !index.global
                && crate::state::needs_unprojected(
                    index,
                    &table.key_schema,
                    &selection,
                    &filter.attributes(),
                )
        });

        // The walk is over what this scan READS: the table's items, or the entries one
        // secondary index holds ([`crate::state::IndexItemKey`]). The list is one entry
        // per STORED item, so its size is what the table HOLDS and not anything this
        // request chose; the entry each row is read as is built inside the page below,
        // where the page budget bounds it. The ORDER the rows are read in is settled
        // below, and is the order a `LastEvaluatedKey` resumes from.
        let candidates: Vec<(crate::state::ReadPosition<'_>, &crate::state::Item)> = match index {
            None => table
                .items
                .iter()
                .map(|(key, item)| (crate::state::ReadPosition::Table(key), item))
                .collect(),
            Some(index) => {
                // An item the index does not cover has no entry to scan at all — a
                // sparse index holds only the items that carry its key attributes
                // ([`crate::state::Table::index_item_key`]).
                let mut entries: Vec<(crate::state::IndexItemKey, &crate::state::Item)> = table
                    .items
                    .iter()
                    .filter_map(|(key, item)| {
                        table
                            .index_item_key(&index, key, item)
                            .map(|position| (position, item))
                    })
                    .collect();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
                    .into_iter()
                    .map(|(position, item)| (crate::state::ReadPosition::Index(position), item))
                    .collect()
            }
        };
        // Where a previous page stopped, read back through the schema this scan walks
        // ([`crate::state::resume_from`]). An EMPTY element of the token is refused
        // against a SECONDARY INDEX: this reader validates its token against one — the
        // index the request named, or the first the table declares where it named none —
        // so the refusal names that index and the element, whichever schema declares the
        // element as a key ([`crate::state::EmptyStartKeyElement`]). A table that declares
        // no index at all leaves the plain sentence, since there is no index to report.
        let empty_element = index
            .as_ref()
            .map(|index| index.name)
            .or_else(|| table.first_secondary_index_name())
            .map_or(
                crate::state::EmptyStartKeyElement::NamesTheKey,
                crate::state::EmptyStartKeyElement::NamesTheIndexKey,
            );
        let resumed = crate::state::resume_from(
            table,
            index.as_ref(),
            input.exclusive_start_key.as_ref(),
            empty_element,
            // A token element that names one of the index's own key attributes in another
            // type than the table declares for it is detailed by this reader: the element,
            // the two types and the index being walked
            // ([`crate::state::MistypedStartKeyElement`]).
            crate::state::MistypedStartKeyElement::NamesTheMismatch,
        )
        .map_err(lift)?;
        // Where the token resumes from sits in the same space the walk below is ordered
        // by, so the place of its partition ([`partition_hash`]) is taken once: the segment
        // check here and the forward filter further down both ask about that place.
        let resumed = resumed
            .as_ref()
            .map(|from| (partition_hash(from.partition(index.as_ref())), from));
        // "In a parallel scan, a Scan request that includes ExclusiveStartKey must specify
        // the same segment whose previous Scan returned the corresponding value of
        // LastEvaluatedKey" (`Scan$ExclusiveStartKey`). A segment is a block of the key
        // space ([`Segmentation::owns`]), so the place a token resumes from belongs to
        // exactly one of them: a token presented under another segment names a place this
        // request's segment does not scan, and resuming it there would read a slice of the
        // table twice and leave another unread. It is refused rather than served
        // ([`start_key_of_another_segment`]).
        if let Some((from_hash, _)) = resumed {
            if !segmentation.owns(from_hash) {
                return Err(lift(start_key_of_another_segment(&segmentation)));
            }
        }

        // Segment membership is decided from the row's own PARTITION KEY, not from where
        // the row sits in this walk ([`Segmentation::owns`]), so a later page of the same
        // segment sees the same share of what is scanned and a segment's share does not
        // move when another item is written. The walk itself is in the order the KEY SPACE
        // is laid out — each partition where the hash of its key puts it
        // ([`partition_hash`]), the rows of one partition in the order the store holds them
        // — which is the order a segment's block of that space is contiguous in, so a
        // segment reads a run of this walk and the segments together read all of it.
        //
        // The place is taken once per PARTITION: rows that share a partition key are
        // contiguous in `candidates` — the table's items are ordered partition-first
        // ([`crate::state::ItemKey`]) and an index's entries by the index key
        // ([`crate::state::IndexItemKey`]) — so one pass places each partition once and
        // reuses that answer for its run of rows.
        let mut ordered: Vec<WalkRow<'_>> = Vec::with_capacity(candidates.len());
        let mut placed: Option<(&[crate::state::AttributeValueKey], u64)> = None;
        for (position, item) in &candidates {
            let partition = position.partition(index.as_ref());
            let hash = match placed {
                Some((previous, hash)) if previous == partition => hash,
                _ => partition_hash(partition),
            };
            placed = Some((partition, hash));
            ordered.push(WalkRow {
                hash,
                position,
                item,
            });
        }
        ordered.sort_by(|left, right| {
            left.hash
                .cmp(&right.hash)
                .then_with(|| left.position.cmp(right.position))
        });
        let evaluated = ordered
            .iter()
            .filter(|row| segmentation.owns(row.hash))
            // "Use this value to start a new operation, EXCLUDING this value in the
            // new request" (`Scan$LastEvaluatedKey`). A scan always walks forward.
            .filter(|row| {
                resumed.is_none_or(|(from_hash, from)| {
                    row.hash > from_hash
                        || (row.hash == from_hash && row.position.past(from, true))
                })
            })
            .map(|row| (row.position, row.item));

        // "A single Scan operation first reads up to the maximum number of items set
        // (if using the Limit parameter) or a maximum of 1 MB of data" (the `Scan`
        // doc), so the page — and the work this request can ask for — is bounded
        // BEFORE any item is cloned, whether or not the caller named a `Limit`.
        let limit = input
            .limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0));
        let mut page: Vec<crate::state::Read<'_>> = Vec::new();
        let mut page_size: i64 = 0;
        // What the page COSTS: one read unit per 4 KB of EACH item it evaluated, half a
        // unit per 4 KB where the read is eventually consistent — the charge a read of one
        // item carries ([`crate::state::read_capacity_units`]), summed over the items this
        // scan examined, exactly as a batched read of the same items is charged. A scan of
        // a SECONDARY INDEX is charged over what the index holds for each row, in whole
        // units ([`crate::state::index_scan_capacity_units`]).
        let mut page_units: f64 = 0.0;
        // What this scan spends on the TABLE rather than on the index it read: the
        // base-table fetches it made, charged per item.
        let mut fetched_units: f64 = 0.0;
        let mut truncated = false;
        for (_, item) in evaluated {
            let read = crate::state::Read::new(
                item,
                index.as_ref(),
                &table.key_schema,
                fetches_from_table,
            );
            let size = crate::state::item_size(read.seen());
            // The first row always fits, the way DynamoDB always returns at least
            // one item it read; after that the page stops once the rows it already
            // holds have CROSSED the budget, so the item that carried it past the
            // limit is in the page and a truncated page is one the limit was
            // exceeded by ([`crate::state::page_budget_spent`]).
            if page.len() >= limit
                || (!page.is_empty() && crate::state::page_budget_spent(page_size))
            {
                truncated = true;
                break;
            }
            page_size = page_size.saturating_add(size);
            // Each figure the scan BILLS is weighed as this table meters an item
            // ([`crate::state::Table::metered_item_size`]) — a vector at the four bytes
            // per dimension it is held as — while the page's own budget above stays the
            // bytes the response carries.
            page_units += crate::state::walk_row_capacity_units(
                index.is_some(),
                table.metered_item_size(read.projected()),
                consistent_read,
            );
            // What the scan spends ON THE TABLE: an index read that had to fetch the
            // attributes its index does not project read the table's own copy of this
            // item, which is charged per item
            // ([`crate::state::fetch_capacity_units`]); what it spends on the INDEX is
            // what the index holds for the item, which is what `page_units` above sums.
            if fetches_from_table {
                fetched_units += crate::state::fetch_capacity_units(
                    table.metered_item_size(read.seen()),
                    consistent_read,
                );
            }
            page.push(read);
        }

        // "The primary key of the item where the operation stopped" — reported when
        // the walk stopped with items left to evaluate, when it stopped because it
        // reached the `Limit` the request asked for, and when it stopped because the
        // page spent its 1 MB budget, whether or not a row remains
        // ([`crate::state::reports_stop_key`], [`crate::state::page_budget_spent`]):
        // "if LastEvaluatedKey is not empty, it does not necessarily mean that there is
        // more data in the result set" (`Scan$LastEvaluatedKey`), so a page the budget
        // stopped carries a key and the client learns the scan is done from the page it
        // asks for next. A page that ran out of rows inside both bounds reports none,
        // which is what tells the client the scan is done. An index scan reports the
        // index's key beside the table's, because that pair is what addresses one entry
        // of an index.
        let stopped = truncated || crate::state::page_budget_spent(page_size);
        let last_evaluated_key = crate::state::reports_stop_key(input.limit, page.len(), stopped)
            .then(|| page.last())
            .flatten()
            .map(|read| crate::state::evaluated_key(table, index.as_ref(), read.item()));
        // "A filter expression is applied after a Scan finishes": every row the page
        // READ is scanned — and charged for — and the ones the filter does not match
        // are left out of the response. "ScannedCount [is] the number of items
        // evaluated, before any ScanFilter is applied"; with no filter, "Count is the
        // same as ScannedCount".
        let scanned_count = i32::try_from(page.len()).unwrap_or(i32::MAX);
        let mut matched: Vec<&crate::state::Read<'_>> = Vec::with_capacity(page.len());
        for read in &page {
            if filter.matches(read.seen()).map_err(lift)? {
                matched.push(read);
            }
        }
        let count = i32::try_from(matched.len()).unwrap_or(i32::MAX);
        // The projection is applied AFTER the filter, so an attribute the response does
        // not carry still narrows the scan.
        let items = crate::state::reported_items(&selection, &matched);

        // A scan that evaluated nothing still spends the one unit a read takes at least —
        // the charge the walk itself carries — which is what a read of a key no item
        // answers to is charged too. That base charge is the WALK's and not a row's: the
        // per-row rounding an index scan is charged in has no entry to apply to here, so an
        // empty page costs what the request asked to read with, index or no index.
        let units = if page.is_empty() {
            crate::state::read_capacity_units(0, consistent_read)
        } else {
            page_units
        };
        let requested_capacity = input.return_consumed_capacity.as_deref();
        // Projected while the index this scan read is still borrowed from the table, so
        // what the response bills is decided before the table is reached for again.
        let consumed_capacity = crate::state::consumed_read_capacity(
            requested_capacity,
            &name,
            index.as_ref(),
            units,
            fetched_units,
        );
        // WHICH throughput series these units are counted into is the line of
        // `ConsumedCapacity` this response bills them to
        // ([`crate::state::consumed_capacity_metrics`]): a global index's own series when the
        // scan read one, the table's otherwise. Taken as an owned name while the index is
        // still borrowed from the table, for the same reason the billing above is.
        let billed_index: Option<String> = index
            .as_ref()
            .filter(|index| index.global)
            .map(|index| index.name.to_string());
        // The read is ADMITTED against the read capacity the table is provisioned for:
        // the units it consumes are the ones this response bills — every item the page
        // scanned, filtered or not — and a read past that capacity is refused rather
        // than served. A read served from an index spends the TABLE's provisioned
        // capacity here, because an index of a provisioned table has no capacity of its
        // own in this store.
        if let Err(throttle) = store
            .active_table_mut(&region, &name, now)
            .map_err(lift)?
            .admit_read_from(billed_index.as_deref(), units + fetched_units, now)
        {
            return Err(ScanErrorModeled::ProvisionedThroughputExceededException(
                crate::state::read_throughput_exceeded(
                    &crate::state::table_arn(&region, &name),
                    throttle,
                ),
            )
            .into());
        }
        // The items this read HANDS BACK are the table's `ReturnedItemCount` for the
        // operation that returned them ([`crate::state::request_outcome_metrics`]): the
        // figure this response reports as its `Count`, which is what a filter left of the
        // page rather than what the page evaluated.
        store
            .pending_outcomes
            .record_returned_items(&name, OPERATION, count, now);
        Ok(ScanOutput {
            items,
            count,
            scanned_count,
            last_evaluated_key,
            consumed_capacity,
        })
    }
}

/// The API this operation is served as, which is the `Operation` dimension value the
/// per-operation CloudWatch series of a request it served are booked under
/// ([`crate::state::request_outcome_metrics`]).
const OPERATION: &str = "Scan";

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> ScanError {
    ScanErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> ScanError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            ScanErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}

/// How a parallel scan whose `Segment` is not inside its `TotalSegments` is refused:
/// "The value for Segment must be greater than or equal to 0, and less than the value
/// provided for TotalSegments" (`Scan$Segment`). The sentence is FIXED — it names the
/// two members the rule is about and reports neither value, so every client that
/// matches this refusal matches one string whatever it sent.
const SEGMENT_NOT_BELOW_TOTAL_SEGMENTS_MESSAGE: &str =
    "The Segment parameter is zero-based and must be less than parameter TotalSegments";

/// How a parallel scan resuming an `ExclusiveStartKey` of ANOTHER segment is refused: the
/// invalid-token statement every unusable resume token earns
/// ([`crate::state::START_KEY_INVALID_MESSAGE`]) and then WHY — that the token has to be
/// presented with its own segment — over the pair the request asked for.
///
/// "The value of LastEvaluatedKey returned from a parallel Scan request must be used as
/// ExclusiveStartKey with the same segment ID in a subsequent Scan operation"
/// (`Scan$Segment`), so the two numbers the sentence states are the request's own
/// `TotalSegments` and `Segment`: they are what tells a worker which share it asked for,
/// and the key itself is not echoed back.
fn start_key_of_another_segment(segmentation: &Segmentation) -> crate::state::RequestError {
    crate::state::RequestError::Validation(format!(
        "{}: Invalid ExclusiveStartKey. Please use ExclusiveStartKey with correct Segment. \
         TotalSegments: {} Segment: {}",
        crate::state::START_KEY_INVALID_MESSAGE, segmentation.total, segmentation.segment
    ))
}

// Which ASK each of this reader's legacy members states, as the position of the expression
// member that replaced it in the list passed beside them
// ([`crate::state::check_expression_parameter_mix`]) — the order
// `smithy_query(query="operation", operation="Scan")` declares those members in.

/// The ask `ProjectionExpression` states, which the legacy `AttributesToGet` stated.
const PROJECTION_ASK: usize = 0;

/// The ask `FilterExpression` states, which the legacy `ScanFilter` — and the
/// `ConditionalOperator` that combines its entries — stated.
const FILTER_ASK: usize = 1;

/// One row of the walk a `Scan` reads, as the walk ORDERS it: where the partition it
/// belongs to sits in the hash space ([`partition_hash`]), the row's own place in what is
/// being walked, and the stored item it reads.
///
/// `hash` first because that is what the walk is ordered by and what decides which segment
/// evaluates the row; the place breaks a tie, so rows of one partition stay in the order
/// the store holds them.
struct WalkRow<'a> {
    hash: u64,
    position: &'a crate::state::ReadPosition<'a>,
    item: &'a crate::state::Item,
}

/// Which share of the table one `Scan` request evaluates.
///
/// "TotalSegments represents the total number of segments into which the Scan
/// operation will be divided ... Segment identifies an individual segment to be
/// scanned by an application worker" (`Scan$TotalSegments` / `Scan$Segment`). What is
/// divided is the table's or index's KEY SPACE, and a key space is laid out by the HASH of
/// each partition key ("DynamoDB uses the partition key's value as input to an internal
/// hash function", the Developer Guide's "Partitions and data distribution"), so a segment
/// is a block of that hash space ([`Self::owns`]): the segments together cover every item
/// exactly once, no two of them cover the same item, every item sharing a partition key is
/// evaluated by one segment, and WHICH segment owns a partition follows from its key alone
/// — not from how many items the table holds, in what order they were written, or which
/// other partitions exist. A request that names neither member is the sequential scan: one
/// segment, holding everything.
#[derive(Debug, Clone, Copy)]
struct Segmentation {
    segment: usize,
    total: usize,
}

impl Segmentation {
    /// Bind the request's `Segment`/`TotalSegments` pair, which is all-or-nothing:
    /// "If you provide Segment, you must also provide TotalSegments" and "If you
    /// specify TotalSegments, you must also specify Segment", with "the value for
    /// Segment ... greater than or equal to 0, and less than the value provided for
    /// TotalSegments".
    fn resolve(segment: Option<i32>, total: Option<i32>) -> Result<Self, String> {
        match (segment, total) {
            (None, None) => Ok(Self {
                segment: 0,
                total: 1,
            }),
            (Some(_), None) => Err(
                "The TotalSegments parameter is required but was not present in the request when \
                 Segment parameter is present"
                    .to_string(),
            ),
            (None, Some(_)) => Err(
                "The Segment parameter is required but was not present in the request when \
                 parameter TotalSegments is present"
                    .to_string(),
            ),
            (Some(segment), Some(total)) => {
                if segment >= total {
                    return Err(SEGMENT_NOT_BELOW_TOTAL_SEGMENTS_MESSAGE.to_string());
                }
                // Both members' modelled ranges are non-negative, and they are
                // checked before this handler runs, so the conversions cannot fail.
                Ok(Self {
                    segment: usize::try_from(segment).unwrap_or(0),
                    total: usize::try_from(total).unwrap_or(1).max(1),
                })
            }
        }
    }

    /// Whether the partition whose key hashes to `hash` belongs to this request's segment.
    ///
    /// The segments divide the hash space the key space is laid out by into as many equal
    /// blocks as there are segments, and a partition sits where the hash of its key puts
    /// it ([`partition_hash`]) — so the blocks together cover every partition exactly
    /// once, no partition falls in two of them, and a segment's share is decided by the
    /// key alone. The sequential scan (one segment) owns the whole space, which is the
    /// same answer the division gives.
    fn owns(&self, hash: u64) -> bool {
        if self.total <= 1 {
            return true;
        }
        let total = u128::try_from(self.total).unwrap_or(1);
        let block = (u128::from(hash) * total) >> u64::BITS;
        usize::try_from(block).unwrap_or(0) == self.segment
    }
}

/// Where in the hash space one partition of the walked key space sits: the leading 64 bits
/// of a digest of its key values, as a fraction of that space.
///
/// "DynamoDB uses the partition key's value as input to an internal hash function. The
/// output value from the hash function determines the partition" (the Developer Guide's
/// "Partitions and data distribution"), so the place a partition occupies is a function of
/// its KEY and of nothing else — which is what makes a segment's share, and the order a
/// scan walks the partitions in, stable while the table is written to. THAT is the rule.
/// WHICH function places a key is internal to the service and published nowhere, so the one
/// here is internal to this emulator too — the digest this crate already digests with, over
/// each key component's own text ([`crate::state::AttributeValueKey::key_text`]),
/// length-prefixed so two components cannot run together into one value a different key
/// would also produce; an index key may carry several components, and they are all part of
/// the one partition they address. No caller can rely on WHERE a given key lands, only on
/// its landing in one place, in one segment, and staying there however the table is
/// written to.
///
/// The digest PLACES a key in that space and does nothing else: it signs nothing, verifies
/// nothing, protects nothing, and the value it answers is derived from a key the response
/// already carries, so nothing about it is secret.
fn partition_hash(partition: &[crate::state::AttributeValueKey]) -> u64 {
    use ::sha2::Digest;

    let mut digest = ::sha2::Sha256::new();
    for component in partition {
        let text = component.key_text();
        digest.update(u64::try_from(text.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(text.as_bytes());
    }
    let hashed = digest.finalize();
    let leading: [u8; 8] = hashed
        .get(..8)
        .and_then(|bytes| bytes.try_into().ok())
        .unwrap_or_default();
    u64::from_be_bytes(leading)
}
