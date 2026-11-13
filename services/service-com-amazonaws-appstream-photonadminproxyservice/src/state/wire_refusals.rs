// SPDX-License-Identifier: CC0-1.0

//! The refusals this service answers whose wire identity its own error list does not
//! express, and the error channel that carries them.
//!
//! Two things about an answer are decided by the MODEL for a modelled error: the shape —
//! and with it the `__type` a client branches on — and the `@httpError` status. AppStream
//! refuses a value one of its members may not carry as `InvalidParameterValueException`,
//! a shape this service's model declares for no operation, so that refusal cannot be
//! constructed as a plain variant of an operation's `#[operation_error]` enum. It lives
//! here, once, rather than in each operation's file: what a refusal looks like on the
//! wire is a property of the service, and two operations must not word one condition two
//! ways. An operation carries it by declaring its error channel [`Refusable`], which
//! widens the operation's own modelled channel without changing how any modelled error is
//! rendered.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::smithy_server_runtime::OperationError;

/// The `__type` AppStream names a refused member value by, as its own API reference
/// publishes that common error ("InvalidParameterValueException — One or more parameter
/// values are not valid").
pub const INVALID_PARAMETER_VALUE_CODE: &str = "InvalidParameterValueException";

/// The status such a refusal answers with: the 400 of a client error, the status every
/// error shape this service's model declares for a bad request carries.
pub const INVALID_PARAMETER_VALUE_STATUS: u16 = 400;

/// The `message` member of the refused-value shape.
///
/// Spelled in lower case, unlike the capitalised `Message` of every error shape this
/// service's model DECLARES: this shape is in no model, so the sentence is carried under
/// the key the service states a non-modelled refusal's message under, which the recorded
/// refusal of a member value this service may not act on shows to be `message`.
static INVALID_PARAMETER_VALUE_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#InvalidParameterValueException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("message")
.with_json_name("message");

/// Schema for the refused-value shape.
///
/// In the `smithy.framework` namespace for the same reason as the scaffold's synthetic
/// shapes (`crate::scaffolded`): the shape is in no service's model, so it must not
/// collide with a modelled one. The wire `__type` comes from
/// [`OperationError::shape_name`], not from here.
static INVALID_PARAMETER_VALUE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#InvalidParameterValueException",
        "smithy.framework",
        "InvalidParameterValueException",
    ),
    ShapeType::Structure,
    &[&INVALID_PARAMETER_VALUE_MESSAGE],
);

/// The body of such a refusal: the one sentence it states.
#[derive(Debug, Clone)]
pub struct InvalidParameterValue {
    pub message: String,
}

impl SerializableStruct for InvalidParameterValue {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&INVALID_PARAMETER_VALUE_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// A refusal this service words itself, carrying the ONE mapping from the refusal class to
/// the wire code, the HTTP status and the body it answers with.
#[derive(Debug)]
pub enum WireRefusal {
    /// A member value this service may not act on — an application whose `Platforms` do
    /// not include the platform of the fleet it is asked to stream on.
    InvalidParameterValue(InvalidParameterValue),
}

impl WireRefusal {
    /// The refusal of a member value, stating `message`.
    pub fn invalid_parameter_value(message: impl Into<String>) -> Self {
        Self::InvalidParameterValue(InvalidParameterValue {
            message: message.into(),
        })
    }

    /// The body this refusal serializes.
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::InvalidParameterValue(e) => e,
        }
    }

    /// The schema the answer is written against.
    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::InvalidParameterValue(_) => &INVALID_PARAMETER_VALUE_SCHEMA,
        }
    }

    /// The code on the wire, which the framework reads from an operation's `shape_name`
    /// rather than from the schema.
    fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidParameterValue(_) => INVALID_PARAMETER_VALUE_CODE,
        }
    }

    /// The HTTP status this refusal answers with.
    fn http_status(&self) -> u16 {
        match self {
            Self::InvalidParameterValue(_) => INVALID_PARAMETER_VALUE_STATUS,
        }
    }

    /// The sentence the body states, for the [`std::fmt::Display`] rendering below.
    fn message(&self) -> &str {
        match self {
            Self::InvalidParameterValue(e) => &e.message,
        }
    }
}

impl ::std::fmt::Display for WireRefusal {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.message())
    }
}

impl ::std::error::Error for WireRefusal {}

/// One operation's error channel, widened by the refusals above.
///
/// `Channel` is the operation's own channel — its modelled errors, the scaffold's 501 and
/// the framework's synthetic `ValidationException` — rendered exactly as it is rendered
/// without this wrapper. `Refused` is one of the service's own refusals, rendered by
/// [`WireRefusal`]. An operation declares `type Error = Refusable<{Op}ErrorModeled>` and
/// keeps writing `Err(SomeModeledError.into())`.
#[derive(Debug)]
pub enum Refusable<E> {
    /// The operation's own error channel.
    Channel(crate::scaffolded::Scaffolded<E>),
    /// A refusal this service words itself.
    Refused(WireRefusal),
}

impl<E> From<crate::scaffolded::Scaffolded<E>> for Refusable<E> {
    fn from(channel: crate::scaffolded::Scaffolded<E>) -> Self {
        Self::Channel(channel)
    }
}

impl<E> From<WireRefusal> for Refusable<E> {
    fn from(refusal: WireRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for Refusable<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Channel(e) => write!(f, "{e}"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
        }
    }
}

/// `source()` forwards to the wrapped failure rather than taking the default `None`.
impl<E: ::std::error::Error + 'static> ::std::error::Error for Refusable<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Channel(e) => e.source(),
            Self::Refused(refusal) => refusal.source(),
        }
    }
}

/// Every method delegates: the operation's own channel renders a modelled error exactly as
/// it would without this wrapper, and a refusal renders as [`WireRefusal`] states it — so
/// an operation that carries this channel answers one of those conditions exactly as its
/// siblings do.
impl<E: OperationError> OperationError for Refusable<E> {
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::Channel(e) => e.as_serializable(),
            Self::Refused(refusal) => refusal.as_serializable(),
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::Channel(e) => e.variant_schema(),
            Self::Refused(refusal) => refusal.variant_schema(),
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Channel(e) => e.shape_name(),
            Self::Refused(refusal) => refusal.error_code(),
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Channel(e) => e.http_status(),
            Self::Refused(refusal) => refusal.http_status(),
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Channel(e) => e.query_error_code(),
            // The refusal's shape is in no model, so there is no `awsQueryError` trait to
            // honour — and this service speaks no query protocol.
            Self::Refused(_) => None,
        }
    }
}
