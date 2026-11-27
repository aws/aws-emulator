// SPDX-License-Identifier: CC0-1.0

//! The not-found error every lookup of this service refuses with, as AWS carries it on the
//! wire: the three members the published model snapshot declares, plus the one member such a
//! refusal reports that the snapshot does not declare — the ARN the request addressed the
//! resource by.
//!
//! The snapshot in this crate's `smithy-model/` is kept EXACTLY as shipped, so this shape is
//! declared HERE rather than beside the other shapes several operations reach — generated,
//! like every modelled shape in this crate, from the small overlay model
//! `smithy-model/local-additions.json`, which restates the shape with that member beside the
//! three the snapshot declares, each against the snapshot's own target, so every `@required`
//! marking and every length and pattern constraint is captured exactly as the snapshot spells
//! it (see `smithy-model/LOCAL-ADDITIONS.md`). Nothing about the wire is hand-written: the
//! member names, the shape id, the error class and the HTTP status all come from a model file.
//!
//! What each member CARRIES is not decided here: the identifier-keyed refusal
//! ([`crate::state::not_found`]) and the ARN-keyed refusal ([`crate::state::arn_not_found`])
//! each fill in what the request it refuses supplied.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The not-found refusal: the service's sentence, what did not resolve, and — on a request
/// that addressed its subject by ARN — that ARN, exactly as the request supplied it.
///
/// The three modelled members are `@required`, so each is a bare `String` and each is always
/// written; the fourth is written only by a refusal that HAS an ARN to report.
#[smithy_macros::smithy(shape = "com.amazonaws.pcs#ResourceNotFoundException")]
#[derive(Debug, Clone, Default)]
pub struct ResourceNotFoundException {
    pub message: String,
    pub resource_id: String,
    pub resource_type: String,
    pub resource_arn: Option<String>,
}
