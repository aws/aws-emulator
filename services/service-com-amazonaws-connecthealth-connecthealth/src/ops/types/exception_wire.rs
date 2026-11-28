// SPDX-License-Identifier: CC0-1.0

//! The exception shapes this service's model declares, and the WIRE KEY an error body of
//! this service carries its detail text under.
//!
//! HAND-WRITTEN, and declared by `src/service.rs` as [`crate::service::exception_wire`]
//! rather than by the `types/mod.rs` beside it: that file is rendered from the model and
//! rewritten by every model update, so a declaration in it does not survive one. An
//! operation that answers with these shapes names them explicitly
//! (`use crate::service::exception_wire::{…}`), which shadows the model-keyed copies the
//! generated `types` re-exports — those cannot be told which key a plane writes the detail
//! text under, which is the whole reason these seven exist.
//!
//! Every one of them declares exactly one member, `message` targeting `String`, with no
//! `@jsonName`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,ValidationException,AccessDeniedException,InternalServerException,ServiceQuotaExceededException,ThrottlingException,ConflictException",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
//! The key that member's text goes out under, though, is NOT decided by the shape: it is
//! decided by the PLANE that answered the call ([`AnsweringPlane`]).
//!
//! RECORDED behaviour, and the reason this module exists: ConnectHealth is fronted by more
//! than one backend, and they do not key that text alike. Every recorded refusal of a
//! SUBSCRIPTION call — the reader, the listing and the status transitions under
//! `/domains/{domainId}/subscriptions…` — carries it under `Message`, while every recorded
//! refusal of a DOMAIN call (`/domain/…`) and of the Medical Scribe streaming endpoint
//! carries it under `message`, the name the model spells. So the key a client reads is
//! whatever the plane that answered wrote, and each exception shape below therefore
//! carries the PAIR of member schemas `exception_shape!` emits, with the handler's
//! operation picking between them ([`Keyed::answered_by`]).
//!
//! The restJson1 codec takes a member's wire key from its `@jsonName`, falling back to the
//! member's Smithy name (`vendor/smithy-schema-server/src/json.rs`), which the
//! `#[smithy_macros::smithy(shape = …)]` expansion derives from the model and cannot be
//! told — which is why the seven shapes are hand-written here. One macro emits exactly
//! what that expansion emits for an `@error` structure: the member statics, the `SCHEMA_*`
//! static each operation's error enum references, `shape_name`, `Display`, `Error` and
//! `SerializableStruct`. Declaring them ONCE, together, is what keeps one rule for all of
//! them: an exception added to this service's model is added here and is keyed by the same
//! plane rule rather than by whichever handler answers it.
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//! x-amzn-errortype: ResourceNotFoundException
//!
//! {"__type":"ResourceNotFoundException","Message":"Subscription not found: dom-0123456789abcdef0123/sub-000000000000000000000"}
//! ```
//!
//! Nothing else about an answer is decided here: the status, the `__type` discriminator and
//! the `x-amzn-errortype` header stay the framework's rendering of the modelled shape, and
//! the SENTENCE each exception carries is built in [`crate::service::error_detail`]. One
//! refusal class is not rendered by this service at all — a request the framework's
//! boundary walk refuses before a handler runs renders in the framework's own protocol
//! envelope (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), which names the member it
//! writes itself and is reached by no service seam; what a HANDLER answers is what this
//! module keys.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};

/// The key the plane that answers a SUBSCRIPTION call writes an exception's detail text
/// under, which is not the member name the model spells.
pub const CAPITALISED_DETAIL_KEY: &str = "Message";

/// Which of ConnectHealth's backends answered a call — all this service's error bodies
/// differ by, and the one thing that decides the wire key the detail text travels under.
///
/// It is NOT a modelled member of any shape and never reaches the wire itself; it selects
/// which of the two member schemas of a shape the text is written through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AnsweringPlane {
    /// Keys the text as the model names the member: `message`.
    ///
    /// RECORDED for the domain plane (`/domain/…`) and the Medical Scribe streaming
    /// endpoint, and the DEFAULT for every other plane — the ARN-addressed tagging calls
    /// and the patient-insights runtime endpoint — because a refusal says nothing about a
    /// plane unless it is answered by one that keys the text differently.
    #[default]
    ModelMemberName,
    /// The SUBSCRIPTION plane (`/domains/{domainId}/subscriptions…`: the reader, the
    /// listing, the create and the two status transitions), which keys it
    /// [`CAPITALISED_DETAIL_KEY`].
    ///
    /// RECORDED: every refusal of a subscription call carries its detail text there,
    /// whichever error class answered — a subscription that cannot be resolved, a page
    /// size outside the modelled range, a parent domain that cannot be resolved.
    Subscriptions,
}

impl AnsweringPlane {
    /// The member schema this plane writes the detail text through: one of the pair
    /// `exception_shape!` emits for an error shape of this service.
    fn schema<'s>(self, model_member_name: &'s Schema, capitalised: &'s Schema) -> &'s Schema {
        match self {
            Self::ModelMemberName => model_member_name,
            Self::Subscriptions => capitalised,
        }
    }
}

/// An exception of this service, told which plane is answering with it.
///
/// A handler builds the shape with the sentence alone — the plane is not what a refusal is
/// ABOUT — and the operation that answers states its plane once, so the two cannot drift
/// apart per refusal.
pub trait Keyed: Sized {
    /// This exception, keyed as `plane` keys an error body.
    fn answered_by(self, plane: AnsweringPlane) -> Self;
}

/// Key every exception of ONE operation's modelled error enum by the plane that answers
/// that operation.
///
/// An operation states its plane in one place — the `From<{Op}ErrorModeled>` impl its own
/// file carries — and this walks whichever variants that operation's model declares, so a
/// handler builds a refusal from the sentence alone and no two refusals of one operation
/// can be keyed differently. `$enum` is the operation's modelled error enum and
/// `[$variant…]` its variants, which is what makes this exhaustive per operation: a
/// variant the model adds and this list omits fails to compile.
macro_rules! answered_by_plane {
    ($enum:ident, $modeled:expr, $plane:expr, [$($variant:ident),+ $(,)?]) => {
        match $modeled {
            $(
                $enum::$variant(exception) => $enum::$variant(
                    $crate::service::exception_wire::Keyed::answered_by(exception, $plane),
                ),
            )+
        }
    };
}

pub(crate) use answered_by_plane;

/// Emit one exception shape of this service: the `message` member in the two keyings a
/// plane of this service writes it under, the shape's schema, and the struct an
/// operation's error enum carries.
///
/// `$shape` is the shape's local name, which is also its wire `__type`; the shape id is
/// built from this service's one namespace. The member is at index 0 because `message` is
/// the only member any of these shapes declares, and both keyings carry the model's own
/// shape id and member name — only the WIRE key differs.
macro_rules! exception_shape {
    (
        $(#[$shape_doc:meta])*
        $shape:literal => $struct:ident, $member_schema:ident, $capitalised:ident, $schema:ident
    ) => {
        /// The `message` member keyed as the model names it.
        static $member_schema: Schema = Schema::new_member(
            ShapeId::from_static(
                concat!("com.amazonaws.connecthealth#", $shape, "$message"),
                "com.amazonaws.connecthealth",
                "message",
            ),
            ShapeType::String,
            "message",
            0,
        )
        // The casing the XML error envelopes use, exactly as the model-derived expansion
        // emits it for a member named `message`.
        .with_xml_name(CAPITALISED_DETAIL_KEY);

        /// The same member keyed as the subscription plane keys it.
        static $capitalised: Schema = Schema::new_member(
            ShapeId::from_static(
                concat!("com.amazonaws.connecthealth#", $shape, "$message"),
                "com.amazonaws.connecthealth",
                "message",
            ),
            ShapeType::String,
            "message",
            0,
        )
        .with_xml_name(CAPITALISED_DETAIL_KEY)
        // THE wire key on restJson1, which reads `@jsonName` before the member name.
        .with_json_name(CAPITALISED_DETAIL_KEY);

        /// The shape's schema, as each operation's `#[operation_error]` enum references it
        /// for this variant. Its member list is the MODEL's: which key the text is written
        /// under is chosen per answer and is not a second member of the shape.
        pub static $schema: Schema = Schema::new_struct(
            ShapeId::from_static(
                concat!("com.amazonaws.connecthealth#", $shape),
                "com.amazonaws.connecthealth",
                $shape,
            ),
            ShapeType::Structure,
            &[&$member_schema],
        );

        $(#[$shape_doc])*
        #[derive(Debug, Clone, Default)]
        pub struct $struct {
            /// The model's one member: the detail text of the refusal, built in
            /// [`crate::service::error_detail`].
            pub message: Option<String>,
            /// NOT a modelled member: which plane is answering, and so which wire key
            /// [`Self::message`] travels under ([`AnsweringPlane`]).
            pub plane: AnsweringPlane,
        }

        impl $struct {
            /// The wire error code (`__type`, and the `x-amzn-errortype` header): the
            /// model's shape name, unchanged.
            pub const fn shape_name() -> &'static str {
                $shape
            }
        }

        impl Keyed for $struct {
            fn answered_by(mut self, plane: AnsweringPlane) -> Self {
                self.plane = plane;
                self
            }
        }

        impl ::std::fmt::Display for $struct {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(f, "{}", $shape)?;
                if let Some(message) = &self.message {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
        }

        impl ::std::error::Error for $struct {}

        impl SerializableStruct for $struct {
            fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
                if let Some(message) = self.message.as_ref() {
                    s.write_string(
                        self.plane.schema(&$member_schema, &$capitalised),
                        message.as_str(),
                    )?;
                }
                Ok(())
            }
        }
    };
}

exception_shape! {
    /// This error is thrown when the client does not supply proper credentials to the API
    /// (`@error client`, `@httpError 401`).
    "AccessDeniedException" => AccessDeniedException,
    ACCESS_DENIED_EXCEPTION_M_MESSAGE,
    ACCESS_DENIED_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_ACCESS_DENIED_EXCEPTION
}

exception_shape! {
    /// This error is thrown when a resource update is no longer valid due to assumptions
    /// about initial state changing (`@error client`, `@httpError 409`).
    "ConflictException" => ConflictException,
    CONFLICT_EXCEPTION_M_MESSAGE,
    CONFLICT_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_CONFLICT_EXCEPTION
}

exception_shape! {
    /// This error is thrown when a transient error causes our API to fail (`@error
    /// server`, `@httpError 500`).
    "InternalServerException" => InternalServerException,
    INTERNAL_SERVER_EXCEPTION_M_MESSAGE,
    INTERNAL_SERVER_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_INTERNAL_SERVER_EXCEPTION
}

exception_shape! {
    /// This error is thrown when the requested resource is not found (`@error client`,
    /// `@httpError 404`).
    "ResourceNotFoundException" => ResourceNotFoundException,
    RESOURCE_NOT_FOUND_EXCEPTION_M_MESSAGE,
    RESOURCE_NOT_FOUND_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION
}

exception_shape! {
    /// The request exceeds a service quota (`@error client`, `@httpError 402`).
    "ServiceQuotaExceededException" => ServiceQuotaExceededException,
    SERVICE_QUOTA_EXCEEDED_EXCEPTION_M_MESSAGE,
    SERVICE_QUOTA_EXCEEDED_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_SERVICE_QUOTA_EXCEEDED_EXCEPTION
}

exception_shape! {
    /// This error is thrown when the client exceeds the allowed request rate (`@error
    /// client`, `@httpError 429`, `@retryable{throttling}`).
    "ThrottlingException" => ThrottlingException,
    THROTTLING_EXCEPTION_M_MESSAGE,
    THROTTLING_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_THROTTLING_EXCEPTION
}

exception_shape! {
    /// This error is thrown when the client supplies invalid input to the API (`@error
    /// client`, `@httpError 400`).
    "ValidationException" => ValidationException,
    VALIDATION_EXCEPTION_M_MESSAGE,
    VALIDATION_EXCEPTION_M_MESSAGE_CAPITALISED,
    SCHEMA_VALIDATION_EXCEPTION
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every exception shape of this service, with the pair of member schemas its detail
    /// text is written through — one row per shape the model declares, so a shape added
    /// without the pair fails here rather than only on the wire.
    fn shapes() -> Vec<(&'static str, &'static Schema, &'static Schema, &'static Schema)> {
        vec![
            (
                "AccessDeniedException",
                &SCHEMA_ACCESS_DENIED_EXCEPTION,
                &ACCESS_DENIED_EXCEPTION_M_MESSAGE,
                &ACCESS_DENIED_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
            (
                "ConflictException",
                &SCHEMA_CONFLICT_EXCEPTION,
                &CONFLICT_EXCEPTION_M_MESSAGE,
                &CONFLICT_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
            (
                "InternalServerException",
                &SCHEMA_INTERNAL_SERVER_EXCEPTION,
                &INTERNAL_SERVER_EXCEPTION_M_MESSAGE,
                &INTERNAL_SERVER_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
            (
                "ResourceNotFoundException",
                &SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
                &RESOURCE_NOT_FOUND_EXCEPTION_M_MESSAGE,
                &RESOURCE_NOT_FOUND_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
            (
                "ServiceQuotaExceededException",
                &SCHEMA_SERVICE_QUOTA_EXCEEDED_EXCEPTION,
                &SERVICE_QUOTA_EXCEEDED_EXCEPTION_M_MESSAGE,
                &SERVICE_QUOTA_EXCEEDED_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
            (
                "ThrottlingException",
                &SCHEMA_THROTTLING_EXCEPTION,
                &THROTTLING_EXCEPTION_M_MESSAGE,
                &THROTTLING_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
            (
                "ValidationException",
                &SCHEMA_VALIDATION_EXCEPTION,
                &VALIDATION_EXCEPTION_M_MESSAGE,
                &VALIDATION_EXCEPTION_M_MESSAGE_CAPITALISED,
            ),
        ]
    }

    /// RULE: a shape keeps the MODEL's own identity — its shape name, its one member and
    /// that member's name — whichever plane answers with it. Only the wire key differs.
    #[test]
    fn every_exception_shape_keeps_the_models_own_identity() {
        for (case, schema, model_member_name, capitalised) in shapes() {
            assert_eq!(
                schema.shape_id().shape_name(),
                case,
                "case {case:?}: the shape keeps the model's own name, got {:?}",
                schema.shape_id().shape_name()
            );
            let members = schema.members();
            assert_eq!(
                members.len(),
                1,
                "case {case:?}: the model declares exactly one member, got {}",
                members.len()
            );
            for (keying, member) in [
                ("as the model names it", model_member_name),
                ("as the subscription plane keys it", capitalised),
            ] {
                assert_eq!(
                    member.member_name(),
                    Some("message"),
                    "case {case:?} ({keying}): the member keeps the model's own name, got {:?}",
                    member.member_name()
                );
                assert_eq!(
                    member.shape_id().to_string(),
                    format!("com.amazonaws.connecthealth#{case}$message"),
                    "case {case:?} ({keying}): the member keeps the model's own shape id, \
                     got {:?}",
                    member.shape_id().to_string()
                );
            }
        }
    }

    /// RULE: the plane that answers decides the wire key — the member name the model
    /// spells for every plane but one, and `Message` for the subscription plane — and the
    /// choice is the same for every exception shape of this service.
    #[test]
    fn the_answering_plane_decides_the_wire_key() {
        for (case, _, model_member_name, capitalised) in shapes() {
            for (plane, expected) in [
                (AnsweringPlane::ModelMemberName, None),
                (
                    AnsweringPlane::Subscriptions,
                    Some(CAPITALISED_DETAIL_KEY),
                ),
            ] {
                let chosen = plane.schema(model_member_name, capitalised);
                let key = chosen.json_name().map(|name| name.value());
                assert_eq!(
                    key, expected,
                    "case {case:?} ({plane:?}): expected the detail text keyed {expected:?}, \
                     got {key:?}"
                );
                // With no `@jsonName`, restJson1 falls back to the member's Smithy name,
                // so a plane that overrides nothing writes `message`.
                assert_eq!(
                    key.unwrap_or_else(|| chosen.member_name().unwrap_or_default()),
                    match plane {
                        AnsweringPlane::ModelMemberName => "message",
                        AnsweringPlane::Subscriptions => CAPITALISED_DETAIL_KEY,
                    },
                    "case {case:?} ({plane:?}): the wire key must be the plane's own"
                );
            }
        }
    }

    /// RULE: a handler builds the sentence and the operation states the plane, so
    /// [`Keyed::answered_by`] is what moves an exception from one keying to the other and
    /// changes nothing else about it.
    #[test]
    fn answering_by_a_plane_changes_only_the_keying() {
        let built = ResourceNotFoundException {
            message: Some("Subscription not found: dom-0123456789abcdef0123/sub-0".to_string()),
            ..Default::default()
        };
        assert_eq!(
            built.plane,
            AnsweringPlane::ModelMemberName,
            "case \"a shape built with the sentence alone\": expected the model's own \
             keying, got {:?}",
            built.plane
        );
        let keyed = built.clone().answered_by(AnsweringPlane::Subscriptions);
        assert_eq!(
            keyed.plane,
            AnsweringPlane::Subscriptions,
            "case \"answered by the subscription plane\": expected that plane's keying, \
             got {:?}",
            keyed.plane
        );
        assert_eq!(
            keyed.message, built.message,
            "case \"answered by the subscription plane\": the sentence must be unchanged, \
             got {:?}",
            keyed.message
        );
    }
}
