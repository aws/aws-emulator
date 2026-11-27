// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate: `automod::dir!` declares one module
//! per `tests/all/<name>.rs`, so a new suite is a new file and nothing else.

automod::dir!("tests/all");
