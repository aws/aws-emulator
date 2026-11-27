// SPDX-License-Identifier: CC0-1.0

//! @op UpdatePlan
//!
//! Writes the plan's mutable configuration from the request and answers with the plan
//! read back from the store, so what the caller sees is what a later `GetPlan` reads.
//!
//! Every member a Region switch plan lets a caller change is a member of this request
//! (`description`, `workflows`, `executionRole`, `recoveryTimeObjectiveMinutes`,
//! `associatedAlarms`, `triggers`, `reportConfiguration`, `serviceQuotaChecksEnabled` —
//! each `@cfnMutability: full`), so the request carries the plan's whole mutable
//! configuration and the update REPLACES it: an omitted optional member leaves the plan
//! without that configuration rather than keeping what was there before. The plan's
//! identity and topology (`arn`, `name`, `regions`, `recoveryApproach`, `primaryRegion`,
//! `owner`) is not a member of this request and is left untouched.
//!
//! The server-owned `version` and `updatedAt` move with the write: `version` is derived
//! from the record's revision (`crate::state::StoredPlan::version`), which this handler
//! bumps once per accepted update, and `updatedAt` becomes the instant of the write.
//!
//! ## The modelled conflict
//!
//! The model also declares `RetryableConflictException` (HTTP 409) for this operation —
//! "the request conflicted with a concurrent modification of the same resource and did
//! not take effect" — so the error channel below carries it. It is the refusal a write
//! gets when another modification of the same plan is applied between the point this one
//! read the record and the point it would have written it. This handler leaves no such
//! window: the whole read-modify-write runs under ONE store guard, so each accepted
//! update becomes exactly one revision of the record, and a plan can never end up
//! carrying one caller's workflows beside another caller's execution role. Nothing is
//! refused for a conflict it cannot have had.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read and write it through the
//! guard it hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries
//! request metadata only; the engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdatePlan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Updates an existing Region switch plan. You can modify the plan's description,
//! workflows, execution role, recovery time objective, associated alarms, and triggers.
//!
//! ## Input
//!
//! * `arn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan.
//! * `associatedAlarms` (AssociatedAlarmMap) — The updated CloudWatch alarms associated with
//!   the plan.
//! * `description` (String) — The updated description for the Region switch plan.
//! * `executionRole` (IamRoleArn, pattern ^arn:aws[a-zA-Z0-9-]*:iam::[0-9]{12}:role/.+$)
//!   **required** — The updated IAM role ARN that grants Region switch the permissions needed
//!   to execute the plan steps.
//! * `recoveryTimeObjectiveMinutes` (Integer) — The updated target recovery time objective
//!   (RTO) in minutes for the plan.
//! * `reportConfiguration` (ReportConfiguration) — The updated report configuration for the
//!   plan.
//! * `serviceQuotaChecksEnabled` (Boolean) — Specifies whether service quota checks are
//!   enabled for the Region switch plan.
//! * `triggers` (TriggerList) — The updated conditions that can automatically trigger the
//!   execution of the plan.
//! * `workflows` (WorkflowList) **required** — The updated workflows for the Region switch
//!   plan.
//!
//! ## Output
//!
//! * `plan` (Plan) — The details of the updated Region switch plan.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404
//! * `RetryableConflictException` (HTTP 409) — The request conflicted with a concurrent
//!   modification of the same resource and did not take effect.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdatePlanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdatePlanError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `InternalServerException` — a shape the SERVICE models, which this operation does not
// list — is widened in through `extra_errors`, so an unreachable shared store has a
// modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#UpdatePlan",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum UpdatePlanErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    // The write-conflict refusal the model declares for this operation; see "The modelled
    // conflict" above for the window it reports and why an update applied under one store
    // guard has none.
    RetryableConflictException(RetryableConflictException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_plan::UpdatePlanError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdatePlanError = crate::scaffolded::Scaffolded<UpdatePlanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdatePlanErrorModeled> for UpdatePlanError {
    fn from(modeled: UpdatePlanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdatePlan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdatePlan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdatePlan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdatePlan<Eng>
{
    type Input = UpdatePlanInput;
    type Output = UpdatePlanOutput;
    type Error = UpdatePlanError;

    const NAME: &'static str = "UpdatePlan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_PLAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_PLAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdatePlanErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        // Like every handler of this service: materialise the state changes the clock
        // already made due before reading the store, so no two handlers disagree about
        // what the current state is.
        crate::state::settle_due(&state, ctx.callback(), now).await;
        let mut store = state.lock();

        let Some(record) = store.plans.get_mut(&input.arn) else {
            return Err(UpdatePlanErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // The crate's single plan-not-found sentence, so every operation that
                    // resolves a plan by ARN words that refusal identically.
                    message: crate::state::plan_not_found_message(&input.arn),
                },
            )
            .into());
        };

        // The request carries the plan's whole mutable configuration, so it REPLACES it;
        // an omitted optional member clears that configuration. Nothing outside this set
        // is touched — the plan's ARN, name, regions, recovery approach, primary Region
        // and owner are not updatable members of this operation.
        record.plan.description = input.description;
        record.plan.workflows = input.workflows;
        record.plan.execution_role = input.execution_role;
        record.plan.recovery_time_objective_minutes = input.recovery_time_objective_minutes;
        record.plan.associated_alarms = input.associated_alarms;
        record.plan.triggers = input.triggers;
        record.plan.report_configuration = input.report_configuration;
        record.plan.service_quota_checks_enabled = input.service_quota_checks_enabled;
        // The members the service materialises on every write: an update that supplies no
        // alarms, no triggers and nothing about quota checks leaves the plan carrying all
        // three, exactly as creating one does.
        crate::state::materialise_plan_members(&mut record.plan);
        // Server-owned: the plan has been written once more, and this is when. `version`
        // is derived from the revision by `StoredPlan::as_plan`.
        record.revision += 1;
        record.plan.updated_at = Some(crate::state::reported_instant(now));

        // Project the response from the STORED record, so the caller sees exactly what a
        // later `GetPlan` will read back.
        Ok(UpdatePlanOutput {
            plan: Some(record.as_plan()),
        })
    }
}
