// SPDX-License-Identifier: CC0-1.0

//! The two refusal classes this service publishes no `errorCode` for, as AWS
//! carries them on the wire: the `message` the snapshot declares `@required`,
//! and `errorCode` as an OPTIONAL member, so a refusal that has no
//! service-defined code to report writes none.
//!
//! The snapshot in this crate's `smithy-model/` is kept EXACTLY as shipped, so
//! these two shapes are declared HERE rather than beside the other shapes
//! several operations reach — generated, like every modelled shape in this
//! crate, from the small overlay model `smithy-model/local-additions.json`,
//! which restates each of them against the snapshot's own targets, so every
//! length and pattern constraint and the `@sensitive` marking of the message are
//! captured exactly as the snapshot spells them (see
//! `smithy-model/LOCAL-ADDITIONS.md`). Nothing about the wire is hand-written:
//! the member names, the shape ids, the error classes and the HTTP statuses all
//! come from a model file.
//!
//! What each member CARRIES is not decided here: [`crate::state::no_tax_registration`]
//! and [`crate::state::state_unavailable`] word the refusals, and the classes
//! this service DOES publish a code for (`ValidationException`,
//! `ConflictException`) are declared from the snapshot in [`super::shared_types`]
//! and carry it.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The not-found refusal (HTTP 404): the service's sentence, and no code.
#[smithy_macros::smithy(shape = "com.amazonaws.taxsettings#ResourceNotFoundException")]
#[derive(Debug, Clone, Default)]
pub struct ResourceNotFoundException {
    pub message: String,
    pub error_code: Option<String>,
}

/// The unexpected-failure refusal (HTTP 500): the service's sentence, and no
/// code.
#[smithy_macros::smithy(shape = "com.amazonaws.taxsettings#InternalServerException")]
#[derive(Debug, Clone, Default)]
pub struct InternalServerException {
    pub message: String,
    pub error_code: Option<String>,
}
