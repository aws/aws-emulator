// SPDX-License-Identifier: CC0-1.0

//! Which member of an error body this service reports a failure's text in: `Message`.
//!
//! # The rule
//!
//! Every error shape this model declares that carries prose carries it in one member spelled
//! `Message`, targeting `String`, and none of them carries a `@jsonName`
//! (`smithy_query(query="shape",shape="AccessDeniedException,ConflictException,InternalServerException,ServiceQuotaExceededException,ServiceUnavailableException,ThrottlingException,ValidationException",model_path="services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/smithy-model/com-amazonaws-paymentcryptography-paymentcryptographycontrolplane.json")`),
//! and an `awsJson1_0` member with no `@jsonName` goes on the wire under its Smithy name. So
//! `Message` is where one of this service's errors carries its text, whatever produced it:
//!
//! ```json
//! HTTP/1.1 400 Bad Request
//! content-type: application/x-amz-json-1.0
//!
//! {"__type":"ValidationException","Message":"1 validation error detected: …"}
//! ```
//!
//! An error a HANDLER returns is serialized from the model's own shape, so it already carries
//! that member. The body the FRAMEWORK renders for a request it refused before any handler ran
//! does not: that body is built from a synthetic shape whose own member is spelled `message`,
//! and the JSON error renderer writes that spelling even when the code names one of this
//! service's MODELLED error shapes. A client — or an SDK deserializing the modelled exception
//! the code names — then finds no `Message` member at all and surfaces the refusal with no text
//! at all, though its status and its code are right.
//!
//! This layer therefore reports that text in `Message` on every error body this service answers,
//! wherever it was produced. The renaming itself is `service_common`'s, so this file states only
//! the two member names and the shapes the rule covers; the status, the `__type` discriminator,
//! every other member and the whole success path stay with the renderer that owns them. A
//! failure whose class this model does not declare — the scaffold's own 501, a pre-dispatch
//! authentication refusal — keeps the member the framework wrote it in, because the rename is
//! keyed on the modelled shapes.
//!
//! The structural home for the rule is the framework's error renderer, which writes the
//! synthetic shape's `message` for every service; that renderer is in the agent-immutable
//! `vendor/` tree, so this service states it at the one seam that can see a whole rendered
//! response — its operation wrapper in [`super`]. A failure answered before any operation is
//! served (a request naming no modelled operation, a transport-level failure) is therefore not
//! reached from here.

use aws_smithy_runtime_api::http::Response as RtResponse;

/// The body member this service's errors carry their text in — the member name every error
/// shape in the model declares for its prose; see the module header for the `smithy_query`
/// call it comes from.
const MODELLED_MESSAGE_MEMBER: &str = "Message";

/// The body member the FRAMEWORK's own refusal renderer writes that text in: the member of the
/// synthetic shape it builds a pre-handler refusal from.
const RENDERED_MESSAGE_MEMBER: &str = "message";

/// Every error shape this model declares that carries PROSE, by the wire code (`__type`) it
/// renders under — the same `smithy_query` call the module header cites, which is where this
/// list comes from and the only thing that may extend it.
///
/// `ResourceNotFoundException` is deliberately absent: it is the one error shape in this model
/// that declares no prose member at all (its single member is `ResourceId`), so there is no
/// member for its text to be reported in.
///
/// The rename is keyed on THIS set rather than on the status, so a failure whose class this
/// service does not model keeps the member the framework wrote its text in.
const MODELLED_ERROR_SHAPES: [&str; 7] = [
    "AccessDeniedException",
    "ConflictException",
    "InternalServerException",
    "ServiceQuotaExceededException",
    "ServiceUnavailableException",
    "ThrottlingException",
    "ValidationException",
];

/// `resp` as this service answers it: a modelled error body whose text is in its `Message`
/// member, through the one renaming `service_common` states for every service whose wire member
/// differs from the one a layer wrote.
///
/// A response that is not a failure, or whose body is not a modelled error body carrying the
/// framework's own member, is returned untouched.
#[must_use]
pub(super) fn with_modelled_message_member(resp: RtResponse) -> RtResponse {
    service_common::error_detail_member::with_renamed_detail_member(
        resp,
        &MODELLED_ERROR_SHAPES,
        RENDERED_MESSAGE_MEMBER,
        MODELLED_MESSAGE_MEMBER,
    )
}
