// SPDX-License-Identifier: CC0-1.0

//! @op ModifyClusterSnapshot
//!
//! Modifies the settings of ONE snapshot: the number of days a manual snapshot is
//! retained.
//!
//! The retention period is validated against the published range, applied to the
//! stored record through [`crate::state::apply_manual_retention`] — the one rule
//! for it, shared with `CreateClusterSnapshot` — and reported back with the
//! members it derives (`SnapshotRetentionStartTime`,
//! `ManualSnapshotRemainingDays`), so a later `DescribeClusterSnapshots` reports
//! exactly what this call persisted.
//!
//! A period whose window has already ended for the named snapshot (retention is
//! counted from the snapshot's own creation time) is refused with the modelled
//! `InvalidRetentionPeriodFault`, unless the request carries `Force`, which
//! deletes the snapshot immediately instead.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ModifyClusterSnapshot`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json")
//! ```
//!
//! Modifies the settings for a snapshot. This exanmple modifies the manual retention period
//! setting for a cluster snapshot.
//!
//! ## Input
//!
//! * `Force` (Boolean) — A Boolean option to override an exception if the retention period
//!   has already passed.
//! * `ManualSnapshotRetentionPeriod` (IntegerOptional) — The number of days that a manual
//!   snapshot is retained. If the value is -1, the manual snapshot is retained indefinitely.
//!   If the manual snapshot falls outside of the new retention period, you can specify the
//!   force option to immediately delete the snapshot. The value must be either -1 or an
//!   integer between 1 and 3,653.
//! * `SnapshotIdentifier` (String, len 0..2147483647) **required** — The identifier of the
//!   snapshot whose setting you want to modify.
//!
//! ## Output
//!
//! * `Snapshot` (Snapshot) — Describes a snapshot.
//!
//! ## Errors
//!
//! * `ClusterSnapshotNotFoundFault` (HTTP 404) — The snapshot identifier does not refer to an
//!   existing cluster snapshot.
//! * `InvalidClusterSnapshotStateFault` (HTTP 400) — The specified cluster snapshot is not in
//!   the available state, or other accounts are authorized to access the snapshot.
//! * `InvalidRetentionPeriodFault` (HTTP 400) — The retention period specified is either in
//!   the past or is not a valid value. The value must be either -1 or an integer between 1
//!   and 3,653.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ModifyClusterSnapshotError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ModifyClusterSnapshotError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `DependentServiceUnavailableFault` is widened in: the model does not list it for
// this operation, but the service declares the shape, and it is the only modelled
// way to say "the control-plane store could not be reached".
#[smithy_macros::operation_error(
    "com.amazonaws.redshift#ModifyClusterSnapshot",
    extra_errors = ["DependentServiceUnavailableFault"]
)]
#[derive(Debug)]
pub enum ModifyClusterSnapshotErrorModeled {
    ClusterSnapshotNotFoundFault(ClusterSnapshotNotFoundFault),
    InvalidClusterSnapshotStateFault(InvalidClusterSnapshotStateFault),
    InvalidRetentionPeriodFault(InvalidRetentionPeriodFault),
    DependentServiceUnavailableFault(DependentServiceUnavailableFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::modify_cluster_snapshot::ModifyClusterSnapshotError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ModifyClusterSnapshotError = crate::scaffolded::Scaffolded<ModifyClusterSnapshotErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ModifyClusterSnapshotErrorModeled> for ModifyClusterSnapshotError {
    fn from(modeled: ModifyClusterSnapshotErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ModifyClusterSnapshot` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ModifyClusterSnapshot<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ModifyClusterSnapshot<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ModifyClusterSnapshot<Eng>
{
    type Input = ModifyClusterSnapshotInput;
    type Output = ModifyClusterSnapshotOutput;
    type Error = ModifyClusterSnapshotError;

    const NAME: &'static str = "ModifyClusterSnapshot";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::MODIFY_CLUSTER_SNAPSHOT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::MODIFY_CLUSTER_SNAPSHOT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        // This operation reads and writes the store alone; nothing it reports is
        // Region-derived, so `ctx` carries nothing it needs.
        let _ = ctx;

        let Some(state) = st::service_state(&*self.engine) else {
            return Err(
                ModifyClusterSnapshotErrorModeled::DependentServiceUnavailableFault(
                    DependentServiceUnavailableFault {
                        message: Some(st::STORE_UNAVAILABLE.to_string()),
                    },
                )
                .into(),
            );
        };
        let started = ::service_common::clock::now(&*self.engine);
        let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());

        // A request carrying an empty identifier names no snapshot to modify,
        // which is a malformed request rather than a missing snapshot.
        let identifier = input.snapshot_identifier.as_str();
        if identifier.is_empty() {
            return Err(crate::scaffolded::Scaffolded::validation(
                "SnapshotIdentifier must be supplied.",
            ));
        }

        let mut store = state.lock();
        store.settle(started);

        let Some(index) = store.snapshot_located_position(Some(identifier), None) else {
            return Err(
                ModifyClusterSnapshotErrorModeled::ClusterSnapshotNotFoundFault(
                    ClusterSnapshotNotFoundFault {
                        message: Some(st::cluster_snapshot_not_found_message(
                            st::ClusterSnapshotNotFoundWording::NotFoundThenIdentifier,
                            identifier,
                        )),
                    },
                )
                .into(),
            );
        };

        let snapshot = &store.snapshots[index].snapshot;
        let stored_identifier = snapshot
            .snapshot_identifier
            .clone()
            .unwrap_or_else(|| identifier.to_string());
        // `InvalidClusterSnapshotStateFault` is "the specified cluster snapshot is
        // not in the available state": a snapshot still being taken, or one no
        // longer holding its data, carries no settings to change.
        if snapshot.status.as_deref() != Some(st::STATUS_AVAILABLE) {
            return Err(
                ModifyClusterSnapshotErrorModeled::InvalidClusterSnapshotStateFault(
                    InvalidClusterSnapshotStateFault {
                        message: Some(st::invalid_snapshot_state_message(
                            &stored_identifier,
                            snapshot.status.as_deref().unwrap_or_default(),
                        )),
                    },
                )
                .into(),
            );
        }
        // The only setting this operation changes is how long a MANUAL snapshot
        // is kept: `Snapshot$ManualSnapshotRetentionPeriod` is "the number of
        // days that a MANUAL snapshot is retained", and how long Redshift keeps a
        // snapshot it took on its own schedule is the source cluster's
        // `AutomatedSnapshotRetentionPeriod`, which `ModifyCluster` sets.
        if snapshot.snapshot_type.as_deref() != Some(st::SNAPSHOT_TYPE_MANUAL) {
            return Err(
                ModifyClusterSnapshotErrorModeled::InvalidClusterSnapshotStateFault(
                    InvalidClusterSnapshotStateFault {
                        message: Some(format!(
                            "Cluster snapshot {stored_identifier} is of type {}, and only a {} snapshot carries a manual retention period.",
                            snapshot.snapshot_type.as_deref().unwrap_or_default(),
                            st::SNAPSHOT_TYPE_MANUAL
                        )),
                    },
                )
                .into(),
            );
        }

        // A request that names no setting changes none, and reports the snapshot
        // exactly as it stands.
        let Some(period) = input.manual_snapshot_retention_period else {
            return Ok(ModifyClusterSnapshotOutput {
                snapshot: Some(snapshot.clone()),
            });
        };
        if !st::manual_retention_admitted(period) {
            return Err(
                ModifyClusterSnapshotErrorModeled::InvalidRetentionPeriodFault(
                    InvalidRetentionPeriodFault {
                        message: Some(format!(
                            "Invalid value for ManualSnapshotRetentionPeriod: {period}. It must be either {} or an integer between {} and {}.",
                            st::MANUAL_RETENTION_INDEFINITE,
                            st::MANUAL_RETENTION_BOUNDS.0,
                            st::MANUAL_RETENTION_BOUNDS.1
                        )),
                    },
                )
                .into(),
            );
        }

        // A period that has already passed for this snapshot would leave it
        // outside its own retention window: refused, unless the request forces
        // it, which deletes the snapshot straight away.
        if st::manual_retention_has_passed(snapshot.snapshot_create_time, period, now) {
            if !input.force.unwrap_or(false) {
                return Err(
                    ModifyClusterSnapshotErrorModeled::InvalidRetentionPeriodFault(
                        InvalidRetentionPeriodFault {
                            message: Some(format!(
                                "Cluster snapshot {stored_identifier} falls outside a manual retention period of {period} day(s), which is already in the past. Specify Force to delete the snapshot."
                            )),
                        },
                    )
                    .into(),
                );
            }
            let mut removed = store.snapshots.remove(index);
            st::apply_manual_retention(&mut removed.snapshot, period, now);
            // "DeleteClusterSnapshot returns status as \"deleted\"": a snapshot
            // this call removed is reported back in the state it left it in, and
            // the store no longer holds it.
            removed.snapshot.status = Some(st::STATUS_DELETED.to_string());
            return Ok(ModifyClusterSnapshotOutput {
                snapshot: Some(removed.snapshot),
            });
        }

        let stored = &mut store.snapshots[index];
        st::apply_manual_retention(&mut stored.snapshot, period, now);
        Ok(ModifyClusterSnapshotOutput {
            snapshot: Some(stored.snapshot.clone()),
        })
    }
}
