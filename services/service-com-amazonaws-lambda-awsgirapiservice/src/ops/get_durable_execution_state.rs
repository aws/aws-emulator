// SPDX-License-Identifier: CC0-1.0

//! @op GetDurableExecutionState
//!
//! Reports the OPERATIONS a durable execution's replay has to resume from, for the
//! checkpoint token the caller holds: `GET
//! /2025-12-01/durable-executions/{DurableExecutionArn}/state`.
//!
//! # Modeled contract for `GetDurableExecutionState`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")
//! ```
//!
//! Retrieves the current execution state required for the replay process during durable
//! function execution. This API is used by the Lambda durable functions SDK to get state
//! information needed for replay. You typically don't need to call this API directly as the
//! SDK handles state management automatically. The response contains operations ordered by
//! start sequence number in ascending order. Completed operations with children don't
//! include child operation details since they don't need to be replayed.
//!
//! ## Input
//!
//! * `CheckpointToken` (CheckpointToken, len 1..2048, pattern ^[A-Za-z0-9+/]+={0,2}$)
//!   **required** — A checkpoint token that identifies the current state of the execution.
//!   This token is provided by the Lambda runtime and ensures that state retrieval is
//!   consistent with the current execution context.
//! * `DurableExecutionArn` (DurableExecutionArn, len 1..1024, pattern
//!   ^arn:([a-zA-Z0-9-]+):lambda:([a-zA-Z0-9-]+):(\d{12}):function:([a-zA-Z0-9_-]+):(\$LATEST(?:\.PUBLISHED)?|[0-9]+)/durable
//!   …) **required** — The Amazon Resource Name (ARN) of the durable execution.
//! * `Marker` (String) — If NextMarker was returned from a previous request, use this value
//!   to retrieve the next page of operations. Each pagination token expires after 24 hours.
//! * `MaxItems` (ItemCount, range 0..1000, default 0) **required** — The maximum number of
//!   operations to return per call. You can use Marker to retrieve additional pages of
//!   results. The default is 100 and the maximum allowed is 1000. A value of 0 uses the
//!   default.
//!
//! ## Output
//!
//! * `NextMarker` (String) — If present, indicates that more operations are available. Use
//!   this value as the Marker parameter in a subsequent request to retrieve the next page of
//!   results.
//! * `Operations` (Operations) **required** — An array of operations that represent the
//!   current state of the durable execution. Operations are ordered by their start sequence
//!   number in ascending order and include information needed for replay processing.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — One of the parameters in the request is
//!   not valid.
//! * `KMSAccessDeniedException` (HTTP 502) — Lambda couldn't decrypt the environment
//!   variables because KMS access was denied. Check the Lambda function's KMS permissions.
//! * `KMSDisabledException` (HTTP 502) — Lambda couldn't decrypt the environment variables
//!   because the KMS key used is disabled. Check the Lambda function's KMS key settings.
//! * `KMSInvalidStateException` (HTTP 502) — Lambda couldn't decrypt the environment
//!   variables because the state of the KMS key used is not valid for Decrypt. Check the
//!   function's KMS key settings.
//! * `KMSNotFoundException` (HTTP 502) — Lambda couldn't decrypt the environment variables
//!   because the KMS key was not found. Check the function's KMS key settings.
//! * `ServiceException` (HTTP 500) — The Lambda service encountered an internal error.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was exceeded. For
//!   more information, see Lambda quotas.
//!
//! ## What this emulator answers
//!
//! The state is READ off the execution's own history, which is the record of what its run
//! really did (`crate::state::durable_steps`): one `Operation` per operation of the
//! execution, folded from the events that operation took — the event that STARTED it gives
//! its type, name, parent and start instant, and the event that ended it gives its status,
//! its end instant and the result or error it came to. Nothing is invented: an execution
//! whose run has taken no operation yet reports the one operation it is, and an operation the
//! run never took is reported by nothing.
//!
//! The `CheckpointToken` is "a checkpoint token that identifies the current state of the
//! execution […] provided by the Lambda runtime", so a token that is not the one currently
//! issued for this execution identifies no state to report and is refused
//! `InvalidParameterValueException` with the sentence the same token rule is refused with on
//! `CheckpointDurableExecution` (`crate::state::INVALID_CHECKPOINT_TOKEN_MESSAGE`) — which is
//! every token for an execution whose history is CLOSED, because such an execution has no
//! current checkpoint state at all.
//!
//! Reading an operation's payloads decrypts them under the key the function's `DurableConfig`
//! names, which is why this operation binds the four KMS failures at 502: the key is asked
//! about at the point of use, exactly as `GetDurableExecution` asks
//! (`crate::state::durable_data_key_fault`).
//!
//! This is a READ: it reports the token's state and consumes nothing — the token is rotated
//! by a checkpoint and by nothing else.

#[allow(unused_imports)]
use super::types::*;

// The `OperationType` values the fold below reports, where this service's state names them:
// one spelling per type, shared with the appender that writes the history these operations
// are read out of (`crate::state::durable_steps`).
use crate::state::durable_callbacks::CALLBACK_TYPE;
use crate::state::durable_steps::{
    CHAINED_INVOKE_TYPE, CONTEXT_TYPE, EXECUTION_TYPE, STEP_TYPE, WAIT_TYPE,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetDurableExecutionStateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetDurableExecutionStateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.lambda#GetDurableExecutionState")]
#[derive(Debug)]
pub enum GetDurableExecutionStateErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    KMSAccessDeniedException(KMSAccessDeniedException),
    KMSDisabledException(KMSDisabledException),
    KMSInvalidStateException(KMSInvalidStateException),
    KMSNotFoundException(KMSNotFoundException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_durable_execution_state::GetDurableExecutionStateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetDurableExecutionStateError = crate::scaffolded::Scaffolded<GetDurableExecutionStateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetDurableExecutionStateErrorModeled> for GetDurableExecutionStateError {
    fn from(modeled: GetDurableExecutionStateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDurableExecutionState` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetDurableExecutionState<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDurableExecutionState<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDurableExecutionState<Eng>
{
    type Input = GetDurableExecutionStateInput;
    type Output = GetDurableExecutionStateOutput;
    type Error = GetDurableExecutionStateError;

    const NAME: &'static str = "GetDurableExecutionState";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DURABLE_EXECUTION_STATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DURABLE_EXECUTION_STATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetDurableExecutionStateErrorModeled::ServiceException(
                ServiceException {
                    r#type: None,
                    message: Some("Lambda state is not reachable on this engine".to_string()),
                },
            )
            .into());
        };

        // "The maximum number of operations to return per call. [...] The default is 100 and
        // the maximum allowed is 1000. A value of 0 uses the default." The member is
        // `@range(min: 0, max: 1000)` with a default of 0, so the boundary validator has
        // already refused anything outside that range and a request naming no limit arrives
        // carrying the 0 that asks for the documented default.
        let page_size = match usize::try_from(input.max_items).unwrap_or(0) {
            0 => DEFAULT_PAGE_SIZE,
            requested => requested.min(MAX_PAGE_SIZE),
        };

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);

        // The payloads an operation carries are the execution's data, and reporting them
        // decrypts them under the key the function's `DurableConfig` names: so the key is
        // asked about at the point of use, with no store guard held, and a key the key service
        // reports as unusable refuses the read (see
        // [`crate::state::durable_data_key_fault`]).
        let data_key = {
            let mut store = state.lock();
            crate::state::resolve_durable_execution(
                &mut store,
                &input.durable_execution_arn,
                &region,
                now,
            )
            .ok()
            .and_then(|(record, name)| {
                record
                    .durable_executions
                    .get(&name)
                    .and_then(|execution| execution.kms_key_arn.clone())
            })
        };
        if let Some(fault) =
            crate::state::durable_data_key_fault(ctx.callback(), &region, data_key.as_deref())
                .await
        {
            return Err(kms_refusal(fault));
        }

        let mut store = state.lock();

        // The execution is addressed by the ARN `Invoke` answered, which carries the function,
        // the version and the execution's name and id; resolving it is also what brings the
        // record up to whatever the clock already made due, so an execution whose
        // `ExecutionTimeout` has fallen due reports the state that timeout owed. This
        // operation models no 404, so an ARN that names no execution of this emulator is
        // refused as the parameter value it is.
        let (record, name) = crate::state::resolve_durable_execution(
            &mut store,
            &input.durable_execution_arn,
            &region,
            now,
        )
        .map_err(|failure| match failure {
            crate::state::ResolveFailure::Malformed(message)
            | crate::state::ResolveFailure::NotFound(message) => invalid_parameter(message),
        })?;
        let record = &*record;
        let Some(execution) = record.durable_executions.get(&name) else {
            return Err(invalid_parameter(
                crate::state::durable_execution_not_found_message(&input.durable_execution_arn),
            ));
        };

        // The token "identifies the current state of the execution [...] and ensures that
        // state retrieval is consistent with the current execution context": a token that is
        // not the one currently issued for this execution identifies no state for this read to
        // be consistent with, so the value itself is what is wrong with the request. WHICH
        // token is current is the one reading this service makes of it
        // (`crate::state::DurableExecutionRecord::current_checkpoint_token`), the same one
        // `CheckpointDurableExecution` applies: the token minted when the execution started or
        // the one its last checkpoint rotated to, and NONE at all for an execution whose
        // history is closed — "the execution transitions to STOPPED status and cannot be
        // resumed", and a succeeded, failed or timed-out execution is just as final, so there
        // is no current checkpoint state for a replay to resume from. A client reading a
        // settled execution reads it through `GetDurableExecution` and
        // `GetDurableExecutionHistory`, which carry no token.
        if !execution.checkpoint_token_is_current(&input.checkpoint_token) {
            return Err(invalid_parameter(
                crate::state::INVALID_CHECKPOINT_TOKEN_MESSAGE.to_string(),
            ));
        }

        let operations = operations_of(execution);

        // "If NextMarker was returned from a previous request, use this value to retrieve the
        // next page of operations": the marker names the last operation the previous page
        // reported, so this page starts at the one ordered directly after it. A value that
        // names no operation of this execution names no place to continue from.
        let start = match input.marker.as_deref() {
            Some(marker) => {
                operations
                    .iter()
                    .position(|operation| operation.id == marker)
                    .ok_or_else(|| {
                        invalid_parameter(crate::state::INVALID_MARKER_MESSAGE.to_string())
                    })?
                    + 1
            }
            None => 0,
        };
        let page = match operations.get(start..) {
            Some(remaining) => &remaining[..page_size.min(remaining.len())],
            None => &[][..],
        };
        let truncated = start + page.len() < operations.len();

        Ok(GetDurableExecutionStateOutput {
            operations: page.to_vec(),
            // "If present, indicates that more operations are available. Use this value as
            // the Marker parameter in a subsequent request" — so it is absent on the last
            // page, and it names the last operation THIS page reported.
            next_marker: page
                .last()
                .filter(|_| truncated)
                .map(|operation| operation.id.clone()),
        })
    }
}

/// The operations this execution reports, in the order their first event took them:
/// "operations ordered by start sequence number in ascending order".
///
/// The history is the append-only record of what the run DID
/// ([`crate::state::durable_steps`]), one event per action taken on one operation, so the
/// operations are those events FOLDED by the operation each belongs to: the event that
/// started an operation gives its identity and its start instant, and the event that ended it
/// gives the status it came to, the instant it ended at and the result or error it carried.
fn operations_of(execution: &crate::state::DurableExecutionRecord) -> Vec<Operation> {
    let mut operations: Vec<Operation> = Vec::new();
    for event in &execution.history {
        let Some(update) = update_of(event, execution) else {
            continue;
        };
        let at = event.event_timestamp;
        match operations
            .iter_mut()
            .find(|operation| operation.id == update.id)
        {
            // A later event of an operation already reported moves that operation: its
            // status, the instant it ended at when this event ended it, and the details that
            // event carried.
            Some(operation) => {
                operation.status = update.status.to_string();
                if update.ends {
                    operation.end_timestamp = at;
                }
                merge_details(operation, update.details);
            }
            None => {
                let mut operation = Operation {
                    id: update.id,
                    parent_id: event.parent_id.clone(),
                    name: event.name.clone(),
                    r#type: update.r#type.to_string(),
                    sub_type: event.sub_type.clone(),
                    // Every operation's start is the instant of its own first event; an
                    // execution's events all carry one ([`crate::state::next_event`]).
                    start_timestamp: at.unwrap_or_else(|| {
                        ::aws_smithy_types::DateTime::from(execution.started)
                    }),
                    end_timestamp: update.ends.then_some(at).flatten(),
                    status: update.status.to_string(),
                    execution_details: None,
                    context_details: None,
                    step_details: None,
                    wait_details: None,
                    callback_details: None,
                    chained_invoke_details: None,
                };
                merge_details(&mut operation, update.details);
                operations.push(operation);
            }
        }
    }
    operations
}

/// What ONE history event says about the operation it belongs to.
struct Update {
    /// The operation's own id, which is what the events of one operation share.
    id: String,
    /// The `OperationType` it is of.
    r#type: &'static str,
    /// The `OperationStatus` this event leaves it in.
    status: &'static str,
    /// Whether this event ENDED the operation, which is what gives it an `EndTimestamp`.
    ends: bool,
    /// The details member of its own type this event carried, and `None` for an event that
    /// carried nothing to report beyond the status above.
    details: Option<Details>,
}

/// The details one event contributes to the operation it belongs to, one variant per
/// `Operation` details member the model declares.
enum Details {
    Execution(ExecutionDetails),
    Context(ContextDetails),
    Step(StepDetails),
    Wait(WaitDetails),
    Callback(CallbackDetails),
    ChainedInvoke(ChainedInvokeDetails),
}

/// Apply `details` to `operation`, each to the one member of its own type.
fn merge_details(operation: &mut Operation, details: Option<Details>) {
    match details {
        Some(Details::Execution(details)) => operation.execution_details = Some(details),
        Some(Details::Context(details)) => operation.context_details = Some(details),
        Some(Details::Step(details)) => operation.step_details = Some(details),
        Some(Details::Wait(details)) => operation.wait_details = Some(details),
        Some(Details::Callback(details)) => operation.callback_details = Some(details),
        Some(Details::ChainedInvoke(details)) => operation.chained_invoke_details = Some(details),
        None => {}
    }
}

/// What `event` says about its operation, and `None` for an event that is of no operation of
/// the execution — an `InvocationCompleted` event reports one INVOCATION of the function
/// rather than an operation of the execution, and an operation whose events carry no id at
/// all is named by no `Operation$Id`, which is required.
fn update_of(event: &Event, execution: &crate::state::DurableExecutionRecord) -> Option<Update> {
    // The EXECUTION's own operation is the execution itself, so it is identified by the
    // execution's own id rather than by an id a checkpoint named.
    let execution_update = |status: &'static str, ends: bool, details: Option<Details>| Update {
        id: execution.execution_id.clone(),
        r#type: EXECUTION_TYPE,
        status,
        ends,
        details,
    };
    let operation = |r#type: &'static str,
                     status: &'static str,
                     ends: bool,
                     details: Option<Details>| {
        event.id.clone().map(|id| Update {
            id,
            r#type,
            status,
            ends,
            details,
        })
    };
    if let Some(details) = event.execution_started_details.as_ref() {
        return Some(execution_update(
            STATUS_STARTED,
            false,
            Some(Details::Execution(ExecutionDetails {
                input_payload: details.input.payload.clone(),
            })),
        ));
    }
    if event.execution_succeeded_details.is_some() {
        return Some(execution_update(STATUS_SUCCEEDED, true, execution_input(execution)));
    }
    if event.execution_failed_details.is_some() {
        return Some(execution_update(STATUS_FAILED, true, execution_input(execution)));
    }
    if event.execution_timed_out_details.is_some() {
        return Some(execution_update(STATUS_TIMED_OUT, true, execution_input(execution)));
    }
    if event.execution_stopped_details.is_some() {
        return Some(execution_update(STATUS_STOPPED, true, execution_input(execution)));
    }
    if event.context_started_details.is_some() {
        return operation(
            CONTEXT_TYPE,
            STATUS_STARTED,
            false,
            // "Whether the children of this context need to be replayed": a context that is
            // still running has its children replayed with it, which is what a replay of a
            // context in flight has to do.
            Some(Details::Context(ContextDetails {
                replay_children: Some(true),
                result: None,
                error: None,
            })),
        );
    }
    if let Some(details) = event.context_succeeded_details.as_ref() {
        return operation(
            CONTEXT_TYPE,
            STATUS_SUCCEEDED,
            true,
            Some(Details::Context(ContextDetails {
                // A context that COMPLETED is replayed from its own result: its children "do
                // not need to be replayed", which is what the model's own documentation of
                // this operation states for a completed operation with children.
                replay_children: Some(false),
                result: details.result.payload.clone(),
                error: None,
            })),
        );
    }
    if let Some(details) = event.context_failed_details.as_ref() {
        return operation(
            CONTEXT_TYPE,
            STATUS_FAILED,
            true,
            Some(Details::Context(ContextDetails {
                replay_children: Some(false),
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    if let Some(details) = event.wait_started_details.as_ref() {
        return operation(
            WAIT_TYPE,
            STATUS_STARTED,
            false,
            Some(Details::Wait(WaitDetails {
                scheduled_end_timestamp: Some(details.scheduled_end_timestamp),
            })),
        );
    }
    if event.wait_succeeded_details.is_some() {
        return operation(WAIT_TYPE, STATUS_SUCCEEDED, true, None);
    }
    if event.wait_cancelled_details.is_some() {
        return operation(WAIT_TYPE, STATUS_CANCELLED, true, None);
    }
    if event.step_started_details.is_some() {
        return operation(
            STEP_TYPE,
            STATUS_STARTED,
            false,
            Some(Details::Step(StepDetails::default())),
        );
    }
    if let Some(details) = event.step_succeeded_details.as_ref() {
        return operation(
            STEP_TYPE,
            STATUS_SUCCEEDED,
            true,
            Some(Details::Step(StepDetails {
                attempt: details.retry_details.current_attempt,
                next_attempt_timestamp: None,
                result: details.result.payload.clone(),
                error: None,
            })),
        );
    }
    if let Some(details) = event.step_failed_details.as_ref() {
        // A failed attempt with another to come leaves the step waiting for that attempt —
        // `PENDING`, at the instant `RetryDetails$NextAttemptDelaySeconds` after this one —
        // and a failure with no attempt left is the step having FAILED.
        let next = details
            .retry_details
            .next_attempt_delay_seconds
            .filter(|seconds| *seconds >= 0)
            .and_then(|seconds| next_attempt_at(event, seconds));
        return operation(
            STEP_TYPE,
            if next.is_some() {
                STATUS_PENDING
            } else {
                STATUS_FAILED
            },
            next.is_none(),
            Some(Details::Step(StepDetails {
                attempt: details.retry_details.current_attempt,
                next_attempt_timestamp: next,
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    if let Some(details) = event.callback_started_details.as_ref() {
        return operation(
            CALLBACK_TYPE,
            STATUS_STARTED,
            false,
            Some(Details::Callback(CallbackDetails {
                callback_id: Some(details.callback_id.clone()),
                result: None,
                error: None,
            })),
        );
    }
    if let Some(details) = event.callback_succeeded_details.as_ref() {
        return operation(
            CALLBACK_TYPE,
            STATUS_SUCCEEDED,
            true,
            Some(Details::Callback(CallbackDetails {
                callback_id: callback_id_of(execution, event.id.as_deref()),
                result: details.result.payload.clone(),
                error: None,
            })),
        );
    }
    if let Some(details) = event.callback_failed_details.as_ref() {
        return operation(
            CALLBACK_TYPE,
            STATUS_FAILED,
            true,
            Some(Details::Callback(CallbackDetails {
                callback_id: callback_id_of(execution, event.id.as_deref()),
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    if let Some(details) = event.callback_timed_out_details.as_ref() {
        return operation(
            CALLBACK_TYPE,
            STATUS_TIMED_OUT,
            true,
            Some(Details::Callback(CallbackDetails {
                callback_id: callback_id_of(execution, event.id.as_deref()),
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    if event.chained_invoke_started_details.is_some() {
        return operation(
            CHAINED_INVOKE_TYPE,
            STATUS_STARTED,
            false,
            Some(Details::ChainedInvoke(ChainedInvokeDetails::default())),
        );
    }
    if let Some(details) = event.chained_invoke_succeeded_details.as_ref() {
        return operation(
            CHAINED_INVOKE_TYPE,
            STATUS_SUCCEEDED,
            true,
            Some(Details::ChainedInvoke(ChainedInvokeDetails {
                result: details.result.payload.clone(),
                error: None,
            })),
        );
    }
    if let Some(details) = event.chained_invoke_failed_details.as_ref() {
        return operation(
            CHAINED_INVOKE_TYPE,
            STATUS_FAILED,
            true,
            Some(Details::ChainedInvoke(ChainedInvokeDetails {
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    if let Some(details) = event.chained_invoke_timed_out_details.as_ref() {
        return operation(
            CHAINED_INVOKE_TYPE,
            STATUS_TIMED_OUT,
            true,
            Some(Details::ChainedInvoke(ChainedInvokeDetails {
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    if let Some(details) = event.chained_invoke_stopped_details.as_ref() {
        return operation(
            CHAINED_INVOKE_TYPE,
            STATUS_STOPPED,
            true,
            Some(Details::ChainedInvoke(ChainedInvokeDetails {
                result: None,
                error: details.error.payload.clone(),
            })),
        );
    }
    None
}

/// The `ExecutionDetails` an event that ENDED the execution reports: the input the execution
/// started from, which is the only member the shape carries — what it came to is the
/// execution's own result and error (`GetDurableExecution$Result`, `$Error`) rather than a
/// detail of this operation.
///
/// The payload is reported as text it really is: the model types `InputPayload` as a string,
/// so a payload that is not text is reported as none at all — the same reading
/// `GetDurableExecution` gives the stored payload.
fn execution_input(execution: &crate::state::DurableExecutionRecord) -> Option<Details> {
    Some(Details::Execution(ExecutionDetails {
        input_payload: String::from_utf8(execution.input_payload.clone())
            .ok()
            .filter(|payload| !payload.is_empty()),
    }))
}

/// The `CallbackId` of the callback operation `id` names, read off the event that STARTED it:
/// the id is minted when the callback begins ([`crate::state::durable_callbacks`]) and the
/// events that end it carry the result or the error alone.
fn callback_id_of(
    execution: &crate::state::DurableExecutionRecord,
    id: Option<&str>,
) -> Option<String> {
    let id = id?;
    execution
        .history
        .iter()
        .filter(|event| event.id.as_deref() == Some(id))
        .find_map(|event| {
            event
                .callback_started_details
                .as_ref()
                .map(|details| details.callback_id.clone())
        })
}

/// When the attempt that follows a failed one is due: `seconds` after the instant the failure
/// happened at, which is what `RetryDetails$NextAttemptDelaySeconds` states.
fn next_attempt_at(event: &Event, seconds: i32) -> Option<::aws_smithy_types::DateTime> {
    let at = event.event_timestamp?;
    let seconds = f64::from(seconds);
    Some(::aws_smithy_types::DateTime::from_secs_f64(
        at.as_secs_f64() + seconds,
    ))
}

/// The `OperationStatus` values this fold reports, from the model's own enum
/// (`smithy_query(query="shape", shape="OperationStatus")`: `STARTED | PENDING | READY |
/// SUCCEEDED | FAILED | CANCELLED | TIMED_OUT | STOPPED`).
const STATUS_STARTED: &str = "STARTED";
/// See [`STATUS_STARTED`]: a step waiting for the attempt that follows a failed one.
const STATUS_PENDING: &str = "PENDING";
/// See [`STATUS_STARTED`].
const STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// See [`STATUS_STARTED`].
const STATUS_FAILED: &str = "FAILED";
/// See [`STATUS_STARTED`].
const STATUS_CANCELLED: &str = "CANCELLED";
/// See [`STATUS_STARTED`].
const STATUS_TIMED_OUT: &str = "TIMED_OUT";
/// See [`STATUS_STARTED`].
const STATUS_STOPPED: &str = "STOPPED";

/// The operations reported to a caller that names no `MaxItems`, which the member's own
/// documentation states: "The default is 100 and the maximum allowed is 1000."
const DEFAULT_PAGE_SIZE: usize = 100;

/// The most operations one page reports, from the same sentence and from the member's own
/// `@range(min: 0, max: 1000)`.
const MAX_PAGE_SIZE: usize = 1_000;

/// The modelled 400 this operation refuses a request it cannot satisfy with.
fn invalid_parameter(message: String) -> GetDurableExecutionStateError {
    GetDurableExecutionStateErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            r#type: None,
            message: Some(message),
        },
    )
    .into()
}

/// How a key that cannot decrypt a durable execution's data refuses this READ: the modelled
/// 502 for the fault the key service reported, carrying that fault's own sentence.
fn kms_refusal(fault: crate::state::EnvironmentKeyFault) -> GetDurableExecutionStateError {
    match fault {
        crate::state::EnvironmentKeyFault::Disabled(message) => {
            GetDurableExecutionStateErrorModeled::KMSDisabledException(KMSDisabledException {
                r#type: None,
                message: Some(message),
            })
        }
        crate::state::EnvironmentKeyFault::InvalidState(message) => {
            GetDurableExecutionStateErrorModeled::KMSInvalidStateException(
                KMSInvalidStateException {
                    r#type: None,
                    message: Some(message),
                },
            )
        }
        crate::state::EnvironmentKeyFault::NotFound(message) => {
            GetDurableExecutionStateErrorModeled::KMSNotFoundException(KMSNotFoundException {
                r#type: None,
                message: Some(message),
            })
        }
        crate::state::EnvironmentKeyFault::AccessDenied(message) => {
            GetDurableExecutionStateErrorModeled::KMSAccessDeniedException(
                KMSAccessDeniedException {
                    r#type: None,
                    message: Some(message),
                },
            )
        }
    }
    .into()
}
