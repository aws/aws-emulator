// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_index_rebuild_status` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
//!
//! HAND-WRITTEN at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: the output's three timestamp members carry `@timestampFormat("date-time")` and
//! the derive does not carry that trait onto the member schemas it emits, so the shape is
//! written out below instead. The input beside it is still declared from the model by the
//! derive. A refresh that restates either shape has to be followed here by hand.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-neptunedata-amazonneptunedataplane.json");

#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#GetIndexRebuildStatusInput")]
#[derive(Debug, Clone, Default)]
pub struct GetIndexRebuildStatusInput {
    pub rebuild_id: String,
}

/// `GetIndexRebuildStatusOutput`, with its schema written out instead of derived.
///
/// WHY it is not `#[smithy_macros::smithy(shape = ...)]` like the input beside it: three of
/// its members are timestamps the model marks `@timestampFormat("date-time")`
/// (`smithy_query(query="shape", shape="GetIndexRebuildStatusOutput")` reports `startTime`,
/// `estimatedEndTime` and `endTime` as `{"target": "Timestamp", "timestampFormat":
/// "date-time"}`, and each member's own documentation calls the value "the ISO-8601
/// timestamp"), and the derive does not carry that trait onto the member schemas it emits.
/// The JSON codec decides a timestamp's rendering per member, from the member schema's own
/// format (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
/// `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds, a
/// JSON NUMBER — when the member schema reports none. So all three instants would go onto
/// the wire in a form no client generated from this model can read.
///
/// The nine member schemas below are the model's, member for member: the same shape ids,
/// the same order and the same member indices (no member of this shape declares a
/// `@jsonName`, so each one's wire name IS its Smithy member name), plus the `date-time`
/// format the model declares for the three timestamps and for nothing else.
///
/// This is a PER-SHAPE workaround and has to be kept in step with the model by hand; the
/// fix belongs in the vendored codegen (chain `.with_timestamp_format(..)` when building a
/// member schema), which is read-only from a service crate. The index-rebuild cases in
/// `tests/all/neptune_data_plane.rs` pin the wire form, so they fail if this shape goes
/// back to the derive.
///
/// The derive's `DeserializableStruct` impl is deliberately not reproduced: this shape is an
/// operation OUTPUT, which the server only ever serializes
/// (`smithy_server_runtime::Operation::Output` requires `SerializableStruct`; only `Input`
/// is deserialized), so a hand-written read path would be code with no caller.
#[derive(Debug, Clone)]
pub struct GetIndexRebuildStatusOutput {
    pub status: String,
    pub scope: String,
    pub concurrency: i32,
    pub write_buffer_size: String,
    pub start_time: ::aws_smithy_types::DateTime,
    pub progress_percentage: i32,
    pub estimated_end_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub failure_reason: Option<String>,
}

/// The namespace and the local name of the shape every member schema below is named under.
const REBUILD_STATUS_NAMESPACE: &str = "com.amazonaws.neptunedata";
const REBUILD_STATUS_OUTPUT: &str = "GetIndexRebuildStatusOutput";

impl GetIndexRebuildStatusOutput {
    /// `status`, a `RebuildStatus` enum — string-encoded, as every Smithy `enum` is.
    pub const M_STATUS: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$status",
            REBUILD_STATUS_NAMESPACE,
            "status",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "status",
        0,
    );

    /// `scope`, a `Scope` enum.
    pub const M_SCOPE: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$scope",
            REBUILD_STATUS_NAMESPACE,
            "scope",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "scope",
        1,
    );

    /// `concurrency`, a `RebuildConcurrency` integer.
    pub const M_CONCURRENCY: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$concurrency",
            REBUILD_STATUS_NAMESPACE,
            "concurrency",
        ),
        ::aws_smithy_schema::ShapeType::Integer,
        "concurrency",
        2,
    );

    /// `writeBufferSize`, a `WriteBufferSize` enum.
    pub const M_WRITE_BUFFER_SIZE: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$writeBufferSize",
                REBUILD_STATUS_NAMESPACE,
                "writeBufferSize",
            ),
            ::aws_smithy_schema::ShapeType::String,
            "writeBufferSize",
            3,
        );

    /// `startTime`, a `Timestamp` the model marks `@timestampFormat("date-time")`.
    pub const M_START_TIME: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$startTime",
            REBUILD_STATUS_NAMESPACE,
            "startTime",
        ),
        ::aws_smithy_schema::ShapeType::Timestamp,
        "startTime",
        4,
    )
    // The one chain call the vendored codegen omits; see the note on the struct.
    .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime);

    /// `progressPercentage`, an `Integer`.
    pub const M_PROGRESS_PERCENTAGE: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$progressPercentage",
                REBUILD_STATUS_NAMESPACE,
                "progressPercentage",
            ),
            ::aws_smithy_schema::ShapeType::Integer,
            "progressPercentage",
            5,
        );

    /// `estimatedEndTime`, a `Timestamp` the model marks `@timestampFormat("date-time")`.
    pub const M_ESTIMATED_END_TIME: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$estimatedEndTime",
                REBUILD_STATUS_NAMESPACE,
                "estimatedEndTime",
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            "estimatedEndTime",
            6,
        )
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime);

    /// `endTime`, a `Timestamp` the model marks `@timestampFormat("date-time")`.
    pub const M_END_TIME: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$endTime",
            REBUILD_STATUS_NAMESPACE,
            "endTime",
        ),
        ::aws_smithy_schema::ShapeType::Timestamp,
        "endTime",
        7,
    )
    .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime);

    /// `failureReason`, a `String`.
    pub const M_FAILURE_REASON: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput$failureReason",
                REBUILD_STATUS_NAMESPACE,
                "failureReason",
            ),
            ::aws_smithy_schema::ShapeType::String,
            "failureReason",
            8,
        );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.neptunedata#GetIndexRebuildStatusOutput",
            REBUILD_STATUS_NAMESPACE,
            REBUILD_STATUS_OUTPUT,
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_STATUS,
            &Self::M_SCOPE,
            &Self::M_CONCURRENCY,
            &Self::M_WRITE_BUFFER_SIZE,
            &Self::M_START_TIME,
            &Self::M_PROGRESS_PERCENTAGE,
            &Self::M_ESTIMATED_END_TIME,
            &Self::M_END_TIME,
            &Self::M_FAILURE_REASON,
        ],
    );
}

pub static GET_INDEX_REBUILD_STATUS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &GetIndexRebuildStatusOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for GetIndexRebuildStatusOutput {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        // The six members the model marks `@required` are always written; the three it does
        // not are written only when the rebuild has one to report.
        serializer.write_string(&Self::M_STATUS, self.status.as_str())?;
        serializer.write_string(&Self::M_SCOPE, self.scope.as_str())?;
        serializer.write_integer(&Self::M_CONCURRENCY, self.concurrency)?;
        serializer.write_string(&Self::M_WRITE_BUFFER_SIZE, self.write_buffer_size.as_str())?;
        serializer.write_timestamp(&Self::M_START_TIME, &self.start_time)?;
        serializer.write_integer(&Self::M_PROGRESS_PERCENTAGE, self.progress_percentage)?;
        if let Some(value) = self.estimated_end_time.as_ref() {
            serializer.write_timestamp(&Self::M_ESTIMATED_END_TIME, value)?;
        }
        if let Some(value) = self.end_time.as_ref() {
            serializer.write_timestamp(&Self::M_END_TIME, value)?;
        }
        if let Some(value) = self.failure_reason.as_ref() {
            serializer.write_string(&Self::M_FAILURE_REASON, value.as_str())?;
        }
        ::std::result::Result::Ok(())
    }
}
