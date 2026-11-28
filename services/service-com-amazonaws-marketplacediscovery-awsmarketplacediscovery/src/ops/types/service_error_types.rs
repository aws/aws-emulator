// SPDX-License-Identifier: CC0-1.0

//! The error shapes this service declares on the SERVICE rather than on an operation.
//!
//! The model binds `AccessDeniedException`, `InternalServerException`, `ThrottlingException`
//! and `ValidationException` to the service shape itself
//! (`smithy_query(query="service_info")` lists them as this service's common errors), so
//! they constrain every operation while appearing in no operation's own error list. The
//! scaffold renders a type for each shape an OPERATION reaches, which leaves these four
//! undeclared — and `AccessDeniedException` is the refusal every handler of this crate
//! answers an unauthorized caller with (`crate::service::authorization`), widened into each
//! operation's error enum through `extra_errors`.
//!
//! So the shape is declared here, from the upstream model like every other modelled shape
//! of this crate: the member names, the shape id, the HTTP status and the error
//! classification all come from that file rather than from a schema written out by hand.
//! This module is NOT rendered by the scaffold — it carries no `@generated` marker — which
//! is what keeps the declaration from being lost the next time the model is refreshed.
//!
//! The other three are not declared: nothing in this crate constructs them. A constraint
//! refusal is worded by `crate::service::validation` over the framework's own 400 channel
//! (`crate::scaffolded`), and this emulator neither throttles nor faults.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json");

/// "You don't have sufficient access to perform this action. Verify that you have the
/// required permissions." — HTTP 403, a client error, with one optional `message` member
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
#[smithy_macros::smithy(shape = "com.amazonaws.marketplacediscovery#AccessDeniedException")]
#[derive(Debug, Clone, Default)]
pub struct AccessDeniedException {
    pub message: Option<String>,
}
