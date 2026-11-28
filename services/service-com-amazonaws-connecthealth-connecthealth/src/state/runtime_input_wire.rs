// SPDX-License-Identifier: CC0-1.0

//! The request shape of `StartPatientInsightsJob`, declared with the one member whose
//! modelled rules its own HANDLER judges rather than the request boundary.
//!
//! WHY it is declared here instead of being taken from `src/ops/types/`. That operation is
//! served by the patient-insights RUNTIME endpoint (`hostPrefix runtime.`), which states a
//! refusal of the request itself as ONE fixed sentence
//! ([`crate::service::error_detail::REQUEST_VALIDATION_FAILED_MESSAGE`]) rather than as the
//! per-member constraint clause the control plane's boundary walk renders. A member whose
//! constraint traits the walk can see is refused BEFORE any handler runs, in the
//! framework's words — so the output-location member carries `skip_validation`, which keeps
//! its `@length` and `@pattern` off the member schema the walk reads, and the handler
//! judges the same rules itself ([`crate::state::output_location_within_shape`]) after it
//! has authorized the domain. The ORDER is the point: that endpoint authorizes the domain a
//! request addresses before it says anything about the request's own values.
//!
//! Everything else about these shapes is the MODEL's, captured by the same derive the
//! generated copies use: the shape ids, the member names, the HTTP bindings and every other
//! member's constraints. Only the one member's constraints are withheld from the walk, and
//! nothing about the wire is hand-written.
//!
//! The generated copies (`src/ops/types/start_patient_insights_job_types.rs`) are rendered
//! from the model and rewritten by every model update, which takes `skip_validation` back
//! off; the operation names these explicitly, which shadows them.

use crate::ops::types::OutputDataConfig;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-connecthealth-connecthealth.json");

/// `OutputDataConfig` as the runtime endpoint reads it: the modelled shape with its one
/// member's `@length` / `@pattern` withheld from the request boundary, so the endpoint's own
/// handler is what refuses a location outside them.
///
/// The shape id is the model's, so the member travels under the model's own wire name; only
/// WHERE the rules are enforced differs.
#[smithy_macros::smithy(
    shape = "com.amazonaws.connecthealth#OutputDataConfig",
    skip_validation = ["s3OutputPath"]
)]
#[derive(Debug, Clone, Default)]
pub struct RuntimeOutputDataConfig {
    pub s3_output_path: String,
}

impl RuntimeOutputDataConfig {
    /// This configuration as the modelled shape the job RECORD holds and every reader of a
    /// job projects — the one place the two spellings of one shape meet.
    pub fn as_modelled(&self) -> OutputDataConfig {
        OutputDataConfig {
            s3_output_path: self.s3_output_path.clone(),
        }
    }
}

/// `StartPatientInsightsJobRequest`, with the output location declared as
/// [`RuntimeOutputDataConfig`].
#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#StartPatientInsightsJobRequest")]
#[derive(Debug, Clone, Default)]
pub struct StartPatientInsightsJobInput {
    pub domain_id: String,
    pub patient_context: crate::ops::types::PatientInsightsPatientContext,
    pub insights_context: crate::ops::types::InsightsContext,
    pub encounter_context: crate::ops::types::PatientInsightsEncounterContext,
    pub user_context: crate::ops::types::UserContext,
    pub input_data_config: crate::ops::types::InputDataConfig,
    pub output_data_config: RuntimeOutputDataConfig,
    pub client_token: Option<String>,
}
