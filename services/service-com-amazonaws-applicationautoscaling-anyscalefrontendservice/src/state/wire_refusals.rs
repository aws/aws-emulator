// SPDX-License-Identifier: CC0-1.0

//! The refusals this service answers whose wire identity its own error list does not
//! express, and the error channel that carries them.
//!
//! Two things about an answer are decided by the MODEL for a modelled error: the shape (and
//! with it the `__type` a client branches on) and the `@httpError` status. For the refusals
//! below the service answers something else, so they cannot be constructed as a plain
//! variant of an operation's `#[operation_error]` enum:
//!
//! * the ARN-addressed tag operations answer an unresolvable `ResourceARN` with
//!   `ResourceNotFoundException`'s own body at the 400 of a client error, not at the 404 its
//!   `@httpError` declares ([`WireRefusal::ScalableTargetNotFound`]);
//! * an operation this service does not offer for the namespace addressed is refused
//!   outright, as an action that is not available rather than as a problem with the request
//!   ([`WireRefusal::NotSupported`]) — a shape no operation of this service lists.
//!
//! Both live here, once, rather than in each operation's file: what a refusal looks like on
//! the wire is a property of the service, and the operations that answer one must not word
//! the same condition two ways. An operation carries them by declaring its error channel
//! [`Refusable`], which widens the operation's own modelled channel without changing how any
//! modelled error is rendered.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::smithy_server_runtime::OperationError;

use crate::ops::types::ResourceNotFoundException;

/// The status a tag operation answers an unresolvable `ResourceARN` with: the 4xx of a client
/// error rather than the 404 the modelled shape's `@httpError` declares. Clients and the
/// proxies in front of them branch on the status CLASS before they parse a body, and the two
/// are classified differently — a 404 as "no such endpoint or route", retried and surfaced
/// unlike a client error — so the status is part of the answer, not a detail of it.
const TARGET_NOT_FOUND_STATUS: u16 = 400;

/// The `__type` a resource that does not exist is named by, which is the modelled shape's own
/// name: only the STATUS of this refusal differs from the model's declaration, and a client
/// branches on the code.
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The `__type` an access-denied refusal is named by, as the AWS "Common Errors" table names
/// it (`AccessDeniedException` — "You do not have sufficient access to perform this action").
///
/// Read by every refusal of this service that is about the caller's ACCESS rather than about
/// the request it sent: an operation this service does not offer for the resource addressed
/// ([`WireRefusal::NotSupported`]), and a request the IAM policies in force do not permit
/// ([`crate::service::authorization`]) — so the two cannot answer one condition under two
/// codes.
pub const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status of such a refusal, as the same table publishes it: "HTTP Status Code: 400".
pub const ACCESS_DENIED_STATUS: u16 = 400;

/// How this service states that one of its operations is not offered for the resource
/// addressed: the operation's own name, and nothing else from the request.
pub fn not_supported_message(operation: &str) -> String {
    format!("{operation} is not supported.")
}

/// The `Message` member of the not-supported refusal's shape.
///
/// Spelled `Message`: this service speaks `awsJson1_1`, whose codec takes the wire key from
/// the member NAME, every error shape this service's own model declares carries its sentence
/// under that capitalized key, and so does the AWS common-error envelope. A lowercase name
/// would answer a refusal whose message no client reading either can find.
static NOT_SUPPORTED_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException$Message",
        "smithy.framework",
        "Message",
    ),
    ShapeType::String,
    "Message",
    0,
)
.with_xml_name("Message")
.with_json_name("Message");

/// Schema for the not-supported refusal's shape.
///
/// In the `smithy.framework` namespace for the same reason as the scaffold's synthetic shapes
/// (`crate::scaffolded`): this shape is in no service's model, so it must not collide with a
/// modelled one. The wire `__type` comes from [`OperationError::shape_name`], not from here.
static NOT_SUPPORTED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException",
        "smithy.framework",
        "AccessDeniedException",
    ),
    ShapeType::Structure,
    &[&NOT_SUPPORTED_MESSAGE],
);

/// The body of a not-supported refusal: the one sentence it states.
#[derive(Debug, Clone)]
pub struct NotSupported {
    /// The sentence — [`not_supported_message`].
    pub message: String,
}

impl SerializableStruct for NotSupported {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&NOT_SUPPORTED_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// A refusal this service words itself, carrying the ONE mapping from each refusal class to
/// the wire code, the HTTP status and the body it answers with.
#[derive(Debug)]
pub enum WireRefusal {
    /// A `ResourceARN` a tag operation cannot resolve to a registered scalable target, whether
    /// or not this service ever minted that ARN: either the target is registered or it is not,
    /// and the refusal says so. The READ and the two MUTATIONS answer that condition alike —
    /// the ARN names the resource the call addresses, so one that identifies no target is the
    /// resource reported absent.
    ScalableTargetNotFound(ResourceNotFoundException),
    /// An operation this service does not offer for the resource addressed.
    NotSupported(NotSupported),
}

impl WireRefusal {
    /// The refusal for a `resource_arn` naming no registered scalable target, reporting that
    /// ARN in the modelled `ResourceName` member — "the name of the Application Auto Scaling
    /// resource. This value is an Amazon Resource Name (ARN)" — rather than spliced into the
    /// sentence.
    pub fn scalable_target_not_found(resource_arn: &str) -> Self {
        Self::ScalableTargetNotFound(ResourceNotFoundException {
            message: Some(super::SCALABLE_TARGET_NOT_FOUND_MESSAGE.to_string()),
            resource_name: Some(resource_arn.to_string()),
        })
    }

    /// The refusal of `operation`, which this service does not offer for the resource the
    /// request addressed.
    pub fn not_supported(operation: &str) -> Self {
        Self::NotSupported(NotSupported {
            message: not_supported_message(operation),
        })
    }

    /// The body this refusal serializes: for a modelled shape, the shape itself, so its own
    /// schema decides which members the answer carries.
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::ScalableTargetNotFound(e) => e,
            Self::NotSupported(e) => e,
        }
    }

    /// The schema the answer is written against.
    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::ScalableTargetNotFound(_) => {
                &crate::ops::types::SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION
            }
            Self::NotSupported(_) => &NOT_SUPPORTED_SCHEMA,
        }
    }

    /// The code on the wire, which the framework reads from an operation's `shape_name`
    /// rather than from the schema.
    fn error_code(&self) -> &'static str {
        match self {
            Self::ScalableTargetNotFound(_) => RESOURCE_NOT_FOUND_CODE,
            Self::NotSupported(_) => ACCESS_DENIED_CODE,
        }
    }

    /// The HTTP status this refusal answers with.
    fn http_status(&self) -> u16 {
        match self {
            Self::ScalableTargetNotFound(_) => TARGET_NOT_FOUND_STATUS,
            Self::NotSupported(_) => ACCESS_DENIED_STATUS,
        }
    }

    /// The sentence the body states, for the [`std::fmt::Display`] rendering below.
    fn message(&self) -> &str {
        match self {
            Self::ScalableTargetNotFound(e) => e.message.as_deref().unwrap_or_default(),
            Self::NotSupported(e) => &e.message,
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
/// `Channel` is the operation's own channel — its modelled errors, the scaffold's 501 and the
/// framework's synthetic `ValidationException` — rendered exactly as it is rendered without
/// this wrapper. `Refused` is one of the service's own refusals, rendered by [`WireRefusal`].
/// An operation declares `type Error = Refusable<{Op}ErrorModeled>` and keeps writing
/// `Err(SomeModeledError.into())`.
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

/// Every method delegates: the operation's own channel renders a modelled error exactly as it
/// would without this wrapper, and a refusal renders as [`WireRefusal`] states it — so an
/// operation that carries this channel answers one of those conditions exactly as its
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
            // Neither refusal's shape carries an `awsQueryError` trait to honour: the
            // modelled `ResourceNotFoundException` declares none, and the not-supported
            // shape is in no model.
            Self::Refused(_) => None,
        }
    }
}
