// SPDX-License-Identifier: CC0-1.0

//! The response shapes of this service whose timestamp members declare
//! `@timestampFormat("date-time")`, written out instead of derived.
//!
//! WHY they are hand-written. `#[smithy_macros::smithy(shape = …)]` captures a member's
//! wire name, its HTTP binding and its constraint traits from the model, but NOT its
//! `@timestampFormat`: the expansion emits no format at all
//! (`vendor/smithy-macros/src/lib.rs` chains only the XML and JSON names onto a member
//! schema), and a timestamp member whose schema carries no format is written by the
//! restJson1 codec in the protocol's default EPOCH-SECONDS form
//! (`vendor/smithy-schema-server/src/json.rs`). A member the model marks
//! `date-time` has to travel as an RFC 3339 string, which is what an SDK deserializer
//! generated for it parses — so for those members, and only those, the member schema is
//! declared here with the format the model states.
//!
//! The rule is per MEMBER, not per service: every other timestamp of this service's API —
//! a `Domain`'s `createdAt`, a `Subscription`'s instants, a `Dataset`'s `createdAt` /
//! `updatedAt`, a listening session's stream instants — declares no `@timestampFormat`
//! (`smithy_query(query="shape",shape="SubscriptionDescription,DatasetSummary,MedicalScribeListeningSessionDetails",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! so those shapes stay derived and keep the protocol's epoch number.
//!
//! Each shape below is the model's, member for member: the same shape ids, wire names,
//! order and shape types the model declares, plus the `date-time` format on the members
//! that declare it. The constraint traits the derive also captures are deliberately NOT
//! reproduced (the validating walk reads an operation's `INPUT_SCHEMA` only), and neither
//! is a `DeserializableStruct` impl: these are operation OUTPUTS, which the server only
//! ever serializes.
//!
//! WHY HERE, beside this service's state rather than in `src/ops/types/`: every file under
//! that directory is rendered from the model by the scaffold generator and rewritten by the
//! next model update, which silently takes the format back off. `GetPatientInsightsJobOutput`
//! is the one such shape still declared there (`src/ops/types/get_patient_insights_job_types.rs`,
//! written out for exactly this reason), and this module is where the next one goes.
//! An operation whose output lives here names it explicitly, which shadows the derived copy
//! `super::super::ops::types` re-exports.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::traits::TimestampFormat;
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};

use crate::ops::types::{
    MedicalCodesEncounterContext, MedicalCodesPatientContext, MedicalCodingOutput,
    MedicalCodingOutputDataConfig,
};

/// One member schema of a response shape of this service, at `index` in the model's member
/// list — stated once so no shape below spells the namespace differently.
///
/// `shape_id` is the member's own id (`<shape>$<member>`) and `name` its wire name, both as
/// the model writes them.
const fn member(
    shape_id: &'static str,
    name: &'static str,
    shape_type: ShapeType,
    index: usize,
) -> Schema {
    Schema::new_member(
        ShapeId::from_static(shape_id, "com.amazonaws.connecthealth", name),
        shape_type,
        name,
        index,
    )
}

/// A `@timestampFormat("date-time")` member: [`member`] plus the format the model declares,
/// which is the ONE thing these shapes are written out for.
const fn date_time_member(
    shape_id: &'static str,
    name: &'static str,
    index: usize,
) -> Schema {
    member(shape_id, name, ShapeType::Timestamp, index).with_timestamp_format(
        TimestampFormat::DateTime,
    )
}

/// `StartPatientInsightsJobResponse`: the job the start accepted, and the instant it was
/// submitted.
///
/// `creationTime` is `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="StartPatientInsightsJobResponse")`).
#[derive(Debug, Clone, Default)]
pub struct StartPatientInsightsJobOutput {
    pub job_arn: String,
    pub job_id: String,
    pub creation_time: Option<::aws_smithy_types::DateTime>,
}

impl StartPatientInsightsJobOutput {
    const M_JOB_ARN: Schema = member(
        "com.amazonaws.connecthealth#StartPatientInsightsJobResponse$jobArn",
        "jobArn",
        ShapeType::String,
        0,
    );
    const M_JOB_ID: Schema = member(
        "com.amazonaws.connecthealth#StartPatientInsightsJobResponse$jobId",
        "jobId",
        ShapeType::String,
        1,
    );
    /// `@timestampFormat("date-time")`. THE reason this schema is hand-written.
    const M_CREATION_TIME: Schema = date_time_member(
        "com.amazonaws.connecthealth#StartPatientInsightsJobResponse$creationTime",
        "creationTime",
        2,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.connecthealth#StartPatientInsightsJobResponse",
            "com.amazonaws.connecthealth",
            "StartPatientInsightsJobResponse",
        ),
        ShapeType::Structure,
        &[&Self::M_JOB_ARN, &Self::M_JOB_ID, &Self::M_CREATION_TIME],
    );
}

pub static START_PATIENT_INSIGHTS_JOB_OUTPUT_SCHEMA: &Schema =
    &StartPatientInsightsJobOutput::SCHEMA;

impl SerializableStruct for StartPatientInsightsJobOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        // A `@required` member is written unconditionally (the derive's `is_option` split
        // puts a non-`Option` field on that branch); an optional one only when it is set.
        s.write_string(&Self::M_JOB_ARN, self.job_arn.as_str())?;
        s.write_string(&Self::M_JOB_ID, self.job_id.as_str())?;
        if let Some(value) = self.creation_time.as_ref() {
            s.write_timestamp(&Self::M_CREATION_TIME, value)?;
        }
        Ok(())
    }
}

/// `StartMedicalCodingJobResponse`: the job the start accepted, and the instant it was
/// submitted.
///
/// The same member set as [`StartPatientInsightsJobOutput`] against its own shape ids, and
/// `creationTime` carries the same `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="StartMedicalCodingJobResponse")`).
#[derive(Debug, Clone, Default)]
pub struct StartMedicalCodingJobOutput {
    pub job_arn: String,
    pub job_id: String,
    pub creation_time: Option<::aws_smithy_types::DateTime>,
}

impl StartMedicalCodingJobOutput {
    const M_JOB_ARN: Schema = member(
        "com.amazonaws.connecthealth#StartMedicalCodingJobResponse$jobArn",
        "jobArn",
        ShapeType::String,
        0,
    );
    const M_JOB_ID: Schema = member(
        "com.amazonaws.connecthealth#StartMedicalCodingJobResponse$jobId",
        "jobId",
        ShapeType::String,
        1,
    );
    /// `@timestampFormat("date-time")`.
    const M_CREATION_TIME: Schema = date_time_member(
        "com.amazonaws.connecthealth#StartMedicalCodingJobResponse$creationTime",
        "creationTime",
        2,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.connecthealth#StartMedicalCodingJobResponse",
            "com.amazonaws.connecthealth",
            "StartMedicalCodingJobResponse",
        ),
        ShapeType::Structure,
        &[&Self::M_JOB_ARN, &Self::M_JOB_ID, &Self::M_CREATION_TIME],
    );
}

pub static START_MEDICAL_CODING_JOB_OUTPUT_SCHEMA: &Schema = &StartMedicalCodingJobOutput::SCHEMA;

impl SerializableStruct for StartMedicalCodingJobOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_JOB_ARN, self.job_arn.as_str())?;
        s.write_string(&Self::M_JOB_ID, self.job_id.as_str())?;
        if let Some(value) = self.creation_time.as_ref() {
            s.write_timestamp(&Self::M_CREATION_TIME, value)?;
        }
        Ok(())
    }
}

/// `GetMedicalCodingJobResponse`: everything the medical coding job pair reports about one
/// job — the identity the start minted, the status the clock moved it to, the parameters
/// the start persisted and, once it has run, where its codes went.
///
/// `creationTime` and `updatedTime` both carry `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="GetMedicalCodingJobResponse")`).
#[derive(Debug, Clone, Default)]
pub struct GetMedicalCodingJobOutput {
    pub job_id: String,
    pub job_arn: String,
    pub job_status: String,
    pub creation_time: Option<::aws_smithy_types::DateTime>,
    pub updated_time: Option<::aws_smithy_types::DateTime>,
    pub text: String,
    pub patient_context: Option<MedicalCodesPatientContext>,
    pub encounter_context: Option<MedicalCodesEncounterContext>,
    pub output_data_config: MedicalCodingOutputDataConfig,
    pub dataset_id: Option<String>,
    pub medical_coding_output: Option<MedicalCodingOutput>,
    pub status_details: Option<String>,
}

impl GetMedicalCodingJobOutput {
    const M_JOB_ID: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$jobId",
        "jobId",
        ShapeType::String,
        0,
    );
    const M_JOB_ARN: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$jobArn",
        "jobArn",
        ShapeType::String,
        1,
    );
    /// `jobStatus` targets the `JobStatus` enum, which is string-encoded on the wire — the
    /// derive classifies an enum member as a string too.
    const M_JOB_STATUS: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$jobStatus",
        "jobStatus",
        ShapeType::String,
        2,
    );
    /// `@timestampFormat("date-time")`. THE reason this schema is hand-written.
    const M_CREATION_TIME: Schema = date_time_member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$creationTime",
        "creationTime",
        3,
    );
    /// `@timestampFormat("date-time")`, as `creationTime` above.
    const M_UPDATED_TIME: Schema = date_time_member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$updatedTime",
        "updatedTime",
        4,
    );
    const M_TEXT: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$text",
        "text",
        ShapeType::String,
        5,
    );
    const M_PATIENT_CONTEXT: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$patientContext",
        "patientContext",
        ShapeType::Structure,
        6,
    );
    const M_ENCOUNTER_CONTEXT: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$encounterContext",
        "encounterContext",
        ShapeType::Structure,
        7,
    );
    const M_OUTPUT_DATA_CONFIG: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$outputDataConfig",
        "outputDataConfig",
        ShapeType::Structure,
        8,
    );
    const M_DATASET_ID: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$datasetId",
        "datasetId",
        ShapeType::String,
        9,
    );
    const M_MEDICAL_CODING_OUTPUT: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$medicalCodingOutput",
        "medicalCodingOutput",
        ShapeType::Structure,
        10,
    );
    const M_STATUS_DETAILS: Schema = member(
        "com.amazonaws.connecthealth#GetMedicalCodingJobResponse$statusDetails",
        "statusDetails",
        ShapeType::String,
        11,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.connecthealth#GetMedicalCodingJobResponse",
            "com.amazonaws.connecthealth",
            "GetMedicalCodingJobResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_JOB_ID,
            &Self::M_JOB_ARN,
            &Self::M_JOB_STATUS,
            &Self::M_CREATION_TIME,
            &Self::M_UPDATED_TIME,
            &Self::M_TEXT,
            &Self::M_PATIENT_CONTEXT,
            &Self::M_ENCOUNTER_CONTEXT,
            &Self::M_OUTPUT_DATA_CONFIG,
            &Self::M_DATASET_ID,
            &Self::M_MEDICAL_CODING_OUTPUT,
            &Self::M_STATUS_DETAILS,
        ],
    );
}

pub static GET_MEDICAL_CODING_JOB_OUTPUT_SCHEMA: &Schema = &GetMedicalCodingJobOutput::SCHEMA;

impl SerializableStruct for GetMedicalCodingJobOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_JOB_ID, self.job_id.as_str())?;
        s.write_string(&Self::M_JOB_ARN, self.job_arn.as_str())?;
        s.write_string(&Self::M_JOB_STATUS, self.job_status.as_str())?;
        if let Some(value) = self.creation_time.as_ref() {
            s.write_timestamp(&Self::M_CREATION_TIME, value)?;
        }
        if let Some(value) = self.updated_time.as_ref() {
            s.write_timestamp(&Self::M_UPDATED_TIME, value)?;
        }
        s.write_string(&Self::M_TEXT, self.text.as_str())?;
        if let Some(value) = self.patient_context.as_ref() {
            s.write_struct(&Self::M_PATIENT_CONTEXT, value)?;
        }
        if let Some(value) = self.encounter_context.as_ref() {
            s.write_struct(&Self::M_ENCOUNTER_CONTEXT, value)?;
        }
        s.write_struct(&Self::M_OUTPUT_DATA_CONFIG, &self.output_data_config)?;
        if let Some(value) = self.dataset_id.as_ref() {
            s.write_string(&Self::M_DATASET_ID, value.as_str())?;
        }
        if let Some(value) = self.medical_coding_output.as_ref() {
            s.write_struct(&Self::M_MEDICAL_CODING_OUTPUT, value)?;
        }
        if let Some(value) = self.status_details.as_ref() {
            s.write_string(&Self::M_STATUS_DETAILS, value.as_str())?;
        }
        Ok(())
    }
}
