// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Suites are picked up from this directory by `automod`, one module per `.rs` file, so a
//! new suite is a new file and needs no declaration. Add a suite at `tests/all/<name>.rs`
//! with a snake_case name. Group suites with `tests/all/<group>.rs` containing
//! `automod::dir!("tests/all/<group>");`, and put the group's files in `tests/all/<group>/`.
//!
//! Never add a `main.rs` or `mod.rs` elsewhere under `tests/`, and never put a suite at the
//! top level of `tests/`: each fails the build.

automod::dir!("tests/all");
