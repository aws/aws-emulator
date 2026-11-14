// SPDX-License-Identifier: CC0-1.0

//! @op GenerateCredentialReport
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming a modelled condition.
//! The input, output, error enum and schemas a generated stub would have carried are
//! declared in `src/ops/types/generate_credential_report_types.rs` (the layout every other
//! operation of this service follows) and reached through `super::types::*`; `build.rs`
//! picks the operation up from the `@op` line above and wires it into the service's
//! routing table.
//!
//! ## What this operation answers on the wire
//!
//! A mutation: it "generates a credential report for the Amazon Web Services account",
//! which is work that is STARTED rather than done inside the call — the operation answers
//! the `State` of that generation, one of the three values `ReportStateType` declares
//! ([`super::types::REPORT_STATE_TYPE_VALUES`]), beside a `Description` of it.
//!
//! An account has ONE credential report, so what a call does is decided by the generation
//! the account already holds ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::credential_report`])
//! and by the clock:
//!
//! * no generation on record: one is started, and the call reports it [`STATE_STARTED`];
//! * a generation not yet due: nothing is started and the call reports it
//!   [`STATE_INPROGRESS`] — the span is stored rather than slept for, so a caller that
//!   calls again straight away still sees the generation in flight;
//! * a generation that has become due and whose completion no call has reported yet: the
//!   call reports it [`STATE_COMPLETE`], which is the transition the CLOCK already made
//!   due being materialised rather than new work — while the report it produced is still
//!   readable ([`crate::state::CREDENTIAL_REPORT_VALIDITY_SECONDS`]), since a completion
//!   the account can no longer read is not what a caller asking for a report needs;
//! * a report the account already has, asked for again before the published rate allows a
//!   new one ("you can generate a credential report as often as once every four hours",
//!   [`crate::state::CREDENTIAL_REPORT_REGENERATION_SECONDS`]): nothing is started, and
//!   the call reports the generation the account holds [`STATE_COMPLETE`] — the report it
//!   produced is still the account's;
//! * from the end of that window: a new generation is started, so the account can get a
//!   fresh report, and the call reports it [`STATE_STARTED`] again.
//!
//! A call that STARTS a generation is also what renders the report that generation
//! produces ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::credential_report_csv`]):
//! a credential report is a snapshot of the account's credentials, so it is made once and
//! kept on the record, and `GetCredentialReport` serves those same bytes from the instant
//! the generation is due until the report's window ends.
//!
//! A caller whose identity-based policies do not permit the call is refused before
//! anything is recorded, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]), so a request that was not allowed starts
//! no generation.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GenerateCredentialReport`
//!
//! Generates a credential report for the Amazon Web Services account. For more information
//! about the credential report, see Getting credential reports in the IAM User Guide.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `State` (ReportStateType, one of: STARTED | INPROGRESS | COMPLETE) — Information about
//!   the state of the credential report.
//! * `Description` (ReportStateDescriptionType) — Information about the credential report.
//!
//! ## Errors
//!
//! * `LimitExceededException` (HTTP 409) — The request was rejected because it attempted to
//!   create resources beyond the current Amazon Web Services account limits. The error
//!   message describes the limit exceeded.
//! * `ServiceFailureException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

/// The `ReportStateType` value of a generation this call has just started: `STARTED`.
///
/// One of the three values the enum declares
/// ([`super::types::REPORT_STATE_TYPE_VALUES`]).
const STATE_STARTED: &str = "STARTED";

/// The `ReportStateType` value of a generation that is running: `INPROGRESS`. See
/// [`STATE_STARTED`].
const STATE_INPROGRESS: &str = "INPROGRESS";

/// The `ReportStateType` value of a generation that has finished: `COMPLETE`. See
/// [`STATE_STARTED`].
const STATE_COMPLETE: &str = "COMPLETE";

/// "Information about the credential report" (`GenerateCredentialReportResponse$Description`)
/// for the call that starts a generation because the account has no report: the sentence
/// the service answers that call with, as it was recorded.
const DESCRIPTION_STARTED: &str = "No report exists. Starting a new report generation task";

/// The description of a generation that is still running. It states the state the response
/// reports, the way [`DESCRIPTION_STARTED`] states its own; `ReportStateDescriptionType`
/// declares no values, so what the member carries is a sentence rather than a token.
const DESCRIPTION_INPROGRESS: &str = "Report generation is in progress";

/// The description of a generation that has finished, so the account now has a report. See
/// [`DESCRIPTION_INPROGRESS`].
const DESCRIPTION_COMPLETE: &str = "Report has been generated";

/// The description of a generation started for an account that ALREADY has a report, which
/// the published rate now allows another of: it states the task that was started, and not
/// that no report exists — which is what [`DESCRIPTION_STARTED`] says, and is true only of
/// an account that has never had one.
const DESCRIPTION_RESTARTED: &str = "Starting a new report generation task";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GenerateCredentialReportError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#GenerateCredentialReport")]
#[derive(Debug)]
pub enum GenerateCredentialReportErrorModeled {
    LimitExceededException(LimitExceededException),
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::generate_credential_report::GenerateCredentialReportError`).
pub type GenerateCredentialReportError =
    crate::scaffolded::Scaffolded<GenerateCredentialReportErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GenerateCredentialReportErrorModeled> for GenerateCredentialReportError {
    fn from(modeled: GenerateCredentialReportErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GenerateCredentialReport` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GenerateCredentialReport<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GenerateCredentialReport<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GenerateCredentialReport<Eng>
{
    type Input = GenerateCredentialReportInput;
    type Output = GenerateCredentialReportOutput;
    type Error = GenerateCredentialReportError;

    const NAME: &'static str = "GenerateCredentialReport";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GENERATE_CREDENTIAL_REPORT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GENERATE_CREDENTIAL_REPORT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it answers comes from the account's own
        // record of its credential report and from the clock.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GenerateCredentialReportErrorModeled::ServiceFailureException(
                ServiceFailureException {
                    message: Some("the IAM store is unavailable".to_string()),
                },
            )
            .into());
        };

        // What the account recorded about each of its access keys being used, read BEFORE
        // the store is locked, because it lives in another store — the account's event
        // history — and the report this call may render carries it. A record of the
        // account's calls this process cannot reach is not the same fact as a key nothing
        // was recorded against, so it is reported as the modelled server failure, exactly
        // as an unreachable IAM store is above.
        let Some(key_uses) = crate::service::management_events::last_use_by_access_key(&*self.engine)
        else {
            return Err(GenerateCredentialReportErrorModeled::ServiceFailureException(
                ServiceFailureException {
                    message: Some("the account's management event log is unavailable".to_string()),
                },
            )
            .into());
        };

        // A mutation: the operation generates the account's credential report, so the
        // lock is held for write across the decision and the write — two calls cannot
        // both start a generation for one account.
        let mut store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is recorded. The report is the ACCOUNT's, not one
        // entity's, and the Service Authorization Reference lists no resource type for
        // the action, so the decision is made against the account-wide resource — exactly
        // as the other account-wide operations of this service decide.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::GENERATE_CREDENTIAL_REPORT,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        let now = crate::state::now();
        let caller_akid = ::engine::extract_akid(&ctx.parts().headers);

        // The generation the account already holds decides what this call does. Each arm
        // below is a different fact about that record, never about who asked.
        let (report_state, description) = match store.credential_report.as_mut() {
            // A generation that is not due yet is still running: nothing is started, and
            // the state says so.
            Some(held) if now.as_secs_f64() < held.completes.as_secs_f64() => {
                (STATE_INPROGRESS, DESCRIPTION_INPROGRESS)
            }
            // A generation the clock has made due whose completion nothing has reported
            // yet: the transition is materialised here and reported once, because from
            // now on the account HAS that report.
            //
            // Only while that report is still readable, though: a generation whose report
            // has already passed its validity window is not news a caller can act on —
            // `GetCredentialReport` refuses such a report `ReportExpired` — so the call
            // falls through to the arms below, which start the new generation the account
            // now needs.
            Some(held)
                if !held.reported_complete
                    && now.secs()
                        < held.requested.secs()
                            + crate::state::CREDENTIAL_REPORT_VALIDITY_SECONDS =>
            {
                held.reported_complete = true;
                (STATE_COMPLETE, DESCRIPTION_COMPLETE)
            }
            // The account has a report, and the published rate does not allow another
            // one yet: no generation is started and nothing is written, and what the call
            // reports is the generation the account holds — complete, with its report
            // still the account's. The published rate governs whether a new report is
            // GENERATED, not whether the call is served.
            Some(held)
                if now.secs()
                    < held.requested.secs() + crate::state::CREDENTIAL_REPORT_REGENERATION_SECONDS =>
            {
                (STATE_COMPLETE, DESCRIPTION_COMPLETE)
            }
            // Either the account has never asked for a report, or the rate now allows a
            // new one: a generation is started, due after the span this emulator gives
            // one, and the REPORT itself is rendered here — a credential report is a
            // snapshot of the account's credentials, so it is made once, by the call that
            // starts the generation, and every later read of it answers those same bytes
            // ([`crate::state::CredentialReportGeneration::content`]).
            //
            // Which of the two cases it was decides the description: the sentence the
            // recorded first call carries states that no report exists, which is true only
            // of an account that has never had one.
            _ => {
                let first = store.credential_report.is_none();
                // A report that could not be written is no report: nothing is started and
                // the call reports the modelled server failure, rather than recording a
                // generation whose read would hand a client a file it cannot tell from the
                // report of an account holding nothing.
                let Some(content) = store.credential_report_csv(&key_uses) else {
                    return Err(GenerateCredentialReportErrorModeled::ServiceFailureException(
                        ServiceFailureException {
                            message: Some(
                                "the account's credential report could not be generated"
                                    .to_string(),
                            ),
                        },
                    )
                    .into());
                };
                store.credential_report = Some(crate::state::CredentialReportGeneration {
                    requested: now,
                    completes: ::aws_smithy_types::DateTime::from_secs(
                        now.secs() + crate::state::CREDENTIAL_REPORT_GENERATION_SECONDS,
                    ),
                    reported_complete: false,
                    content,
                });
                (
                    STATE_STARTED,
                    if first {
                        DESCRIPTION_STARTED
                    } else {
                        DESCRIPTION_RESTARTED
                    },
                )
            }
        };

        ::tracing::info!(
            audit = true,
            operation = "GenerateCredentialReport",
            outcome = "reported",
            caller_akid = %caller_akid,
            state = %report_state,
            "the credential report generation was reported"
        );

        Ok(GenerateCredentialReportOutput {
            state: Some(report_state.to_string()),
            description: Some(description.to_string()),
        })
    }
}
