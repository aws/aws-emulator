// SPDX-License-Identifier: CC0-1.0

//! EMPTY — this file carries no test and is not declared in `tests/all.rs`, so nothing
//! here compiles or runs.
//!
//! It held a throwaway probe used to locate where the pre-existing stack overflow in
//! `all/s3_notification_invokes_its_lambda_target.rs` happens (the deploy, the object
//! write and the handler's own calls all pass; the abort comes later, in the phase that
//! reads the function's log group). Its content is gone rather than the file because this
//! fixer's tool set can write a file but not remove one; the next change to touch this
//! directory should delete it.
