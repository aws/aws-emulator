// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and nothing else: `automod::dir!` declares one
//! module per file in that directory, so a suite is picked up by existing.
//!
//! A suite left at the top level of `tests/` fails the build.

automod::dir!("tests/all");
