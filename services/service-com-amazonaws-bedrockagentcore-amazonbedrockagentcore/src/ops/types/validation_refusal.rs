// SPDX-License-Identifier: CC0-1.0

//! The payload of this service's modelled 400 `ValidationException`, for the two
//! operations that answer it in both of its shapes.
//!
//! WHY THIS MODULE EXISTS. `ValidationException` carries `message`, `reason` and
//! `fieldList` (`smithy_query(query="shape", shape="ValidationException",
//! model_path="services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/\
//! smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json")`). `reason`
//! classifies which request MEMBER failed its constraints and `fieldList` names those
//! members, so a refusal no request member is at fault for — the session the call
//! addresses is in a state that no longer permits it — has nothing to put in either and
//! carries `message` alone, while a refusal a member IS at fault for carries the
//! classification with it. The derived struct's `reason` is not an `Option`, so the
//! derived serializer always writes it; this enum is what lets one operation answer both
//! bodies with the same modelled shape id, status and error code.
//!
//! Hand-authored and in its own module rather than in `types/shared_types.rs`: it is not
//! rendered from the model, and a generated types file is rewritten by the next model
//! update.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};

use super::ValidationException;

/// The `message` member of the modelled `ValidationException`, declared here because the
/// derive emits its member schemas as private statics of the module that declares the
/// struct. Same shape id, wire name, type and member index (`message` is the shape's first
/// member) as the derived one, so one body is not serialized differently from the other.
///
/// `with_xml_name("Message")` matches what the derive puts on an error shape's `message`
/// member; this service's only protocol is restJson1, which reads the member name.
static VALIDATION_EXCEPTION_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.bedrockagentcore#ValidationException$message",
        "com.amazonaws.bedrockagentcore",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Which body a modelled `ValidationException` carries.
#[derive(Debug, Clone)]
pub enum ValidationRefusal {
    /// A refusal a request member is at fault for: the modelled shape, member for
    /// member, with `reason` classifying the member-level failure.
    Field(ValidationException),
    /// A refusal over the state of the resource the request addresses: `message` alone,
    /// since no member is at fault for it.
    ResourceState(String),
}

impl ValidationRefusal {
    /// A refusal over the state of the resource the request addresses, carrying
    /// `message` alone.
    pub fn resource_state(message: impl Into<String>) -> Self {
        Self::ResourceState(message.into())
    }

    /// The sentence the refusal carries, whichever body it is.
    pub fn message(&self) -> &str {
        match self {
            Self::Field(modelled) => modelled.message.as_str(),
            Self::ResourceState(message) => message.as_str(),
        }
    }
}

impl std::fmt::Display for ValidationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ValidationException: {}", self.message())
    }
}

impl std::error::Error for ValidationRefusal {}

/// A field refusal serializes through the derived serializer, so it carries every member
/// the modelled shape declares; a state refusal writes `message` alone.
impl SerializableStruct for ValidationRefusal {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        match self {
            Self::Field(modelled) => modelled.serialize_members(s),
            Self::ResourceState(message) => {
                s.write_string(&VALIDATION_EXCEPTION_MESSAGE, message.as_str())
            }
        }
    }
}
