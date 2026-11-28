// SPDX-License-Identifier: CC0-1.0

//! Whether the pollers the mappings of this store ASK FOR are running.
//!
//! "When true, the event source mapping is active. When false, Lambda pauses polling and
//! invocation" (`CreateEventSourceMapping$Enabled`), and a mapping's `State` is which of the
//! two it is in — so a mapping a client is told is `Enabled` is one that READS from its source
//! and hands what it reads to its function. That reading is done by a background task per
//! mapping, claimed once so two tasks never read one mapping (the store carries that claim):
//! the task stands down as soon as its mapping stops wanting to be polled, and its claim goes
//! back with it — so a mapping that HAS no task is a mapping nothing polls, whatever its
//! `State` says.
//!
//! Which is why arming a poller cannot belong to the call that made the mapping alone. A task
//! that ends for any other reason — a pass that fails on its way out, a create served where
//! there was no runtime to spawn onto — leaves the mapping reporting `Enabled` with nothing
//! reading its source and no call that would ever bring it back: the inert trigger a client
//! cannot tell from a working one. So the operations that create or change a mapping arm the
//! mappings of the store ([`arm_mapping_pollers`]) and the ones that REPORT mappings arm
//! exactly the mappings they reported ([`arm_reported_mapping_pollers`]) — a call reaches the
//! data plane of the resources it is about and of no others — and the state a mapping reports
//! is never a state its own data plane is not in.
//!
//! What either does is owed by the mappings themselves at the instant it runs rather than by
//! the call that happened to arrive, and nothing a caller is answered turns on it: a mapping
//! that is paused or being deleted is armed by neither, a mapping already being polled keeps
//! the one task it has, and a mapping whose function cannot be invoked yet is polled as soon as
//! it can be. Each reader decides for itself whether a mapping's own source is one it reads, so
//! a mapping is armed by exactly one of them.

use ::std::sync::Arc;

/// Arm the poller of every mapping of `engine`'s store that wants one and has none running.
///
/// For the operations that CREATE or CHANGE a mapping, which is what the mappings of a store
/// were armed by before any of them could be reported. Called AFTER the store's lock is
/// released — each reader takes it itself — and nothing waits for a poll: every poller runs on
/// its own clock.
pub fn arm_mapping_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
) {
    arm(ctx, engine, None);
}

/// Arm the pollers of exactly the mappings `reported` names, which are the ones the calling
/// operation has just REPORTED the state of.
///
/// Scoped to those: a call about one mapping (or about one PAGE of them) is no call about the
/// data plane of the mappings it did not report, so it arms none of them. Called AFTER the
/// store's lock is released, for the reason above.
pub fn arm_reported_mapping_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
    reported: &[String],
) {
    for uuid in reported {
        arm(ctx, engine, Some(uuid));
    }
}

/// The whole of both: the mapping `only` names when it names one, and every mapping of the
/// store when it names none.
///
/// Each reader is handed that same selection and starts a task for the mappings it reads which
/// want one and have none, so the selection is applied in ONE place rather than once per
/// reader.
fn arm<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
    only: Option<&str>,
) {
    super::sqs_poller::drive_sqs_pollers_of(ctx, engine, only);
    super::mq_poller::drive_mq_pollers_of(ctx, engine, only);
    super::kafka_poller::drive_kafka_pollers_of(ctx, engine, only);
    match only {
        // The reader of a SHARD source is armed per mapping, so a selection of one mapping is
        // that one call; a mapping whose source it does not read starts nothing.
        Some(uuid) => super::event_source_poller::drive_mapping(ctx, engine, uuid),
        None => {
            let Some(state) = super::service_state(&**engine) else {
                return;
            };
            let mappings: Vec<String> = {
                let store = state.lock();
                store.event_source_mappings.keys().cloned().collect()
            };
            for uuid in mappings {
                super::event_source_poller::drive_mapping(ctx, engine, &uuid);
            }
        }
    }
}
