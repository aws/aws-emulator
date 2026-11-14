// SPDX-License-Identifier: CC0-1.0

//! EMPTY, and declared only because the build script requires every file under `tests/all/`
//! to be declared.
//!
//! This file held a first draft of the suite that pins what an `UpdateStack` does to the
//! `AWS::S3::Bucket` a stack already created. The suite itself lives — and runs — in the
//! CloudFormation crate's own test target, which mounts Amazon S3 beside CloudFormation on one
//! engine exactly as `core/router/src/lib.rs` does, and so can drive both ends of the rule:
//! `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/
//! bucket_policy_update_and_rename_rollback.rs`.
//!
//! The file stays because this fixer's tool set can write a file but not remove one (the same
//! reason `zz_probe.rs` beside it stays); the next change to touch this directory should delete
//! both it and its declaration.
