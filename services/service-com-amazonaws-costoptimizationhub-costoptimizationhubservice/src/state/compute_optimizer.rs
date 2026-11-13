// SPDX-License-Identifier: CC0-1.0

//! Whether an account is opted into AWS Compute Optimizer.
//!
//! Part of Cost Optimization Hub's data is not its own: the efficiency and
//! savings figures a time-series read reports are derived from AWS Compute
//! Optimizer's analysis of the account's resources, which that service produces
//! only for an account that has opted INTO it. So the opt-in is a precondition
//! of such a read, and the record of it belongs to Compute Optimizer, whose
//! `UpdateEnrollmentStatus` is the only call that writes it.
//!
//! This module is therefore the seam that asks THAT service's store, over the
//! one shared engine, rather than holding a second copy of the flag here: a
//! second copy could say "opted in" for an account Compute Optimizer reports as
//! `Inactive`, and the two services would then disagree about one fact. It is a
//! pure read: nothing here writes to either store.

use ::service_com_amazonaws_computeoptimizer_computeoptimizerservice::state as compute_optimizer;

/// Whether `account_id` is opted into AWS Compute Optimizer, as that service's
/// own store holds it.
///
/// Compute Optimizer records the enrollment of the account it answers for and,
/// where an organization is recorded, one record per member account of it, so
/// the question is asked of whichever record covers `account_id`. An account
/// with no record there has never opted in.
///
/// `false` as well when the shared engine bears no Compute Optimizer store at
/// all: with no record of an opt-in anywhere, no account is opted in, and a read
/// that depends on the analysis the opt-in starts has nothing to report.
pub fn opted_into_compute_optimizer<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> bool {
    let Some(state) = compute_optimizer::service_state(engine) else {
        return false;
    };
    let store = state.lock();
    if account_id == ::engine::emulator_account_id() {
        return store.enrollment.is_opted_in();
    }
    store
        .organization_members
        .get(account_id)
        .is_some_and(|member| member.is_opted_in_at(compute_optimizer::now_secs()))
}
