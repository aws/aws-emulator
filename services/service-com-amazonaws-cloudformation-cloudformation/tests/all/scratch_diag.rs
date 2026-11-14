// SPDX-License-Identifier: CC0-1.0

//! Deliberately empty: this file held the probe that located why a stack declaring a
//! DynamoDB stream, the function it triggers and the mapping between them rolled back while
//! that was being traced, and the suite that now pins the rule lives under the name it
//! deserves, `cloudwatch_alarm_and_ecs_service_resources.rs`.
//!
//! The file stays declared in `tests/all.rs` for the reason `scratch.rs` and
//! `scratch_probe.rs` beside it record: this crate's build script requires every file under
//! `tests/all/` to be declared with a module of the same name, and the fixer tooling that
//! wrote it can create and rewrite a file but cannot remove one.
