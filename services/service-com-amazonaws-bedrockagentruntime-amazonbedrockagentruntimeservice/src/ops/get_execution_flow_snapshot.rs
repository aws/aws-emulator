// SPDX-License-Identifier: CC0-1.0

//! @op GetExecutionFlowSnapshot
//!
//! `GET /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}/executions/{executionIdentifier}/flowsnapshot
//! -> 200`. Reports "the flow definition snapshot used for a flow execution. The snapshot
//! represents the flow metadata and definition as it existed at the time the execution was
//! started."
//!
//! REPORTING, and a pure read: the answer is composed entirely from the snapshot the START
//! of this execution copied into its record (`crate::state::flow_snapshot`), so nothing is
//! read back off the Bedrock Agent build-time plane, whose flow may have been edited,
//! repointed or deleted since. "Even if the flow is edited after an execution starts, the
//! snapshot connected to the execution remains unchanged", and reading the plane that holds
//! the flow NOW could not keep that. Nothing about the execution's status is reported here
//! either, so no transition is materialised on this path.
//!
//! `definition` is that stored `FlowDefinition` rendered as the JSON object the member's
//! text "follows the structure of", and `flowIdentifier`, `flowAliasIdentifier` and
//! `flowVersion` are the ids the execution resolved to rather than the identifier forms the
//! request happened to carry. An `executionIdentifier` naming no execution of the addressed
//! alias is the modelled `ResourceNotFoundException`, in either form the shape admits.
//!
//! ## Input
//!
//! * `executionIdentifier` (FlowExecutionIdentifier, len ..2048, pattern
//!   ^[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10}/execution/[a
//!   …) **required** — The unique identifier of the flow execution.
//! * `flowAliasIdentifier` (FlowAliasIdentifier, len ..2048, pattern
//!   ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10})|(\bTSTALIASID\b|[0-9a-zA-Z]+)$)
//!   **required** — The unique identifier of the flow alias used for the flow execution.
//! * `flowIdentifier` (FlowIdentifier, len ..2048, pattern
//!   ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$)
//!   **required** — The unique identifier of the flow.
//!
//! ## Output
//!
//! * `customerEncryptionKeyArn` (KmsKeyArn, len 1..2048, pattern
//!   ^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$) — The Amazon
//!   Resource Name (ARN) of the customer managed KMS key that's used to encrypt the flow
//!   snapshot.
//! * `definition` (String) **required** — The flow definition used for the flow execution,
//!   including the nodes, connections, and configuration at the time when the execution
//!   started. The definition returns as a string that follows the structure of a
//!   FlowDefinition object.
//! * `executionRoleArn` (FlowExecutionRoleArn, len ..2048, pattern
//!   ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$) **required** — The Amazon
//!   Resource Name (ARN) of the IAM service role that's used by the flow execution.
//! * `flowAliasIdentifier` (FlowAliasIdentifier, len ..2048, pattern
//!   ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10})|(\bTSTALIASID\b|[0-9a-zA-Z]+)$)
//!   **required** — The unique identifier of the flow alias used for the flow execution.
//! * `flowIdentifier` (FlowIdentifier, len ..2048, pattern
//!   ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$)
//!   **required** — The unique identifier of the flow.
//! * `flowVersion` (Version, len 1..5, pattern ^(DRAFT|[0-9]{0,4}[1-9][0-9]{0,4})$)
//!   **required** — The version of the flow used for the flow execution.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions. Check your permissions and retry your request.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

use crate::state::flow_execution as executions;
use crate::state::flow_invocation as flows;
use crate::state::flow_snapshot as snapshots;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetExecutionFlowSnapshotError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetExecutionFlowSnapshotError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#GetExecutionFlowSnapshot")]
#[derive(Debug)]
pub enum GetExecutionFlowSnapshotErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_execution_flow_snapshot::GetExecutionFlowSnapshotError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetExecutionFlowSnapshotError = crate::scaffolded::Scaffolded<GetExecutionFlowSnapshotErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetExecutionFlowSnapshotErrorModeled> for GetExecutionFlowSnapshotError {
    fn from(modeled: GetExecutionFlowSnapshotErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetExecutionFlowSnapshot` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetExecutionFlowSnapshot<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetExecutionFlowSnapshot<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetExecutionFlowSnapshot<Eng>
{
    type Input = GetExecutionFlowSnapshotInput;
    type Output = GetExecutionFlowSnapshotOutput;
    type Error = GetExecutionFlowSnapshotError;

    const NAME: &'static str = "GetExecutionFlowSnapshot";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_EXECUTION_FLOW_SNAPSHOT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_EXECUTION_FLOW_SNAPSHOT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal("the flow execution store is unavailable"));
        };

        // Decided against the flow alias the execution runs on, before any store is read,
        // exactly as the start, the read and the stop of one are.
        let resource =
            flows::alias_resource_arn(ctx, &input.flow_identifier, &input.flow_alias_identifier);
        if let Err(denied) = crate::state::authorize(&*self.engine, ctx, Self::NAME, &resource) {
            return Err(GetExecutionFlowSnapshotErrorModeled::AccessDeniedException(denied).into());
        }

        let not_found = || {
            GetExecutionFlowSnapshotErrorModeled::ResourceNotFoundException(
                executions::execution_not_found(),
            )
        };
        // The flow and the alias the labels name, resolved without reading the build-time
        // plane: this operation addresses a resource of this plane's OWN store, as the read
        // of one execution and the listing of its events do.
        let Ok(placed) = flows::placement(
            &*self.engine,
            ctx,
            &input.flow_identifier,
            &input.flow_alias_identifier,
        ) else {
            return Err(not_found().into());
        };
        let Some(name) = executions::execution_name_from_identifier(
            &input.execution_identifier,
            &placed.flow_id,
            &placed.alias_id,
        ) else {
            return Err(not_found().into());
        };

        let guard = state.lock();
        let key = executions::FlowExecutionKey {
            flow_id: placed.flow_id,
            alias_id: placed.alias_id,
            name: name.to_string(),
        };
        let Some(execution) = guard.flow_executions.get(&key) else {
            return Err(not_found().into());
        };
        // The definition the START copied, rendered as the structure the member's text
        // follows. A structure the serializer refuses leaves this read with no definition to
        // report, which is a fault of this service rather than of the request.
        let Some(definition) = snapshots::definition_text(&execution.snapshot.definition) else {
            return Err(internal(
                "the flow definition this execution was started against cannot be rendered",
            ));
        };
        Ok(GetExecutionFlowSnapshotOutput {
            flow_identifier: execution.flow_id.clone(),
            flow_alias_identifier: execution.alias_id.clone(),
            flow_version: execution.flow_version.clone(),
            execution_role_arn: execution.snapshot.execution_role_arn.clone(),
            definition,
            customer_encryption_key_arn: execution.snapshot.customer_encryption_key_arn.clone(),
        })
    }
}

/// This operation's modelled 500 `InternalServerException`, for a store this engine does not
/// carry and for a stored definition that cannot be rendered — answered as a server error,
/// never as a not-found.
fn internal(message: &str) -> GetExecutionFlowSnapshotError {
    GetExecutionFlowSnapshotErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.to_string()),
        reason: None,
    })
    .into()
}
