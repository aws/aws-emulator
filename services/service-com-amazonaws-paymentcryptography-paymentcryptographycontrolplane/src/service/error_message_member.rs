// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty, and declared nowhere.
//!
//! This change first stated the body-member rule for an error's prose here; mainline then
//! landed the same rule for the whole service in [`super::message_member`], reached from
//! the operation wrapper in [`super`], so this file's content was dropped in the merge
//! rather than kept as a second copy of it. Nothing compiles from here: `src/service.rs`
//! declares `message_member` and not this file.
