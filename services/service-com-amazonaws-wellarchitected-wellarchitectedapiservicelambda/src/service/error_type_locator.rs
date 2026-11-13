// SPDX-License-Identifier: CC0-1.0

//! How this service NAMES, in the `x-amzn-errortype` response header, the error shape a
//! refused call is refused with.
//!
//! A restJson1 client resolves the error shape from that header, and the framework's
//! renderers write the shape's bare name into it. What this service's wire carries is the
//! QUALIFIED form: the shape name, the separator, and then the locator of the namespace that
//! declares the shape, under the service stack's locator root:
//!
//! ```text
//! HTTP/1.1 404 Not Found
//! x-amzn-errortype: NotFoundException:http://internal.amazon.com/coral/com.amazonaws.wellarchitected/
//!
//! {"__type":"NotFoundException","Message":"Resource was not found"}
//! ```
//!
//! The locator is not part of the shape's NAME — every SDK resolves the shape from the text
//! before the separator (and after a `#`, for a namespaced name), so nothing a client
//! resolves changes — but it is what this service's wire carries, and a client that reads or
//! validates the whole header value sees it. The body keeps naming the bare shape in
//! `__type`, exactly as the renderer wrote it.
//!
//! The form itself, and the wrapper that completes a framework-level response with it, are
//! [`service_common::error_type_locator`]'s: the locator is DERIVED from this service's own
//! generated identity (`com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda`
//! carries the namespace that declares this service's shapes), so nothing about the value is
//! written out here and a model moved to another namespace follows.
//!
//! The header belongs to the RESPONSE, not to any modelled shape — no error shape of this
//! model declares it, so no handler can set it — so it is completed at the two layers every
//! response of this service passes through:
//!
//! * [`super::ModelledValidation`], the per-operation wrapper
//!   [`crate::service::EmulatedService`] hands the router from `Service::override_operation`,
//!   covers everything an operation answers: a success (untouched), a modelled error, the
//!   error of a surface this model does not declare, and a refusal raised while that
//!   operation was being served;
//! * [`ErrorTypeLocatorDefinition`], the definition wrapper `Service::definition` hands back,
//!   covers a response NO operation produced: a routing failure, a transport failure detected
//!   up front, and a pre-dispatch refusal.
//!
//! Nothing else about a response changes: the status, the body and every other header stay as
//! the renderer wrote them, and a success keeps every byte.

pub use service_common::error_type_locator::{shape_locator as locator, with_error_type_locator};

/// This service's generated definition under the shared wrapper that qualifies the error
/// shape of every framework-level response it renders.
pub type ErrorTypeLocatorDefinition<D> =
    service_common::error_type_locator::QualifiedErrorTypeDefinition<D>;
