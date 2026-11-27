// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate: every `tests/all/<name>.rs` is one of its
//! suites, declared by `automod` from its own file, so a new suite is a new file and nothing
//! lists it.
automod::dir!("tests/all");
