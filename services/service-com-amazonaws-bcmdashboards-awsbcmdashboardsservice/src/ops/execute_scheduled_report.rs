// SPDX-License-Identifier: CC0-1.0

//! @op ExecuteScheduledReport
//!
//! Triggers one immediate execution of the stored scheduled report the requested ARN is
//! the key of, outside its schedule, and answers whether that execution was triggered
//! together with the report's health after the request.
//!
//! The decisions, each taken from this service's persisted state rather than from the
//! request:
//!
//! * An ARN no report is stored under is refused with the service's not-found answer
//!   (`crate::scaffolded::Scaffolded::resource_not_found`, HTTP 400 with the error code
//!   `ResourceNotFoundException` and the requested ARN in `resourceId`), exactly as
//!   `GetScheduledReport` refuses one.
//! * A report whose `scheduleConfig.state` is `DISABLED` is refused with the modelled
//!   `ValidationException` (400): the model documents "The scheduled report must be in
//!   ENABLED state. Calling this operation on a DISABLED scheduled report returns a
//!   ValidationException."
//! * `healthStatus` is DERIVED from the stores and from the DELIVERY this execution makes
//!   — `crate::state::ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::scheduled_report_health`
//!   answers healthy exactly while the dashboard the report renders and every widget its
//!   `widgetIds` name still resolve AND the account's IAM still lets this service assume the
//!   role the report executes as and do the execution's work under it
//!   (`crate::state::execution_role`), and the execution is
//!   triggered only when the configuration does resolve, so `executionTriggered` is false on
//!   a report whose configuration does not and the report's `lastExecutionAt` does not move. A triggered
//!   execution then hands one notification to the account's default Amazon EventBridge
//!   event bus (`crate::state::deliver_execution_notification`), because that delivery is
//!   what the model defines a scheduled report by: `HealthStatus.statusCode` documents
//!   `UNHEALTHY` as "the scheduled report is unable to deliver the notification to the
//!   default Amazon Web Services EventBridge EventBus in your account". A notification
//!   the bus did not take is therefore answered as `UNHEALTHY` with `INTERNAL_FAILURE`
//!   (`crate::state::undelivered_notification_health`) rather than as a healthy
//!   execution; the execution was still triggered, so `executionTriggered` stays true and
//!   `lastExecutionAt` moves.
//! * `dryRun` is a validation only: the ARN and the schedule state are still checked and
//!   the health the configuration establishes is still returned, but nothing is written,
//!   no execution is triggered and no notification is delivered, so `executionTriggered`
//!   is false.
//! * A `clientToken` makes the call idempotent for
//!   `crate::state::EXECUTION_IDEMPOTENCY_WINDOW` (the minute the model documents): a
//!   retry presenting the same token for the same report answers the first call's
//!   outcome and triggers nothing. The token is RESERVED under the same guard that reads
//!   it (`crate::state::ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState::reserve_execution_token`),
//!   before the delivery below, so a duplicate still IN FLIGHT triggers no second
//!   execution either: several calls presenting one token for one report produce exactly
//!   one execution and one notification however they overlap. A duplicate that arrives
//!   while the execution is still reaching the bus is answered the outcome that execution
//!   was already committed to when it reserved the token, which is all that is known of
//!   it at that instant; the record is then settled with the health the delivery
//!   established, so every later retry replays the outcome the execution ended with.
//!   The token is scoped to the request it was presented
//!   with — the model's only documented effect is that "requests with the same client
//!   token will not trigger a new execution within the same minute" — so the same token
//!   presented for a DIFFERENT report is a different request: it triggers that report's
//!   execution and answers that report's health.
//!
//! A triggered execution writes two members of the stored record — `lastExecutionAt`
//! and `healthStatus` — which `GetScheduledReport` and `ListScheduledReports` then read
//! back; the configuration itself, `updatedAt` included, is untouched, because an
//! execution does not update the report. Every call that changes the store (the
//! execution, and the refusals that are state-change attempts) emits the audit record
//! `crate::state::record_state_change` writes; the dry run and the idempotent retry
//! change nothing, so they emit none — the same rule this service's reporting
//! operations follow.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ExecuteScheduledReport::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `ExecuteScheduledReport`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json")
//! ```
//!
//! Triggers an immediate execution of a scheduled report, outside of its regular schedule.
//! The scheduled report must be in ENABLED state. Calling this operation on a DISABLED
//! scheduled report returns a ValidationException. If a clientToken is provided, the
//! service uses it for idempotency. Requests with the same client token will not trigger a
//! new execution within the same minute.
//!
//! ## Input
//!
//! * `arn` (ScheduledReportArn, len 20..2048, pattern
//!   ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$)
//!   **required** — The ARN of the scheduled report to execute.
//! * `clientToken` (ClientToken, len 1..128, pattern ^[\u0021-\u007E]+$, @idempotencyToken) —
//!   A unique, case-sensitive identifier that you provide to ensure the idempotency of the
//!   request.
//! * `dryRun` (Boolean) — When set to true, validates the scheduled report configuration
//!   without triggering an actual execution.
//!
//! ## Output
//!
//! * `executionTriggered` (Boolean) — Indicates whether the execution was successfully
//!   triggered.
//! * `healthStatus` (HealthStatus) — The health status of the scheduled report after the
//!   execution request.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permissions to perform
//!   this action. Verify your IAM permissions and any resource policies.
//! * `ConflictException` (HTTP 409) — The request could not be completed due to a conflict
//!   with the current state of the resource. For example, attempting to create a resource
//!   that already exists or is being created.
//! * `InternalServerException` (HTTP 500) — An internal error occurred while processing the
//!   request. Retry your request. If the problem persists, contact Amazon Web Services
//!   Support.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource (dashboard, policy, or
//!   widget) was not found. Verify the ARN and try again.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//!   Reduce the frequency of requests and use exponential backoff.
//! * `ValidationException` (HTTP 400) — The input parameters do not satisfy the requirements.
//!   Check the error message for specific validation details.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ExecuteScheduledReportError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ExecuteScheduledReportError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bcmdashboards#ExecuteScheduledReport")]
#[derive(Debug)]
pub enum ExecuteScheduledReportErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::execute_scheduled_report::ExecuteScheduledReportError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ExecuteScheduledReportError = crate::scaffolded::Scaffolded<ExecuteScheduledReportErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ExecuteScheduledReportErrorModeled> for ExecuteScheduledReportError {
    fn from(modeled: ExecuteScheduledReportErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ExecuteScheduledReport` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ExecuteScheduledReport<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ExecuteScheduledReport<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ExecuteScheduledReport<Eng>
{
    type Input = ExecuteScheduledReportInput;
    type Output = ExecuteScheduledReportOutput;
    type Error = ExecuteScheduledReportError;

    const NAME: &'static str = "ExecuteScheduledReport";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXECUTE_SCHEDULED_REPORT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXECUTE_SCHEDULED_REPORT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ExecuteScheduledReportErrorModeled::InternalServerException(InternalServerException {
                message: "the dashboard store is unavailable".to_string(),
            })
        })?;
        let now = ::std::time::SystemTime::now();

        // What the account's IAM shows about this account's execution roles, resolved
        // BEFORE this service's store is read so no two stores are held at once. A report
        // whose execution role no longer admits this service, or no longer lets it do the
        // execution's work, has nothing it could execute as, which is what the health below
        // reports.
        let role_faults = crate::state::ExecutionRoleFaults::resolve(&*self.engine, ctx);

        // The idempotency key of THIS request: the report it names together with the
        // token it presented. A token suppresses a repeat of the request that carried it,
        // so the same token on another report is looked up under another key and decided
        // on its own merits rather than answered with — or refused because of — the first
        // report's execution.
        let idempotency_key = input
            .client_token
            .as_ref()
            .map(|token| (input.arn.clone(), token.clone()));

        // Everything this request can decide from the store alone is decided here, under
        // one guard that is DROPPED before the delivery below: the delivery is an await,
        // and a lock is not held across one.
        let (record, configured_health, execution_triggered) = {
            let mut store = state.lock();

            // The report the ARN is the key of. Cloned so the health below can read the
            // rest of the store (the dashboard the report renders) while the record is in
            // hand.
            let Some(record) = store.scheduled_reports.get(&input.arn).cloned() else {
                crate::state::record_state_change(
                    ctx,
                    Self::NAME,
                    &input.arn,
                    crate::scaffolded::RESOURCE_NOT_FOUND_CODE,
                );
                return Err(crate::scaffolded::Scaffolded::resource_not_found(
                    &input.arn,
                    format!("Scheduled report not found: {}", input.arn),
                ));
            };

            // "The scheduled report must be in ENABLED state. Calling this operation on a
            // DISABLED scheduled report returns a ValidationException."
            if record.schedule_state() != crate::state::SCHEDULE_STATE_ENABLED {
                crate::state::record_state_change(
                    ctx,
                    Self::NAME,
                    &input.arn,
                    "ValidationException",
                );
                return Err(ExecuteScheduledReportErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "Member must be in {} state: scheduled report {}",
                            crate::state::SCHEDULE_STATE_ENABLED,
                            input.arn
                        ),
                    },
                )
                .into());
            }

            // "When set to true, validates the scheduled report configuration without
            // triggering an actual execution": a reporting path. No execution is
            // triggered, so no notification is delivered and the health answered is the
            // one the configuration establishes. It writes NOTHING — not the health
            // refresh, not the idempotency record — so a dry run leaves every later
            // caller seeing exactly what it would have seen without it — and, changing
            // nothing, it accounts for nothing in the audit stream, as this service's
            // reporting operations do.
            if input.dry_run == Some(true) {
                return Ok(ExecuteScheduledReportOutput {
                    health_status: Some(store.scheduled_report_health(
                        &record,
                        &role_faults,
                        now,
                    )),
                    execution_triggered: Some(false),
                });
            }

            // The idempotency records the clock has already finished with, dropped before
            // any of them can be read as a retry.
            store.expire_execution_tokens(now);

            // The health the stored configuration establishes for this report right now,
            // and with it whether the execution could be triggered at all: a report whose
            // dashboard or widgets no longer resolve has nothing to render, so nothing is
            // triggered and nothing is delivered.
            let configured_health = store.scheduled_report_health(&record, &role_faults, now);
            let execution_triggered =
                configured_health.status_code == crate::state::HEALTH_STATUS_HEALTHY;

            // The token is looked up and, when it is free, RESERVED here — one operation
            // under this one guard, and not two straddling the delivery below. The guard
            // is dropped across that delivery, so a token recorded only once the bus had
            // answered would leave a window in which a duplicate presenting it finds
            // nothing recorded, triggers its own execution and publishes its own
            // notification: one token, two executions. Reserving it here is how
            // `CreateScheduledReport` resolves its own racing duplicates, and it makes
            // several calls presenting one token for one report produce exactly one
            // execution and one notification however they overlap.
            //
            // What is reserved is the outcome this call is already committed to: whether
            // it triggers an execution, and the health its stored configuration
            // establishes — all the delivery can still do is refine that health, which it
            // does once it is known (`settle_execution_token` below).
            if let Some(reserved) = idempotency_key.clone().and_then(|key| {
                store.reserve_execution_token(
                    key,
                    crate::state::ScheduledReportExecution {
                        executed_at: now,
                        execution_triggered,
                        health_status: configured_health.clone(),
                    },
                )
            }) {
                // A duplicate inside the documented window: the outcome of the call that
                // holds the token, and no second execution — so no second notification
                // either. It triggers nothing and changes nothing a later caller sees, so
                // — like the dry run — it emits no audit record; the execution it replays
                // already emitted one.
                return Ok(ExecuteScheduledReportOutput {
                    health_status: Some(reserved.health_status),
                    execution_triggered: Some(reserved.execution_triggered),
                });
            }

            (record, configured_health, execution_triggered)
        };

        // The execution itself: the notification the model defines a scheduled report by,
        // handed to the account's default EventBridge event bus. The health this request
        // establishes is the outcome of that delivery — `UNHEALTHY` is documented as the
        // report "unable to deliver the notification to the default Amazon EventBridge
        // EventBus in your account" — so a delivery nothing accepted is reported as such
        // instead of being presented as a healthy execution.
        let health_status = if execution_triggered
            && !crate::state::deliver_execution_notification(ctx, &record, now).await
        {
            crate::state::undelivered_notification_health(now)
        } else {
            configured_health
        };

        // The writes, under a guard taken AFTER the delivery: the store was unlocked
        // while the bus was reached, so the report is looked up again rather than
        // assumed to still be there — a `DeleteScheduledReport` in between removes it,
        // and this execution must not write it back.
        let mut store = state.lock();
        if let Some(stored) = store.scheduled_reports.get_mut(&input.arn) {
            // The health this request refreshed, which the readers project from here on.
            stored.health_status = Some(health_status.clone());
            if execution_triggered {
                stored.last_execution_at = Some(now);
            }
            // `updated_at` is deliberately untouched: an execution does not update the
            // report's configuration.
        }

        if let Some(key) = idempotency_key {
            // The reservation this call took before the delivery, settled with the health
            // that delivery established: a retry presenting the token from here on
            // replays the outcome this call ended with rather than the one it was
            // committed to beforehand.
            store.settle_execution_token(&key, now, &health_status);
        }

        // The record this call changed state, for the audit stream.
        crate::state::record_state_change(
            ctx,
            Self::NAME,
            &input.arn,
            if execution_triggered {
                "SUCCESS"
            } else {
                // An audit outcome, not a health code: the request was answered, and
                // the health it refreshed says why nothing was triggered.
                "NOT_TRIGGERED"
            },
        );

        Ok(ExecuteScheduledReportOutput {
            health_status: Some(health_status),
            execution_triggered: Some(execution_triggered),
        })
    }
}
