// SPDX-License-Identifier: CC0-1.0

//! The `ExecutionRecord` one flow run is reported as, and the `executionMetadata` block it
//! carries.
//!
//! WHY THIS MODULE EXISTS. `#[smithy_macros::smithy(shape = ...)]` derives a structure's
//! `Schema`, its serializer and its deserializer from the on-disk model's member list, and
//! it admits EXACTLY that list: a Rust field with no member behind it is a compile error, so
//! a member the service really reports but the published model omits cannot be declared
//! through the derive at all. AppFlow reports four such members on every run record — the
//! run's own `executionMessage`, its `executionMetadata` accounting block, and
//! `executionStartTime`/`lastUpdatedTime` beside `startedAt`/`lastUpdatedAt` — so this
//! module emits for `ExecutionRecord` the same items the derive emits: one member schema per
//! member (index = its position in the member list), the struct `SCHEMA` naming them in that
//! order, and the `SerializableStruct`/`DeserializableStruct` pair.
//!
//! The model's own members keep the indices and the order the model gives them (the order
//! `smithy_query(query="shape", shape="ExecutionRecord")` lists them in); the members it does
//! not declare are appended LAST, with shape ids in a namespace of their own
//! ([`RECORDED_NAMESPACE`]), so neither can claim to be modelled nor collide with a member the
//! model may declare later — a model refresh only has to move them into place and put the
//! derive back. The ids never leave the process: the JSON codec writes a member's NAME as the
//! object key.
//!
//! WHAT IS DELIBERATELY ABSENT, for the same reason the derive's own output would be inert
//! here: the constraint traits (the validating walk runs on operation INPUTS only, and this
//! structure is output-only), the `@http` chain (baked on only for an operation's input) and
//! `@jsonName` (no member here declares one: each one's wire name IS its member name).
//!
//! Nothing here decides a VALUE: every member is written from what the stored run holds (see
//! `crate::state::FlowExecution::to_execution_record`).

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::aws_smithy_schema_server::DeserializableStruct;
use ::aws_smithy_types::DateTime;

use super::{ExecutionResult, MetadataCatalogDetail};

/// The namespace the shapes and members the published model does not declare sit in: NOT
/// this service's own, so a hand-written member can never be mistaken for a modelled one.
const RECORDED_NAMESPACE: &str = "com.amazonaws.appflow.recorded";

/// One recorded run of a flow, as `DescribeFlowExecutionRecords` reports it.
///
/// The first eight members are `com.amazonaws.appflow#ExecutionRecord`'s, in the model's
/// member order. The last four are the ones AppFlow reports beyond the published model:
///
/// * `executionMessage` — what the run has to say for itself, in one sentence;
/// * `executionMetadata` — the run's own accounting, beside the `executionResult` counters;
/// * `executionStartTime` / `lastUpdatedTime` — the same two instants as `startedAt` /
///   `lastUpdatedAt`, under the names the console and several SDK samples read.
#[derive(Debug, Clone, Default)]
pub struct ExecutionRecord {
    pub execution_id: Option<String>,
    pub execution_status: Option<String>,
    pub execution_result: Option<ExecutionResult>,
    pub started_at: Option<DateTime>,
    pub last_updated_at: Option<DateTime>,
    pub data_pull_start_time: Option<DateTime>,
    pub data_pull_end_time: Option<DateTime>,
    pub metadata_catalog_details: Option<Vec<MetadataCatalogDetail>>,
    pub execution_message: Option<String>,
    pub execution_metadata: Option<ExecutionMetadata>,
    pub execution_start_time: Option<DateTime>,
    pub last_updated_time: Option<DateTime>,
}

impl ExecutionRecord {
    pub const M_EXECUTION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$executionId",
            "com.amazonaws.appflow",
            "executionId",
        ),
        ShapeType::String,
        "executionId",
        0,
    );
    pub const M_EXECUTION_STATUS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$executionStatus",
            "com.amazonaws.appflow",
            "executionStatus",
        ),
        ShapeType::String,
        "executionStatus",
        1,
    );
    pub const M_EXECUTION_RESULT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$executionResult",
            "com.amazonaws.appflow",
            "executionResult",
        ),
        ShapeType::Structure,
        "executionResult",
        2,
    );
    pub const M_STARTED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$startedAt",
            "com.amazonaws.appflow",
            "startedAt",
        ),
        ShapeType::Timestamp,
        "startedAt",
        3,
    );
    pub const M_LAST_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$lastUpdatedAt",
            "com.amazonaws.appflow",
            "lastUpdatedAt",
        ),
        ShapeType::Timestamp,
        "lastUpdatedAt",
        4,
    );
    pub const M_DATA_PULL_START_TIME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$dataPullStartTime",
            "com.amazonaws.appflow",
            "dataPullStartTime",
        ),
        ShapeType::Timestamp,
        "dataPullStartTime",
        5,
    );
    pub const M_DATA_PULL_END_TIME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$dataPullEndTime",
            "com.amazonaws.appflow",
            "dataPullEndTime",
        ),
        ShapeType::Timestamp,
        "dataPullEndTime",
        6,
    );
    pub const M_METADATA_CATALOG_DETAILS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord$metadataCatalogDetails",
            "com.amazonaws.appflow",
            "metadataCatalogDetails",
        ),
        ShapeType::List,
        "metadataCatalogDetails",
        7,
    );
    pub const M_EXECUTION_MESSAGE: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow.recorded#ExecutionRecord$executionMessage",
            RECORDED_NAMESPACE,
            "executionMessage",
        ),
        ShapeType::String,
        "executionMessage",
        8,
    );
    pub const M_EXECUTION_METADATA: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow.recorded#ExecutionRecord$executionMetadata",
            RECORDED_NAMESPACE,
            "executionMetadata",
        ),
        ShapeType::Structure,
        "executionMetadata",
        9,
    );
    pub const M_EXECUTION_START_TIME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow.recorded#ExecutionRecord$executionStartTime",
            RECORDED_NAMESPACE,
            "executionStartTime",
        ),
        ShapeType::Timestamp,
        "executionStartTime",
        10,
    );
    pub const M_LAST_UPDATED_TIME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appflow.recorded#ExecutionRecord$lastUpdatedTime",
            RECORDED_NAMESPACE,
            "lastUpdatedTime",
        ),
        ShapeType::Timestamp,
        "lastUpdatedTime",
        11,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.appflow#ExecutionRecord",
            "com.amazonaws.appflow",
            "ExecutionRecord",
        ),
        ShapeType::Structure,
        &[
            &Self::M_EXECUTION_ID,
            &Self::M_EXECUTION_STATUS,
            &Self::M_EXECUTION_RESULT,
            &Self::M_STARTED_AT,
            &Self::M_LAST_UPDATED_AT,
            &Self::M_DATA_PULL_START_TIME,
            &Self::M_DATA_PULL_END_TIME,
            &Self::M_METADATA_CATALOG_DETAILS,
            &Self::M_EXECUTION_MESSAGE,
            &Self::M_EXECUTION_METADATA,
            &Self::M_EXECUTION_START_TIME,
            &Self::M_LAST_UPDATED_TIME,
        ],
    );
}

pub static EXECUTION_RECORD_SCHEMA: &Schema = &ExecutionRecord::SCHEMA;

impl SerializableStruct for ExecutionRecord {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        if let Some(value) = self.execution_id.as_ref() {
            s.write_string(&Self::M_EXECUTION_ID, value.as_str())?;
        }
        if let Some(value) = self.execution_status.as_ref() {
            s.write_string(&Self::M_EXECUTION_STATUS, value.as_str())?;
        }
        if let Some(value) = self.execution_result.as_ref() {
            s.write_struct(&Self::M_EXECUTION_RESULT, value)?;
        }
        if let Some(value) = self.started_at.as_ref() {
            s.write_timestamp(&Self::M_STARTED_AT, value)?;
        }
        if let Some(value) = self.last_updated_at.as_ref() {
            s.write_timestamp(&Self::M_LAST_UPDATED_AT, value)?;
        }
        if let Some(value) = self.data_pull_start_time.as_ref() {
            s.write_timestamp(&Self::M_DATA_PULL_START_TIME, value)?;
        }
        if let Some(value) = self.data_pull_end_time.as_ref() {
            s.write_timestamp(&Self::M_DATA_PULL_END_TIME, value)?;
        }
        if let Some(value) = self.metadata_catalog_details.as_ref() {
            s.write_list(&Self::M_METADATA_CATALOG_DETAILS, &|element| {
                for detail in value.iter() {
                    element.write_struct(&MetadataCatalogDetail::SCHEMA, detail)?;
                }
                Ok(())
            })?;
        }
        if let Some(value) = self.execution_message.as_ref() {
            s.write_string(&Self::M_EXECUTION_MESSAGE, value.as_str())?;
        }
        if let Some(value) = self.execution_metadata.as_ref() {
            s.write_struct(&Self::M_EXECUTION_METADATA, value)?;
        }
        if let Some(value) = self.execution_start_time.as_ref() {
            s.write_timestamp(&Self::M_EXECUTION_START_TIME, value)?;
        }
        if let Some(value) = self.last_updated_time.as_ref() {
            s.write_timestamp(&Self::M_LAST_UPDATED_TIME, value)?;
        }
        Ok(())
    }
}

impl DeserializableStruct for ExecutionRecord {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut record = Self::default();
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => record.execution_id = Some(d.read_string(&Self::M_EXECUTION_ID)?),
                Some(1) => {
                    record.execution_status = Some(d.read_string(&Self::M_EXECUTION_STATUS)?)
                }
                Some(2) => {
                    record.execution_result = Some(ExecutionResult::deserialize(d)?);
                }
                Some(3) => record.started_at = Some(d.read_timestamp(&Self::M_STARTED_AT)?),
                Some(4) => {
                    record.last_updated_at = Some(d.read_timestamp(&Self::M_LAST_UPDATED_AT)?)
                }
                Some(5) => {
                    record.data_pull_start_time =
                        Some(d.read_timestamp(&Self::M_DATA_PULL_START_TIME)?)
                }
                Some(6) => {
                    record.data_pull_end_time =
                        Some(d.read_timestamp(&Self::M_DATA_PULL_END_TIME)?)
                }
                Some(7) => {
                    let mut details: Vec<MetadataCatalogDetail> = Vec::new();
                    d.read_list(&Self::M_METADATA_CATALOG_DETAILS, &mut |element| {
                        details.push(MetadataCatalogDetail::deserialize(element)?);
                        Ok(())
                    })?;
                    record.metadata_catalog_details = Some(details);
                }
                Some(8) => {
                    record.execution_message = Some(d.read_string(&Self::M_EXECUTION_MESSAGE)?)
                }
                Some(9) => {
                    record.execution_metadata = Some(ExecutionMetadata::deserialize(d)?);
                }
                Some(10) => {
                    record.execution_start_time =
                        Some(d.read_timestamp(&Self::M_EXECUTION_START_TIME)?)
                }
                Some(11) => {
                    record.last_updated_time = Some(d.read_timestamp(&Self::M_LAST_UPDATED_TIME)?)
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok(record)
    }
}

/// The `executionMetadata` of one run: what that run actually did, counted.
///
/// A member the published model does not declare at all, so the whole block lives in
/// [`RECORDED_NAMESPACE`]. `reason` is the only member that may be reported as `null`: a run
/// that ended well has no reason to give, and reporting the member as absent would leave a
/// client unable to tell "no reason" from "not reported".
#[derive(Debug, Clone, Default)]
pub struct ExecutionMetadata {
    pub bytes_processed: Option<i64>,
    pub bytes_written: Option<i64>,
    pub max_page_size: Option<i64>,
    pub num_failures: Option<i64>,
    pub num_filtered_records: Option<i64>,
    pub num_parallel_processes: Option<i64>,
    pub num_put_failures: Option<i64>,
    pub reason: Option<String>,
    pub records_processed: Option<i64>,
    pub terminate_flow: Option<bool>,
}

impl ExecutionMetadata {
    pub const M_BYTES_PROCESSED: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$bytesProcessed",
        "bytesProcessed",
        ShapeType::Long,
        0,
    );
    pub const M_BYTES_WRITTEN: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$bytesWritten",
        "bytesWritten",
        ShapeType::Long,
        1,
    );
    pub const M_MAX_PAGE_SIZE: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$maxPageSize",
        "maxPageSize",
        ShapeType::Long,
        2,
    );
    pub const M_NUM_FAILURES: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$numFailures",
        "numFailures",
        ShapeType::Long,
        3,
    );
    pub const M_NUM_FILTERED_RECORDS: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$numFilteredRecords",
        "numFilteredRecords",
        ShapeType::Long,
        4,
    );
    pub const M_NUM_PARALLEL_PROCESSES: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$numParallelProcesses",
        "numParallelProcesses",
        ShapeType::Long,
        5,
    );
    pub const M_NUM_PUT_FAILURES: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$numPutFailures",
        "numPutFailures",
        ShapeType::Long,
        6,
    );
    pub const M_REASON: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$reason",
        "reason",
        ShapeType::String,
        7,
    );
    pub const M_RECORDS_PROCESSED: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$recordsProcessed",
        "recordsProcessed",
        ShapeType::Long,
        8,
    );
    pub const M_TERMINATE_FLOW: Schema = Self::member(
        "com.amazonaws.appflow.recorded#ExecutionMetadata$terminateFlow",
        "terminateFlow",
        ShapeType::Boolean,
        9,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.appflow.recorded#ExecutionMetadata",
            RECORDED_NAMESPACE,
            "ExecutionMetadata",
        ),
        ShapeType::Structure,
        &[
            &Self::M_BYTES_PROCESSED,
            &Self::M_BYTES_WRITTEN,
            &Self::M_MAX_PAGE_SIZE,
            &Self::M_NUM_FAILURES,
            &Self::M_NUM_FILTERED_RECORDS,
            &Self::M_NUM_PARALLEL_PROCESSES,
            &Self::M_NUM_PUT_FAILURES,
            &Self::M_REASON,
            &Self::M_RECORDS_PROCESSED,
            &Self::M_TERMINATE_FLOW,
        ],
    );

    /// One member of this block, as the codegen emits a member schema: its shape id, its
    /// shape type, its wire name and its index in the member list.
    const fn member(
        id: &'static str,
        name: &'static str,
        shape_type: ShapeType,
        index: usize,
    ) -> Schema {
        Schema::new_member(
            ShapeId::from_static(id, RECORDED_NAMESPACE, name),
            shape_type,
            name,
            index,
        )
    }
}

pub static EXECUTION_METADATA_SCHEMA: &Schema = &ExecutionMetadata::SCHEMA;

impl SerializableStruct for ExecutionMetadata {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        if let Some(value) = self.bytes_processed {
            s.write_long(&Self::M_BYTES_PROCESSED, value)?;
        }
        if let Some(value) = self.bytes_written {
            s.write_long(&Self::M_BYTES_WRITTEN, value)?;
        }
        if let Some(value) = self.max_page_size {
            s.write_long(&Self::M_MAX_PAGE_SIZE, value)?;
        }
        if let Some(value) = self.num_failures {
            s.write_long(&Self::M_NUM_FAILURES, value)?;
        }
        if let Some(value) = self.num_filtered_records {
            s.write_long(&Self::M_NUM_FILTERED_RECORDS, value)?;
        }
        if let Some(value) = self.num_parallel_processes {
            s.write_long(&Self::M_NUM_PARALLEL_PROCESSES, value)?;
        }
        if let Some(value) = self.num_put_failures {
            s.write_long(&Self::M_NUM_PUT_FAILURES, value)?;
        }
        match self.reason.as_ref() {
            Some(reason) => s.write_string(&Self::M_REASON, reason.as_str())?,
            None => s.write_null(&Self::M_REASON)?,
        }
        if let Some(value) = self.records_processed {
            s.write_long(&Self::M_RECORDS_PROCESSED, value)?;
        }
        if let Some(value) = self.terminate_flow {
            s.write_boolean(&Self::M_TERMINATE_FLOW, value)?;
        }
        Ok(())
    }
}

impl DeserializableStruct for ExecutionMetadata {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut metadata = Self::default();
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => metadata.bytes_processed = Some(d.read_long(&Self::M_BYTES_PROCESSED)?),
                Some(1) => metadata.bytes_written = Some(d.read_long(&Self::M_BYTES_WRITTEN)?),
                Some(2) => metadata.max_page_size = Some(d.read_long(&Self::M_MAX_PAGE_SIZE)?),
                Some(3) => metadata.num_failures = Some(d.read_long(&Self::M_NUM_FAILURES)?),
                Some(4) => {
                    metadata.num_filtered_records =
                        Some(d.read_long(&Self::M_NUM_FILTERED_RECORDS)?)
                }
                Some(5) => {
                    metadata.num_parallel_processes =
                        Some(d.read_long(&Self::M_NUM_PARALLEL_PROCESSES)?)
                }
                Some(6) => {
                    metadata.num_put_failures = Some(d.read_long(&Self::M_NUM_PUT_FAILURES)?)
                }
                Some(7) => metadata.reason = Some(d.read_string(&Self::M_REASON)?),
                Some(8) => {
                    metadata.records_processed = Some(d.read_long(&Self::M_RECORDS_PROCESSED)?)
                }
                Some(9) => {
                    metadata.terminate_flow = Some(d.read_boolean(&Self::M_TERMINATE_FLOW)?)
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok(metadata)
    }
}
