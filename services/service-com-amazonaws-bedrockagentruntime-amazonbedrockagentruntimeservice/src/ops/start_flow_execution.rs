// SPDX-License-Identifier: CC0-1.0

//! @op StartFlowExecution
//!
//! `POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}/executions -> 200`. Starts
//! one asynchronous EXECUTION of the flow the alias routes to and answers the ARN that
//! identifies it.
//!
//! ## What is real here
//!
//! The flow, the alias and the version the alias routes to are the Bedrock Agent build-time
//! plane's resources, so `crate::state::flow_invocation` resolves all three there and walks
//! the definition that version packages — the SAME resolution and the SAME walk `InvokeFlow`
//! runs, because an execution runs the flow a client could also invoke synchronously. A
//! call that cannot run the flow AT ALL is refused here, as the HTTP error its modelled
//! class carries, because this operation is not streaming: `ResourceNotFoundException` for a
//! flow, alias or version that plane does not hold, `ValidationException` for a version
//! nothing has prepared, and `DependencyFailedException` for an input that addresses no
//! input node of the flow, which leaves that node without the input the run depends on.
//!
//! A flow that IS invocable is STARTED, and the outcome of its run is the execution's own
//! status: NO LANGUAGE MODEL, LAMBDA FUNCTION OR OTHER EXTERNAL WORKER RUNS IN THIS
//! EMULATOR, so a run whose path leaves the input node for any other node kind comes to
//! `Failed` carrying the sentence saying which node nothing computed a value for, rather
//! than to a `Succeeded` with a value nothing produced. That is the channel an asynchronous
//! execution reports a failed run in; the synchronous `InvokeFlow` delivers the same fact as
//! an in-stream exception instead.
//!
//! A started execution is this plane's own resource (`crate::state::flow_execution`): it is
//! `Running`, its run is due to finish `FLOW_EXECUTION_SPAN` later, and `GetFlowExecution`
//! and `StopFlowExecution` read and move exactly what is written here. The flow the run
//! resolved — the definition the routed version packages, the role it runs with and the key
//! it is encrypted with — is COPIED into that record as well
//! (`crate::state::flow_snapshot`), because that is the snapshot
//! `GetExecutionFlowSnapshot` reports and a flow edited after the start does not move it.
//!
//! ## Input
//!
//! * `flowAliasIdentifier` (FlowAliasIdentifier, len ..2048, pattern
//!   ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10})|(\bTSTALIASID\b|[0-9a-zA-Z]+)$)
//!   **required** — The unique identifier of the flow alias to use for the flow execution.
//! * `flowExecutionName` (FlowExecutionName, len ..36, pattern ^[a-zA-Z0-9-]{1,36}$) — The
//!   unique name for the flow execution. If you don't provide one, a system-generated name is
//!   used.
//! * `flowIdentifier` (FlowIdentifier, len ..2048, pattern
//!   ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$)
//!   **required** — The unique identifier of the flow to execute.
//! * `inputs` (FlowInputs, len 1..1) **required** — The input data required for the flow
//!   execution. This must match the input schema defined in the flow.
//! * `modelPerformanceConfiguration` (ModelPerformanceConfiguration) — The performance
//!   settings for the foundation model used in the flow execution.
//!
//! ## Output
//!
//! * `executionArn` (FlowExecutionIdentifier, len ..2048, pattern
//!   ^[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10}/execution/[a
//!   …) — The Amazon Resource Name (ARN) that uniquely identifies the flow execution.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions. Check your permissions and retry your request.
//! * `BadGatewayException` (HTTP 502) — There was an issue with a dependency due to a server
//!   issue. Retry your request.
//! * `ConflictException` (HTTP 409) — There was a conflict performing an operation. Resolve
//!   the conflict and retry your request.
//! * `DependencyFailedException` (HTTP 424) — There was an issue with a dependency. Check the
//!   resource configurations and retry the request.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

use crate::state::flow_execution as executions;
use crate::state::flow_invocation as flows;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartFlowExecutionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartFlowExecutionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#StartFlowExecution")]
#[derive(Debug)]
pub enum StartFlowExecutionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_flow_execution::StartFlowExecutionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartFlowExecutionError = crate::scaffolded::Scaffolded<StartFlowExecutionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartFlowExecutionErrorModeled> for StartFlowExecutionError {
    fn from(modeled: StartFlowExecutionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartFlowExecution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartFlowExecution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartFlowExecution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartFlowExecution<Eng>
{
    type Input = StartFlowExecutionInput;
    type Output = StartFlowExecutionOutput;
    type Error = StartFlowExecutionError;

    const NAME: &'static str = "StartFlowExecution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_FLOW_EXECUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_FLOW_EXECUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal("the flow execution store is unavailable"));
        };

        // Authorization is decided against the flow alias the execution runs on — the
        // resource an identity-based policy for `bedrock:StartFlowExecution` names — before
        // any store is read, so an unauthorized call is answered the same way whether or not
        // the flow exists. The same resource `InvokeFlow` decides against, through the one
        // helper both read it from.
        let resource = flows::alias_resource_arn(
            ctx,
            &input.flow_identifier,
            &input.flow_alias_identifier,
        );
        if let Err(denied) = crate::state::authorize(&*self.engine, ctx, Self::NAME, &resource) {
            return Err(StartFlowExecutionErrorModeled::AccessDeniedException(denied).into());
        }

        // `inputs` is modelled `@length(min: 1, max: 1)`, so the framework has already
        // refused a request carrying none; the branch states the member it would be about
        // rather than unwrapping.
        let Some(entry) = input.inputs.first() else {
            return Err(crate::scaffolded::Scaffolded::validation(
                "inputs carries no input to enter the flow with",
            ));
        };
        let FlowInputContent::Document(document) = &entry.content;
        // The run the execution IS: resolved and walked exactly as an invocation of the same
        // alias would be, so an execution is started only for a flow this emulator really
        // ran, and the outcome of that run is what the execution reports (see the module
        // docs).
        let run = flows::run_resolved(
            &*self.engine,
            ctx,
            &input.flow_identifier,
            &input.flow_alias_identifier,
            &entry.node_name,
            document,
        )
        .map_err(|refusal| match refusal {
            flows::FlowRefusal::StoreUnavailable => internal("the flow store is unavailable"),
            flows::FlowRefusal::NotFound(resource) => {
                StartFlowExecutionErrorModeled::ResourceNotFoundException(
                    crate::state::flow_resource_not_found(resource.as_wire()),
                )
                .into()
            }
            flows::FlowRefusal::InputNodeUnavailable(node) => {
                StartFlowExecutionErrorModeled::DependencyFailedException(
                    DependencyFailedException {
                        message: Some(flows::dependency_failed_message()),
                        resource_name: Some(node),
                    },
                )
                .into()
            }
            flows::FlowRefusal::Unservable(message) => {
                crate::scaffolded::Scaffolded::validation(message)
            }
        })?;

        let started = ::std::time::SystemTime::now();
        // What the run came to, which is the status the execution reaches when its span is
        // up: the values the walk delivered, or the sentence saying which node of the flow
        // nothing here could compute one for.
        let (completes_as, failure) = match run.outcome {
            Ok(_delivered) => (executions::FlowExecutionStatus::Succeeded, None),
            Err(message) => (executions::FlowExecutionStatus::Failed, Some(message)),
        };
        // "If you don't provide one, a system-generated name is used"
        // (`smithy_query(query="doc",shape="StartFlowExecutionRequest",member="flowExecutionName")`).
        // The minted form is a version-4 UUID (`crate::state::new_uuid`), whose 36 characters
        // of lowercase hexadecimal and hyphens sit inside the modelled `FlowExecutionName`'s
        // `@length(1, 36)` and satisfy its `@pattern ^[a-zA-Z0-9-]{1,36}$`.
        let name = input
            .flow_execution_name
            .clone()
            .unwrap_or_else(crate::state::new_uuid);
        let execution_arn = executions::execution_arn(
            &crate::state::request_region(ctx),
            &run.flow_id,
            &run.alias_id,
            &name,
        );

        let mut guard = state.lock();
        executions::materialise_due_transitions(&mut guard, started);
        let key = executions::FlowExecutionKey {
            flow_id: run.flow_id.clone(),
            alias_id: run.alias_id.clone(),
            name: name.clone(),
        };
        if guard.flow_executions.contains_key(&key) {
            return Err(StartFlowExecutionErrorModeled::ConflictException(
                executions::execution_name_in_use(&name),
            )
            .into());
        }
        if guard.flow_executions.len() >= executions::MAX_FLOW_EXECUTIONS {
            // Resource guard, not an AWS quota: see `state::flow_execution::MAX_FLOW_EXECUTIONS`.
            return Err(StartFlowExecutionErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "This emulator holds at most {} flow executions at once.",
                        executions::MAX_FLOW_EXECUTIONS
                    )),
                },
            )
            .into());
        }
        let order = crate::state::next_order(&mut guard.order_counter);
        let record = executions::FlowExecution {
            name,
            execution_arn,
            flow_id: run.flow_id,
            alias_id: run.alias_id,
            flow_version: run.flow_version,
            // The flow as it stood when this start resolved the run, copied into the
            // execution's own record: `GetExecutionFlowSnapshot` reports it, and a later
            // edit of the flow leaves it unchanged (`crate::state::flow_snapshot`).
            snapshot: run.snapshot,
            started_at: ::aws_smithy_types::DateTime::from(started),
            completes_at: started + executions::FLOW_EXECUTION_SPAN,
            completes_as,
            failure,
            // The steps the run really took, recorded beside its outcome so the events of
            // this execution are read back from the run rather than composed afresh.
            trace: run.trace,
            status: executions::FlowExecutionStatus::Running,
            ended_at: None,
            order,
        };
        // The response is the record being persisted, so nothing can be answered that did
        // not reach the store.
        let out = StartFlowExecutionOutput {
            execution_arn: Some(record.execution_arn.clone()),
        };
        guard.flow_executions.insert(key, record);
        Ok(out)
    }
}

/// This operation's modelled 500 `InternalServerException`, for a store this engine does not
/// carry — answered as a server error, never as a not-found.
fn internal(message: &str) -> StartFlowExecutionError {
    StartFlowExecutionErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.to_string()),
        reason: None,
    })
    .into()
}
