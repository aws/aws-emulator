// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs`; `automod` declares it. Nothing is
//! declared by hand here.
//!
//! A suite left at the top level of `tests/` fails the build.

automod::dir!("tests/all");
