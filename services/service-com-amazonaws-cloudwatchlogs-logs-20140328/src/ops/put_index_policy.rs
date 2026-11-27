// SPDX-License-Identifier: CC0-1.0

//! @op PutIndexPolicy
//!
//! The field index policy of ONE log group: `PutIndexPolicy` stores it on the group its
//! `logGroupIdentifier` names (a log-group level policy is named by nothing but that
//! group, so a later put REPLACES it), `DescribeIndexPolicies` reports it back ahead of
//! any account-wide `FIELD_INDEX_POLICY`, and `DeleteIndexPolicy` removes it. The record
//! is [`crate::state::IndexPolicyRecord`], stored on
//! [`crate::state::LogGroupRecord::index_policy`].
//!
//! A log-group-level policy "is identified by nothing but the group it applies to", so a
//! later put REPLACES the stored record rather than adding another, and the response is
//! read back out of that record — the same projection `DescribeIndexPolicies` reports,
//! with `source` `LOG_GROUP` and no `policyName` ("Responses about log group-level field
//! index policies don't have this field, because those policies don't have names").
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutIndexPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json")
//! ```
//!
//! Creates or updates a field index policy for the specified log group. Only log groups in
//! the Standard log class support field index policies. For more information about log
//! classes, see Log classes. You can use field index policies to create field indexes on
//! fields found in log events in the log group. Creating field indexes speeds up and lowers
//! the costs for CloudWatch Logs Insights queries that reference those field indexes,
//! because these queries attempt to skip the processing of log events that are known to not
//! match the indexed field. Good fields to index are fields that you often need to query
//! for and fields or values that match only a small fraction of the total log events.
//! Common examples of indexes include request ID, session ID, userID, and instance IDs. For
//! more information, see Create field indexes to improve query performance and reduce
//! costs. You can configure indexed fields as facets to enable interactive exploration and
//! filtering of your logs in the CloudWatch Logs Insights console. Facets allow you to view
//! value distributions and counts for indexed fields without running queries. When you
//! create a field index, you can optionally set it as a facet to enable this interactive
//! analysis capability. For more information, see Use facets to group and explore logs. To
//! find the fields that are in your log group events, use the GetLogGroupFields operation.
//! For example, suppose you have created a field index for requestId. Then, any CloudWatch
//! Logs Insights query on that log group that includes requestId = value or requestId IN
//! [value, value, ...] will process fewer log events to reduce costs, and have improved
//! performance. CloudWatch Logs provides default field indexes for all log groups in the
//! Standard log class. Default field indexes are automatically available for the following
//! fields: @logStream @aws.region @aws.account @source.log traceId Default field indexes
//! are in addition to any custom field indexes you define within your policy. Default field
//! indexes are not counted towards your field index quota. Each index policy has the
//! following quotas and restrictions: As many as 20 fields can be included in the policy.
//! Each field name can include as many as 100 characters. Matches of log events to the
//! names of indexed fields are case-sensitive. For example, a field index of RequestId
//! won't match a log event containing requestId. Log group-level field index policies
//! created with PutIndexPolicy override account-level field index policies created with
//! PutAccountPolicy that apply to log groups. If you use PutIndexPolicy to create a field
//! index policy for a log group, that log group uses only that policy for log group-level
//! indexing, including any facet configurations. The log group ignores any account-wide
//! field index policy that applies to log groups, but data source-based account policies
//! may still apply.
//!
//! ## Input
//!
//! * `logGroupIdentifier` (LogGroupIdentifier, len 1..2048, pattern ^[\w#+=/:,.@-]*$)
//!   **required** — Specify either the log group name or log group ARN to apply this field
//!   index policy to. If you specify an ARN, use the format
//!   arn:aws:logs:region:account-id:log-group:log_group_name Don't include an * at the end.
//! * `policyDocument` (PolicyDocument, len 1..51200) **required** — The index policy
//!   document, in JSON format. The following is an example of an index policy document that
//!   creates indexes with different types. "policyDocument": "{"Fields": [ "TransactionId" ],
//!   "FieldsV2": {"RequestId": {"type": "FIELD_INDEX"}, "APIName": {"type": "FACET"},
//!   "StatusCode": {"type": "FACET"}}}" You can use FieldsV2 to specify the type for each
//!   field. Supported types are FIELD_INDEX and FACET. Field names within Fields and FieldsV2
//!   must be mutually exclusive. The policy document must include at least one field index.
//!   For more information about the fields that can be included and other restrictions, see
//!   Field index syntax and quotas.
//!
//! ## Output
//!
//! * `indexPolicy` (IndexPolicy) — The index policy that you just created or updated.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — A parameter is specified incorrectly.
//! * `LimitExceededException` (HTTP 400) — You have reached the maximum number of resources
//!   that can be created.
//! * `OperationAbortedException` (HTTP 400) — Multiple concurrent requests to update the same
//!   resource were in conflict.
//! * `ResourceNotFoundException` (HTTP 400) — The specified resource does not exist.
//! * `ServiceUnavailableException` (HTTP 500) — The service cannot complete the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutIndexPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutIndexPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudwatchlogs#PutIndexPolicy")]
#[derive(Debug)]
pub enum PutIndexPolicyErrorModeled {
    InvalidParameterException(InvalidParameterException),
    LimitExceededException(LimitExceededException),
    OperationAbortedException(OperationAbortedException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_index_policy::PutIndexPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutIndexPolicyError = crate::scaffolded::Scaffolded<PutIndexPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutIndexPolicyErrorModeled> for PutIndexPolicyError {
    fn from(modeled: PutIndexPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutIndexPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutIndexPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutIndexPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutIndexPolicy<Eng>
{
    type Input = PutIndexPolicyInput;
    type Output = PutIndexPolicyOutput;
    type Error = PutIndexPolicyError;

    const NAME: &'static str = "PutIndexPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_INDEX_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_INDEX_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries only request metadata; the group this policy attaches to comes
        // from the request's `logGroupIdentifier`, not from its headers.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            PutIndexPolicyError::from(PutIndexPolicyErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the log store is unavailable".to_string()),
                },
            ))
        })?;
        let mut store = state.lock();

        let invalid = |message: String| {
            PutIndexPolicyError::from(PutIndexPolicyErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(message),
                },
            ))
        };

        // "The index policy document, in JSON format" — what such a document must say is
        // that member's own contract, shared with the account-level `FIELD_INDEX_POLICY`
        // a put of the same document creates, so the check and the sentences it refuses
        // with live beside the stored record in `crate::state`. What it hands back is the
        // field indexes the document names.
        let fields = crate::state::check_field_index_policy_document(&input.policy_document)
            .map_err(invalid)?;

        // "Each index policy has the following quotas and restrictions: As many as 20
        // fields can be included in the policy. Each field name can include as many as 100
        // characters." A document that breaks either is a `policyDocument` stated
        // incorrectly — a value of this request, which no retry of it can make acceptable —
        // rather than a count of the resources the account holds, which is what this
        // operation's limit error reports.
        crate::state::check_index_policy_quotas(&fields).map_err(invalid)?;

        // `logGroupIdentifier` is "either the log group name or log group ARN", so an ARN
        // resolves to the name the store is keyed by.
        let log_group_name =
            crate::state::log_group_name_from_identifier(&input.log_group_identifier).to_string();
        // A log group the account does not hold is refused the way this surface words a
        // missing group ([`crate::state::INDEX_POLICY_LOG_GROUP_NOT_FOUND_MESSAGE`]).
        let Some(group) = store.log_groups.get_mut(&log_group_name) else {
            return Err(PutIndexPolicyErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(
                        crate::state::INDEX_POLICY_LOG_GROUP_NOT_FOUND_MESSAGE.to_string(),
                    ),
                },
            )
            .into());
        };

        // "Only log groups in the Standard log class support field index policies": a group
        // in any other class cannot carry one, and this operation models no error for an
        // operation invalid on a resource, so the identifier naming such a group is the
        // parameter that is wrong. What the refusal SAYS is this service's one sentence for a
        // feature the class does not support
        // ([`crate::state::LOG_CLASS_UNSUPPORTED_OPERATION_MESSAGE`]).
        if group.log_group_class != crate::state::INDEX_POLICY_LOG_GROUP_CLASS {
            return Err(invalid(
                crate::state::LOG_CLASS_UNSUPPORTED_OPERATION_MESSAGE.to_string(),
            ));
        }

        // "Creates or updates a field index policy for the specified log group": the put
        // replaces whatever log-group level policy the group holds, and the document is
        // stored exactly as it arrived, because the service reports a stored policy
        // document back unchanged.
        let now = crate::state::now_ms();
        group.index_policy = Some(crate::state::IndexPolicyRecord {
            policy_document: input.policy_document.clone(),
            last_update_time_ms: now,
        });

        Ok(PutIndexPolicyOutput {
            // "The index policy that you just created or updated", which is what
            // `DescribeIndexPolicies` reports for this group: the group's ARN whichever
            // identifier form the request named it by, no `policyName` ("Responses about
            // log group-level field index policies don't have this field, because those
            // policies don't have names") and the source of a policy stored on one group.
            index_policy: Some(IndexPolicy {
                log_group_identifier: Some(group.log_group_arn.clone()),
                last_update_time: Some(now),
                policy_document: Some(input.policy_document),
                policy_name: None,
                source: Some(crate::state::INDEX_SOURCE_LOG_GROUP.to_string()),
            }),
        })
    }
}
