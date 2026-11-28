// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! `automod` declares one module per `tests/all/<name>.rs`, so a new suite is a new
//! file there and needs no declaration. To group suites, add `tests/all/<group>.rs`
//! holding exactly `automod::dir!("tests/all/<group>");` and put them in
//! `tests/all/<group>/`.

automod::dir!("tests/all");
