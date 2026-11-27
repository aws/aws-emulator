// SPDX-License-Identifier: CC0-1.0

//! The ACCESS DENIAL this service answers an unauthorized call with.
//!
//! `AccessDeniedException` (403) is one of the errors the model binds to the SERVICE
//! rather than to an operation (`smithy_query(query="digest")` reports it among
//! `common_errors`), so it reaches every operation but appears in no operation's own
//! error list — and the scaffold renders a declaration for the shapes an operation
//! names. This module is therefore HAND-WRITTEN, and deliberately not at one of the
//! paths the scaffold writes (`shared_types.rs`, `<operation>_types.rs`): a
//! declaration placed there is overwritten the next time the model is refreshed,
//! which is what left `crate::ops::create_stream` without this shape.
//!
//! The declaration itself is generated from the published model exactly as every
//! other shape of this service is — one member list, one set of traits, no
//! hand-written schema or serializer — so it carries the status, the wire code and
//! the member the model declares.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dsql-dsql.json");

/// The denial an operation of this service answers a call it may not perform with:
/// a client error at HTTP 403 carrying the `@required` `message`
/// (`smithy_query(query="shape", shape="AccessDeniedException")`).
#[smithy_macros::smithy(shape = "com.amazonaws.dsql#AccessDeniedException")]
#[derive(Debug, Clone, Default)]
pub struct AccessDeniedException {
    pub message: String,
}
