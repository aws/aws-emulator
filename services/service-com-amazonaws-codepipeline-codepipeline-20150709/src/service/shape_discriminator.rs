// SPDX-License-Identifier: CC0-1.0

//! WHICH structures of this service carry the awsJson shape discriminator, and the shape
//! name each one carries.
//!
//! On `awsJson1_1`, one of the two protocols this service's model declares
//! (`smithy_query query="service_info"`), two of its structures arrive at a client carrying
//! `__type` — the member the awsJson family names a serialized shape with — beside the
//! members the shape declares:
//!
//! ```text
//! "actionTypeId": {"__type": "ActionTypeId", "category": "Source", "owner": "AWS", …}
//! "encryptionKey": {"__type": "EncryptionKey", "id": "…", "type": "KMS"}
//! ```
//!
//! `__type` is a PROTOCOL member rather than a modelled one: the model declares it
//! nowhere, so the generated serializer (which writes exactly the members the shape
//! declares) cannot emit it, and a client that keys such a structure's shape off that
//! discriminator — the same discriminator it reads an error's class from — finds nothing.
//! The framework writes one for an ERROR body
//! (`vendor/smithy-schema-server/src/rpc.rs`'s `__TYPE_MEMBER_SCHEMA`), but nothing on the
//! success path does, and `vendor/` is read-only. So it is written on the SERVICE side,
//! by the serializer in [`crate::service::action_type_wire_form`], over the catalog here.
//!
//! The two shapes a recorded response of this service shows it on are `ActionTypeId` and
//! `EncryptionKey` — the two structures this API reuses across its planes: an action
//! type's identity and an artifact store's KMS key, each appearing in a pipeline's
//! structure, in a catalog entry, in an action execution and in a job's data alike. The
//! rule is per SHAPE and not per operation, so it holds wherever one of them is
//! serialized. No other structure of this service carries one: a recorded response shows
//! the discriminator on these two shapes and on nothing enclosing them, so nothing else is
//! given one here.

use aws_smithy_schema::{Schema, ShapeId, ShapeType};

/// The member the awsJson protocols name a serialized shape in, as the framework's own
/// error renderer declares it.
pub static TYPE_MEMBER_SCHEMA: Schema = Schema::new_member(
    ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
    ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// The shape name the discriminator carries for an action type's identity.
const ACTION_TYPE_ID_SHAPE: &str = "ActionTypeId";

/// The shape name the discriminator carries for an artifact store's encryption key.
const ENCRYPTION_KEY_SHAPE: &str = "EncryptionKey";

/// Every member of this model whose value is a structure that carries the discriminator,
/// paired with the shape name it carries.
///
/// Keyed by the MEMBER's shape id, which is what the codec hands a serializer for a nested
/// structure — a generated member schema names no target — so the catalog lists each
/// member that targets one of the two shapes rather than the shapes themselves. That is
/// every place this model binds one: an action type's identity in a catalog entry
/// (`ActionType$id`), in a pipeline's own structure (`ActionDeclaration$actionTypeId`), in
/// an action execution's input (`ActionExecutionInput$actionTypeId`) and in the data a job
/// worker is handed (`JobData` / `ThirdPartyJobData`); an encryption key in an artifact
/// store (`ArtifactStore$encryptionKey`) and in that same job data.
static DISCRIMINATED_MEMBERS: &[(ShapeId, &str)] = &[
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#ActionType$id",
            "com.amazonaws.codepipeline",
            "id",
        ),
        ACTION_TYPE_ID_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#ActionDeclaration$actionTypeId",
            "com.amazonaws.codepipeline",
            "actionTypeId",
        ),
        ACTION_TYPE_ID_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#ActionExecutionInput$actionTypeId",
            "com.amazonaws.codepipeline",
            "actionTypeId",
        ),
        ACTION_TYPE_ID_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#JobData$actionTypeId",
            "com.amazonaws.codepipeline",
            "actionTypeId",
        ),
        ACTION_TYPE_ID_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#ThirdPartyJobData$actionTypeId",
            "com.amazonaws.codepipeline",
            "actionTypeId",
        ),
        ACTION_TYPE_ID_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#ArtifactStore$encryptionKey",
            "com.amazonaws.codepipeline",
            "encryptionKey",
        ),
        ENCRYPTION_KEY_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#JobData$encryptionKey",
            "com.amazonaws.codepipeline",
            "encryptionKey",
        ),
        ENCRYPTION_KEY_SHAPE,
    ),
    (
        ShapeId::from_static(
            "com.amazonaws.codepipeline#ThirdPartyJobData$encryptionKey",
            "com.amazonaws.codepipeline",
            "encryptionKey",
        ),
        ENCRYPTION_KEY_SHAPE,
    ),
];

/// The shape name the structure bound to `member` carries, or `None` for a member whose
/// value carries no discriminator.
///
/// A linear scan of [`DISCRIMINATED_MEMBERS`], which is a fixed handful of entries: the
/// members of ONE model that bind a discriminated shape.
pub fn discriminator_of(member: &ShapeId) -> Option<&'static str> {
    DISCRIMINATED_MEMBERS
        .iter()
        .find(|(bound, _)| bound == member)
        .map(|(_, shape)| *shape)
}
