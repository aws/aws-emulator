// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Every `tests/all/<name>.rs` is compiled as a suite of this one target — put a
//! new suite there and declare nothing: `automod` writes the `mod` line for it.
//!
//! A suite left at the top level of `tests/` fails the build.

automod::dir!("tests/all");
