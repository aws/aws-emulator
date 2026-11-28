// SPDX-License-Identifier: CC0-1.0

//! The two conditions a Storage Lens group's filter narrows objects by: their
//! object-size and object-age ranges, each BOUND reported only when the document
//! that wrote the group carried it.
//!
//! A bound a document did not carry stays absent through deserialize → store →
//! serialize, so the document a read reports carries exactly the members the
//! document it was written with carried. Both members of each condition carry a
//! modelled `@default` of 0, and 0 is a value their own documentation excludes
//! ("the value must be a positive number, greater than 0", and an upper bound
//! must be "greater than the minimum object size"), so materialising the default
//! for a bound nobody set would state a range the group does not filter on — and
//! a client that reads a group, edits one clause and writes it back would then
//! submit it.
//!
//! A member carrying a `@default` is a member the codec always writes: the
//! declaration admits no `Option` for it, so the shapes are declared here, from
//! the small overlay model `smithy-model/local-additions.json`, which restates
//! the two structures and the two value shapes their members target without that
//! trait (see `smithy-model/LOCAL-ADDITIONS.md`). Nothing about the wire is
//! hand-written: the member names, the member order, the shape ids and the
//! targets all come from that file, and the published snapshot in
//! `smithy-model/` is left exactly as shipped. `types/mod.rs` re-exports this
//! module like every other, so both shapes resolve unchanged wherever they are
//! named.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The object-age range a Storage Lens group filters on.
#[smithy_macros::smithy(shape = "com.amazonaws.s3control#MatchObjectAge")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchObjectAge {
    pub days_greater_than: Option<i32>,
    pub days_less_than: Option<i32>,
}

/// The object-size range a Storage Lens group filters on.
#[smithy_macros::smithy(shape = "com.amazonaws.s3control#MatchObjectSize")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchObjectSize {
    pub bytes_greater_than: Option<i64>,
    pub bytes_less_than: Option<i64>,
}
