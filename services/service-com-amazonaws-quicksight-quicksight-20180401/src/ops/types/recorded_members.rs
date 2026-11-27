// SPDX-License-Identifier: CC0-1.0

//! The members this service's success bodies carry that the on-disk model does not
//! declare, and the wire support each one needs.
//!
//! # Why this module exists
//!
//! `#[smithy_macros::smithy(shape = ...)]` derives a structure's `Schema`, its serializer
//! and its deserializer from the model's member list, and it admits EXACTLY that list — a
//! Rust field with no member behind it is a compile error — so a member the service really
//! reports but this model revision omits cannot be declared through the derive at all.
//!
//! Each structure below therefore PAIRS the derived structure (every modelled member, with
//! the wire name, the HTTP binding and the constraints the model gives it) with the
//! recorded member beside it, and writes the pair: the modelled members first, through the
//! generated serializer, then the recorded one. Nothing of the modelled half is restated
//! here, so a later model revision that declares the member only has to move the field
//! into the derived structure and delete the wrapper.
//!
//! A recorded member's schema sits in this service's own namespace, under the structure it
//! belongs to and at the index after that structure's last modelled member: the id never
//! leaves the process — the JSON codec writes a member's NAME as the object key — and the
//! index orders the slots the DESERIALIZE path fills, which these structures reach by
//! delegating to the modelled half.
#![allow(unused_imports)]

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::DeserializableStruct;

use super::*;

/// This service's namespace: every recorded member below is a member of one of ITS
/// shapes, so none of these ids can collide with another service's.
const NAMESPACE: &str = "com.amazonaws.quicksight";

/// `IAMPolicyAssignment$Arn` — the assignment's own ARN, reported inside the assignment
/// structure a describe answers with.
///
/// Index 6: the structure declares six members and no `Arn`
/// (`smithy_query(query="shape", shape="IAMPolicyAssignment")`).
static M_ASSIGNMENT_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.quicksight#IAMPolicyAssignment$Arn",
        NAMESPACE,
        "Arn",
    ),
    ShapeType::String,
    "Arn",
    6,
);

/// `UpdateIAMPolicyAssignmentResponse$Arn` — the updated assignment's own ARN.
///
/// Index 7: the response declares seven members and no `Arn`
/// (`smithy_query(query="shape", shape="UpdateIAMPolicyAssignmentResponse")`).
static M_UPDATED_ASSIGNMENT_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.quicksight#UpdateIAMPolicyAssignmentResponse$Arn",
        NAMESPACE,
        "Arn",
    ),
    ShapeType::String,
    "Arn",
    7,
);

/// `CreateIAMPolicyAssignmentResponse$Arn` — the created assignment's own ARN.
///
/// Index 7: the response declares seven members and no `Arn`
/// (`smithy_query(query="shape", shape="CreateIAMPolicyAssignmentResponse")`).
static M_CREATED_ASSIGNMENT_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.quicksight#CreateIAMPolicyAssignmentResponse$Arn",
        NAMESPACE,
        "Arn",
    ),
    ShapeType::String,
    "Arn",
    7,
);

/// `DeleteIAMPolicyAssignmentResponse$Arn` — the deleted assignment's own ARN.
///
/// Index 3: the response declares three members and no `Arn`
/// (`smithy_query(query="shape", shape="DeleteIAMPolicyAssignmentResponse")`).
static M_DELETED_ASSIGNMENT_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.quicksight#DeleteIAMPolicyAssignmentResponse$Arn",
        NAMESPACE,
        "Arn",
    ),
    ShapeType::String,
    "Arn",
    3,
);

/// `DataSource$CatalogEnabled` — whether the data source's catalog feature is on.
///
/// Index 15: the structure declares fifteen members and neither flag
/// (`smithy_query(query="shape", shape="DataSource")`).
static M_DATA_SOURCE_CATALOG_ENABLED: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.quicksight#DataSource$CatalogEnabled",
        NAMESPACE,
        "CatalogEnabled",
    ),
    ShapeType::Boolean,
    "CatalogEnabled",
    15,
);

/// `DataSource$TIPEnabled` — whether the data source's TIP feature is on. Index 16, after
/// [`M_DATA_SOURCE_CATALOG_ENABLED`].
static M_DATA_SOURCE_TIP_ENABLED: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.quicksight#DataSource$TIPEnabled",
        NAMESPACE,
        "TIPEnabled",
    ),
    ShapeType::Boolean,
    "TIPEnabled",
    16,
);

/// The `IAMPolicyAssignment` structure as this service reports it: the modelled members,
/// and the assignment's own ARN beside them.
#[derive(Debug, Clone, Default)]
pub struct ReportedIamPolicyAssignment {
    /// The members the model declares for `IAMPolicyAssignment`.
    pub assignment: IAMPolicyAssignment,
    /// The assignment's own ARN — [`crate::state::iam_policy_assignment_arn`].
    pub arn: String,
}

impl SerializableStruct for ReportedIamPolicyAssignment {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.assignment.serialize_members(s)?;
        s.write_string(&M_ASSIGNMENT_ARN, self.arn.as_str())
    }
}

impl DeserializableStruct for ReportedIamPolicyAssignment {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        // The modelled half owns the read: the recorded member is written by this
        // service, never read from a request, so a document that carries it still
        // deserializes to the assignment it describes.
        let assignment = IAMPolicyAssignment::deserialize(d)?;
        let arn = match (&assignment.aws_account_id, &assignment.assignment_name) {
            (Some(aws_account_id), Some(assignment_name)) => {
                crate::state::iam_policy_assignment_arn(aws_account_id, assignment_name)
            }
            _ => String::new(),
        };
        Ok(Self { assignment, arn })
    }
}

/// What `CreateIAMPolicyAssignment` answers: the modelled response, and the created
/// assignment's own ARN beside it.
#[derive(Debug, Clone, Default)]
pub struct ReportedIamPolicyAssignmentCreation {
    /// The members the model declares for `CreateIAMPolicyAssignmentResponse`.
    pub response: CreateIAMPolicyAssignmentOutput,
    /// The created assignment's own ARN — [`crate::state::iam_policy_assignment_arn`].
    pub arn: String,
}

impl SerializableStruct for ReportedIamPolicyAssignmentCreation {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.response.serialize_members(s)?;
        s.write_string(&M_CREATED_ASSIGNMENT_ARN, self.arn.as_str())
    }
}

/// What `UpdateIAMPolicyAssignment` answers: the modelled response, and the updated
/// assignment's own ARN beside it.
#[derive(Debug, Clone, Default)]
pub struct ReportedIamPolicyAssignmentUpdate {
    /// The members the model declares for `UpdateIAMPolicyAssignmentResponse`.
    pub response: UpdateIAMPolicyAssignmentOutput,
    /// The updated assignment's own ARN — [`crate::state::iam_policy_assignment_arn`].
    pub arn: String,
}

impl SerializableStruct for ReportedIamPolicyAssignmentUpdate {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.response.serialize_members(s)?;
        s.write_string(&M_UPDATED_ASSIGNMENT_ARN, self.arn.as_str())
    }
}

/// What `DeleteIAMPolicyAssignment` answers: the modelled response, and the deleted
/// assignment's own ARN beside it.
#[derive(Debug, Clone, Default)]
pub struct ReportedIamPolicyAssignmentDeletion {
    /// The members the model declares for `DeleteIAMPolicyAssignmentResponse`.
    pub response: DeleteIAMPolicyAssignmentOutput,
    /// The deleted assignment's own ARN — [`crate::state::iam_policy_assignment_arn`].
    pub arn: String,
}

impl SerializableStruct for ReportedIamPolicyAssignmentDeletion {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.response.serialize_members(s)?;
        s.write_string(&M_DELETED_ASSIGNMENT_ARN, self.arn.as_str())
    }
}

/// The `DataSource` structure as this service reports it: the modelled members, and the
/// two feature flags every data source of this service carries beside them.
#[derive(Debug, Clone, Default)]
pub struct ReportedDataSource {
    /// The members the model declares for `DataSource`.
    pub data_source: DataSource,
    /// Whether the catalog feature is on for this data source —
    /// [`crate::state::DATA_SOURCE_CATALOG_ENABLED`].
    pub catalog_enabled: bool,
    /// Whether the TIP feature is on for this data source —
    /// [`crate::state::DATA_SOURCE_TIP_ENABLED`].
    pub tip_enabled: bool,
}

#[allow(non_upper_case_globals)]
impl ReportedDataSource {
    /// The structure this is one of: a data source reported in a LIST is written against
    /// the element shape's own schema, which is the modelled `DataSource`'s.
    pub const SCHEMA: Schema = DataSource::SCHEMA;
}

impl SerializableStruct for ReportedDataSource {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.data_source.serialize_members(s)?;
        s.write_boolean(&M_DATA_SOURCE_CATALOG_ENABLED, self.catalog_enabled)?;
        s.write_boolean(&M_DATA_SOURCE_TIP_ENABLED, self.tip_enabled)
    }
}

impl DeserializableStruct for ReportedDataSource {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        // As [`ReportedIamPolicyAssignment`]: the modelled half owns the read, and the
        // flags are this service's own, so they are reported rather than read.
        Ok(Self {
            data_source: DataSource::deserialize(d)?,
            catalog_enabled: crate::state::DATA_SOURCE_CATALOG_ENABLED,
            tip_enabled: crate::state::DATA_SOURCE_TIP_ENABLED,
        })
    }
}
