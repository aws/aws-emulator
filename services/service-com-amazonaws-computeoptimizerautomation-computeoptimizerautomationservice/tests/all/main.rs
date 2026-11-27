// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate: `automod` declares one module per
//! `tests/all/<name>.rs`, so a new suite is a new file and nothing is declared by hand.

automod::dir!("tests/all");
