// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs`; nothing declares it. `automod::dir!` below
//! declares one `mod <name>;` per `tests/all/<name>.rs` when this target compiles, so a
//! new suite is a new file and no list can forget it.
automod::dir!("tests/all");
