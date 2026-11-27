// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-invoicing-invoicing` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Six opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. This service returns `Some` for every operation it
//!   serves, so that a refused input is stated in the modelled
//!   `ValidationException`, the tag plane announces its own media type, and a failure
//!   reports its text and the resource it is about in the members this service's wire
//!   carries them in (see [`FrontDoor`]), and for a fetch of an invoice document, which
//!   is not an API call at all (see [`invoice_document`]).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording; this service words it itself, through the one formatter in
//!   [`validation`] that also words the modelled refusal body [`FrontDoor`] answers
//!   with. See the inline note on that method.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//! - `override_protocol` — the codec this service's own responses are serialized
//!   in. The default returns `None`, keeping the model-derived protocol; this
//!   service overrides it (see "The media type this service announces" below).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The media types this service announces
//!
//! The model declares the `awsJson1_0` protocol trait, but every recorded response of
//! an INVOICE-UNIT operation — a success and a modelled failure its handler raises
//! alike — announces `application/x-amz-json-1.1`, and the media type a client reads
//! is what it keys content negotiation, deserializer selection and any protocol
//! assertion off. `override_protocol` below therefore serves the awsJson `1.1` codec
//! service-wide: the JSON framing is byte-identical to `1.0`, so a request sent with
//! either media type still reads, and only the announced media type changes.
//!
//! The TAG operations — `ListTagsForResource`, `TagResource`, `UntagResource` — are a
//! different plane of this API, and their recorded responses announce the awsJson
//! `1.0` media type the model's protocol trait declares. [`FrontDoor`] therefore
//! stamps [`TAG_PLANE_MEDIA_TYPE`] on what they answer, so each plane announces the
//! media type its own recorded responses carry.
//!
//! A refusal of the REQUEST is not a response the service produced — the input
//! validator in FRONT of the service answers it — so the routing, transport and
//! refusal envelopes keep the media type the model's protocol trait declares, which
//! is the one seam `override_protocol` deliberately does not reach, and which is the
//! media type the recorded refusals carry too.

pub(crate) mod error_body_members;
pub(crate) mod invoice_document;
pub(crate) mod validation;

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused,
    ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

/// The `com-amazonaws-invoicing-invoicing` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec this service answers in (awsJson `1.1`; see the module docs). Built
    /// once here because [`Service::override_protocol`] hands back a borrow of it.
    protocol: aws_smithy_schema_server::rpc::HttpRpcServerProtocol<
        aws_smithy_schema_server::json::JsonCodec,
    >,
    /// Answers the operations this service serves in front of the generated walk; see
    /// [`FrontDoor`].
    front_door: FrontDoor<Eng>,
    /// Serves a download of an invoice's documents; [`Service::override_operation`] hands
    /// it the requests it claims. Held in a field because that seam hands back a borrow
    /// of it.
    invoice_document: invoice_document::InvoiceDocumentRoute<Eng>,
}

/// The HTTP status the model gives `ValidationException`
/// (`smithy_query(query="shape", shape="ValidationException")`: `@error("client")`,
/// `httpError 400`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The media type the TAG operations of this service answer in: the awsJson `1.0`
/// media type the model's protocol trait declares, rather than the `1.1` every
/// invoice-unit response announces (see the module docs).
///
/// Recorded behaviour: `ListTagsForResource`'s recorded 200 carries this media type
/// while every recorded invoice-unit response carries `…-1.1` — tagging is a separate
/// plane of this API, served by the tagging front end rather than by the backend that
/// answers for an invoice unit. `TagResource` and `UntagResource` belong to that same
/// plane and announce it too.
const TAG_PLANE_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// One PLANE of this API: the properties of an answer that belong to the plane that
/// answers rather than to the operation that was called — the media type it announces, and
/// the member its failures carry their explanatory text in (see [`error_body_members`]).
#[derive(Clone, Copy)]
struct Plane {
    /// The media type this plane's responses announce, when it is NOT the one
    /// `override_protocol` serves service-wide: `Some(TAG_PLANE_MEDIA_TYPE)` for the
    /// tag plane, `None` for every other plane.
    media_type: Option<&'static str>,
    /// The body member this plane's failures report their explanatory text in.
    detail_member: &'static str,
}

/// The invoice-unit, invoice-summary, invoice-document and invoice-profile operations: the
/// service-wide media type, and the text under the member name the model spells.
const INVOICE_PLANE: Plane = Plane {
    media_type: None,
    detail_member: error_body_members::MODEL_DETAIL_MEMBER,
};

/// The procurement-portal plane: the service-wide media type, and the text under the
/// capitalised member every recorded body of this plane carries it in.
const PORTAL_PLANE: Plane = Plane {
    media_type: None,
    detail_member: error_body_members::PORTAL_PLANE_DETAIL_MEMBER,
};

/// The tag plane: its own media type ([`TAG_PLANE_MEDIA_TYPE`]), and the text under the
/// member name the model spells.
const TAG_PLANE: Plane = Plane {
    media_type: Some(TAG_PLANE_MEDIA_TYPE),
    detail_member: error_body_members::MODEL_DETAIL_MEMBER,
};

/// One operation [`FrontDoor`] serves: the routing key the model names it by, how a
/// request for it is read and validated against ITS input shape, and the plane it belongs
/// to.
struct ServedOperation {
    /// The operation's routing key — read off the operation, never spelled out.
    name: &'static str,
    /// The violations in a request for this operation, found the way the generated
    /// walk finds them. `None` when there is no structured refusal to restate.
    violations: fn(
        &dyn ServerHttpProtocol,
        &aws_smithy_runtime_api::http::Request,
    ) -> Option<Vec<Violation>>,
    /// The plane this operation belongs to, which decides the two properties of its
    /// answers that are the plane's.
    plane: Plane,
}

/// Serves the operations of this service that are answered in front of the generated
/// walk, for three reasons the generated walk cannot reach on its own:
///
/// 1. A REFUSED INPUT is stated in this service's own modelled `ValidationException`
///    instead of the framework's generic refusal body. `ValidationException` models
///    `reason` (one of `ValidationExceptionReason`'s values) and `fieldList`
///    (`ValidationExceptionField{name, message}`) beside `message`, and the framework
///    validates every modelled constraint — `InvoiceReceiver`'s `@pattern`, `Name`'s
///    length and pattern, `MaxResults`' range, the ARN patterns — BEFORE a handler
///    runs, rendering a violation as the protocol's generic refusal body, which
///    carries `message` alone. A client deserializing that error gets a null `reason`
///    and an empty field list, so it can neither branch on the reason nor report which
///    member was rejected. The refusal is therefore stated HERE, where the violations
///    the boundary found are still in hand, through the one rendering
///    [`validation::field_validation_failed`] owns; a request with no violation is
///    handed to the generated walk unchanged.
/// 2. The TAG operations announce [`TAG_PLANE_MEDIA_TYPE`] rather than the media type
///    `override_protocol` serves service-wide (see the module docs): the media type is
///    a property of the plane that answers, and `override_protocol` is one choice for
///    the whole service, so the per-plane part is stamped here, on everything those
///    operations answer — a success and a modelled failure alike.
/// 3. A FAILURE reports its explanatory text in the member the ANSWERING PLANE carries it
///    in, and the resource a 404 is about in the member this service's bodies carry that
///    in ([`error_body_members`]). Both are wire members rather than the ones the model
///    spells, and `override_protocol` is one choice for the whole service, so the
///    per-plane part is applied here to what the operation's handler answered — which is
///    also why a refusal of the REQUEST, answered by reason 1 above before any handler
///    runs, keeps the member the model spells.
///
/// EVERY operation this service serves is served here, whichever plane it belongs
/// to: the modelled refusal body is a property of the REFUSAL — the model gives
/// `ValidationException` the same `reason` and `fieldList` members for every operation —
/// so leaving an operation out would answer one class of refusal two ways,
/// with the member detail for some operations and the message envelope alone for the
/// rest. This service implements every operation its model binds, so the table below is
/// that whole surface.
///
/// Generic over the engine type for the same reason the rest of this file is: the
/// operations whose schemas and routing keys it reads are, so naming a concrete engine
/// here would let this seam drift from the service the router mounts.
struct FrontDoor<Eng>(::std::marker::PhantomData<Eng>);

impl<Eng: Send + Sync + 'static> FrontDoor<Eng> {
    /// Every operation this seam serves: EVERY operation this crate implements, which is
    /// every operation this service's model binds. Each entry names the
    /// operation through its own `Operation` impl, so the routing key and the input schema
    /// come from the model.
    fn operations() -> [ServedOperation; 21] {
        use crate::ops;
        [
            // The invoice-unit plane.
            Self::served::<ops::create_invoice_unit::CreateInvoiceUnit<Eng>>(INVOICE_PLANE),
            Self::served::<ops::get_invoice_unit::GetInvoiceUnit<Eng>>(INVOICE_PLANE),
            Self::served::<ops::list_invoice_units::ListInvoiceUnits<Eng>>(INVOICE_PLANE),
            Self::served::<ops::update_invoice_unit::UpdateInvoiceUnit<Eng>>(INVOICE_PLANE),
            Self::served::<ops::delete_invoice_unit::DeleteInvoiceUnit<Eng>>(INVOICE_PLANE),
            Self::served::<ops::list_invoice_summaries::ListInvoiceSummaries<Eng>>(INVOICE_PLANE),
            Self::served::<ops::get_invoice_pdf::GetInvoicePDF<Eng>>(INVOICE_PLANE),
            // The invoice-profile plane.
            Self::served::<ops::batch_get_invoice_profile::BatchGetInvoiceProfile<Eng>>(
                INVOICE_PLANE,
            ),
            // The procurement-portal plane.
            Self::served::<
                ops::create_procurement_portal_preference::CreateProcurementPortalPreference<Eng>,
            >(PORTAL_PLANE),
            Self::served::<
                ops::get_procurement_portal_preference::GetProcurementPortalPreference<Eng>,
            >(PORTAL_PLANE),
            Self::served::<
                ops::put_procurement_portal_preference::PutProcurementPortalPreference<Eng>,
            >(PORTAL_PLANE),
            Self::served::<
                ops::delete_procurement_portal_preference::DeleteProcurementPortalPreference<Eng>,
            >(PORTAL_PLANE),
            Self::served::<
                ops::update_procurement_portal_preference_status::UpdateProcurementPortalPreferenceStatus<
                    Eng,
                >,
            >(PORTAL_PLANE),
            Self::served::<
                ops::send_procurement_portal_validation::SendProcurementPortalValidation<Eng>,
            >(PORTAL_PLANE),
            Self::served::<
                ops::verify_procurement_portal_validation::VerifyProcurementPortalValidation<Eng>,
            >(PORTAL_PLANE),
            Self::served::<
                ops::list_procurement_portal_preferences::ListProcurementPortalPreferences<Eng>,
            >(PORTAL_PLANE),
            Self::served::<ops::list_procurement_portals::ListProcurementPortals<Eng>>(
                PORTAL_PLANE,
            ),
            Self::served::<
                ops::list_procurement_portal_suppliers::ListProcurementPortalSuppliers<Eng>,
            >(PORTAL_PLANE),
            // The tag plane, which announces its own media type.
            Self::served::<ops::list_tags_for_resource::ListTagsForResource<Eng>>(TAG_PLANE),
            Self::served::<ops::tag_resource::TagResource<Eng>>(TAG_PLANE),
            Self::served::<ops::untag_resource::UntagResource<Eng>>(TAG_PLANE),
        ]
    }

    /// One entry of the table above, read off the operation itself: `Op` carries the
    /// routing key the model names it by, the input schema a request for it is validated
    /// against, and — as [`Operation::Input`] — the shape that request is read into. Taking
    /// all three from the one type is what keeps an entry from naming one operation's key
    /// beside another's input, and leaves an entry nothing to state but its plane.
    fn served<Op: Operation>(plane: Plane) -> ServedOperation {
        ServedOperation {
            name: <Op as Operation>::NAME,
            violations: |protocol, req| {
                Self::violations::<<Op as Operation>::Input>(
                    protocol,
                    req,
                    <Op as Operation>::INPUT_SCHEMA,
                )
            },
            plane,
        }
    }

    /// The violations in `req` read against `schema`, the way the framework's own walk
    /// reads them: deserialize the input, then validate the built value.
    ///
    /// `None` means the request carries no structured refusal — either it is valid, or
    /// it failed in a way that has no member-level form (a body the codec cannot read
    /// at all), which the generated walk renders itself.
    fn violations<I>(
        protocol: &dyn ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
        schema: &'static aws_smithy_schema::Schema,
    ) -> Option<Vec<Violation>>
    where
        I: DeserializableStruct + aws_smithy_schema::serde::SerializableStruct,
    {
        let error = match protocol.deserialize_input(req, schema) {
            Ok(mut deser) => {
                match <I as DeserializableStruct>::deserialize(&mut *deser) {
                    // A built input: every modelled constraint, in one walk.
                    Ok(input) => {
                        let violations =
                            aws_smithy_schema_server::constraints::validate(&input, schema);
                        return (!violations.is_empty()).then_some(violations);
                    }
                    // Absence is found while READING the request, so it arrives as a
                    // codec error; `refusal_of` gives it the same structured form a
                    // constraint violation has.
                    Err(e) => aws_smithy_schema_server::ProtocolError::Serde(e),
                }
            }
            Err(e) => e,
        };
        match smithy_server_runtime::refusal_of(&error) {
            Some(Refusal::Invalid(violations)) if !violations.is_empty() => Some(violations),
            _ => None,
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for FrontDoor<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The MODEL-DERIVED protocol, not this service's own override: a refusal of
            // the request is answered by the input validator in front of the service,
            // which reads and announces the protocol the model declares (see the module
            // docs on the media types). The codec is the same JSON either way, so the
            // request reads identically.
            let protocol = svc.definition().protocol();
            let Ok(routing_key) = protocol.routing_key(req) else {
                return svc.definition().routing_error_response(req);
            };
            let served = Self::operations()
                .into_iter()
                .find(|entry| entry.name == routing_key);
            let Some(entry) = served else {
                return svc.definition().routing_error_response(req);
            };
            if let Some(violations) = (entry.violations)(protocol, req) {
                let modelled = validation::field_validation_failed(&violations);
                return protocol
                    .serialize_error(
                        &modelled,
                        &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                        crate::ops::types::ValidationException::shape_name(),
                        VALIDATION_EXCEPTION_STATUS,
                    )
                    // The nested detail entries name their own shape here too — the
                    // member the text travels in stays the model's, because this body is
                    // the request validator's rather than the plane's (see
                    // [`error_body_members`]).
                    .map(error_body_members::with_named_detail_entries)
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
            }
            // Nothing to restate: run the generated deserialize -> validate -> handle
            // -> serialize walk. Reached through the routing table rather than through
            // `override_operation`, which would route straight back here. The walk
            // reads and validates the body again — the price of answering a refusal in
            // the modelled shape, paid once per request on a body the transport has
            // already bounded.
            let answered = match svc.definition().routing_table().get(entry.name) {
                Some(op) => op.run(svc, req, ctx).await,
                None => return svc.definition().routing_error_response(req),
            };
            // What this operation ANSWERED is the plane's answer, so the members its
            // failures report are this plane's: the explanatory text under the member
            // this plane's bodies carry it in, and the resource a 404 is about under the
            // member this service's bodies carry that in (see [`error_body_members`]).
            // A success, and a failure whose class this service does not model, are
            // unchanged.
            let mut response =
                error_body_members::as_this_service_reports(answered, entry.plane.detail_member);
            // This operation's own plane announces its own media type: replace the one
            // the service-wide protocol stamped on the body it just serialized. The
            // bytes are unchanged — only the media type naming them differs.
            if let Some(media_type) = entry.plane.media_type {
                response.headers_mut().insert("content-type", media_type);
            }
            response
        })
    }
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            protocol: aws_smithy_schema_server::rpc::AwsJson11(),
            front_door: FrontDoor(::std::marker::PhantomData),
            invoice_document: invoice_document::InvoiceDocumentRoute::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The download of an invoice document, which is not an API call at all: it is the
        // artifact `GetInvoicePDF` hands out a pre-signed URL for, served by the route
        // that owns it (see [`invoice_document`]). Claimed first, because such a request
        // names no operation for the routing key below to resolve.
        if self.invoice_document.claims(req).is_some() {
            return Some(&self.invoice_document as &dyn ErasedOperation);
        }
        // Every operation needs it: a refused input is stated in this
        // service's own modelled `ValidationException` — with the modelled `reason` and
        // `fieldList` — instead of the framework's generic refusal body, and the tag
        // plane announces its own media type. See [`FrontDoor`] for why neither can be
        // done from inside `handle`. The operation names are read from the model, not
        // spelled out.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        FrontDoor::<Eng>::operations()
            .iter()
            .any(|entry| entry.name == routing_key)
            .then_some(&self.front_door as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // A refusal of an operation this service serves never reaches this seam —
        // [`FrontDoor`] answers it BEFORE the framework renders it, because the generic
        // refusal body cannot carry the modelled `reason`/`fieldList` members at all and
        // `Refused` is only a code and a message. What does reach it is a refusal the
        // front door has no structured violation to restate — a request whose body the
        // codec cannot read as the operation's input at all. Its `message` is worded by
        // the same formatter the modelled body uses ([`validation`]), so one constraint
        // violation is stated one way wherever this service refuses it: the member path
        // and the requirement, never the rejected value, and a `@pattern` without its
        // anchors.
        validation::refused(refusal)
    }

    fn override_protocol(&self) -> Option<&dyn ServerHttpProtocol> {
        // Every response this SERVICE produces — a success and a modelled failure its
        // handler raises — is serialized by the protocol returned here, so this is
        // where the media type it announces is decided for the service as a whole. See
        // the module docs: every recorded invoice-unit response carries the awsJson
        // `1.1` media type, which is what a client keys its parsing off. The tag
        // plane's own media type is stamped by [`FrontDoor`], which serves those three
        // operations.
        Some(&self.protocol)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Left to the entry's protocol-correct default
        // (awsJson -> 404 in the media type the model's protocol trait declares):
        // naming no operation is refused in FRONT of the service, so it is not a
        // response the service produced.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. The error arrives BORROWED and is not `Clone`, so it cannot be
        // re-rendered here; the default renderer consumes the owned one.
        let _ = error;
        None
    }
}
