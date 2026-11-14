// SPDX-License-Identifier: CC0-1.0

//! @op BatchModifyClusterSnapshots
//!
//! Modifies the settings of a SET of snapshots in one call: the number of days
//! each named manual snapshot is retained.
//!
//! Two conditions belong to the REQUEST rather than to any one snapshot in it,
//! and refuse the whole call: a list longer than the published batch limit
//! ([`crate::state::BATCH_SNAPSHOT_IDENTIFIER_LIMIT`]) and a retention period
//! outside the published range. Everything else is decided PER ENTRY, against
//! the stored record, by exactly the rules `ModifyClusterSnapshot` applies to one
//! snapshot — the period is applied through
//! [`crate::state::apply_manual_retention`], the one rule for it, so a later
//! `DescribeClusterSnapshots` reports what this call persisted. An entry the call
//! acted on is listed in `Resources`; an entry it could not act on is reported in
//! `Errors` as a `SnapshotErrorMessage` carrying the wire code of the fault the
//! one-snapshot operation would have raised, and leaves its snapshot untouched —
//! one bad entry never fails the batch.
//!
//! A snapshot whose retention window has already ended under the requested period
//! is such an entry, unless the request carries `Force`, which deletes that
//! snapshot immediately instead.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchModifyClusterSnapshots`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json")
//! ```
//!
//! Modifies the settings for a set of cluster snapshots.
//!
//! ## Input
//!
//! * `Force` (Boolean) — A boolean value indicating whether to override an exception if the
//!   retention period has passed.
//! * `ManualSnapshotRetentionPeriod` (IntegerOptional) — The number of days that a manual
//!   snapshot is retained. If you specify the value -1, the manual snapshot is retained
//!   indefinitely. The number must be either -1 or an integer between 1 and 3,653. If you
//!   decrease the manual snapshot retention period from its current value, existing manual
//!   snapshots that fall outside of the new retention period will return an error. If you
//!   want to suppress the errors and delete the snapshots, use the force option.
//! * `SnapshotIdentifierList` (SnapshotIdentifierList) **required** — A list of snapshot
//!   identifiers you want to modify.
//!
//! ## Output
//!
//! * `Errors` (BatchSnapshotOperationErrors) — A list of any errors returned.
//! * `Resources` (SnapshotIdentifierList) — A list of the snapshots that were modified.
//!
//! ## Errors
//!
//! * `BatchModifyClusterSnapshotsLimitExceededFault` (HTTP 400) — The maximum number for
//!   snapshot identifiers has been reached. The limit is 100.
//! * `InvalidRetentionPeriodFault` (HTTP 400) — The retention period specified is either in
//!   the past or is not a valid value. The value must be either -1 or an integer between 1
//!   and 3,653.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchModifyClusterSnapshotsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchModifyClusterSnapshotsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `DependentServiceUnavailableFault` is widened in: the model does not list it for
// this operation, but the service declares the shape, and it is the only modelled
// way to say "the control-plane store could not be reached".
#[smithy_macros::operation_error(
    "com.amazonaws.redshift#BatchModifyClusterSnapshots",
    extra_errors = ["DependentServiceUnavailableFault"]
)]
#[derive(Debug)]
pub enum BatchModifyClusterSnapshotsErrorModeled {
    BatchModifyClusterSnapshotsLimitExceededFault(BatchModifyClusterSnapshotsLimitExceededFault),
    InvalidRetentionPeriodFault(InvalidRetentionPeriodFault),
    DependentServiceUnavailableFault(DependentServiceUnavailableFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_modify_cluster_snapshots::BatchModifyClusterSnapshotsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchModifyClusterSnapshotsError = crate::scaffolded::Scaffolded<BatchModifyClusterSnapshotsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchModifyClusterSnapshotsErrorModeled> for BatchModifyClusterSnapshotsError {
    fn from(modeled: BatchModifyClusterSnapshotsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchModifyClusterSnapshots` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchModifyClusterSnapshots<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchModifyClusterSnapshots<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchModifyClusterSnapshots<Eng>
{
    type Input = BatchModifyClusterSnapshotsInput;
    type Output = BatchModifyClusterSnapshotsOutput;
    type Error = BatchModifyClusterSnapshotsError;

    const NAME: &'static str = "BatchModifyClusterSnapshots";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_MODIFY_CLUSTER_SNAPSHOTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_MODIFY_CLUSTER_SNAPSHOTS_OUTPUT_SCHEMA;
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
                BatchModifyClusterSnapshotsErrorModeled::DependentServiceUnavailableFault(
                    DependentServiceUnavailableFault {
                        message: Some(st::STORE_UNAVAILABLE.to_string()),
                    },
                )
                .into(),
            );
        };
        let started = ::service_common::clock::now(&*self.engine);
        let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());

        // A list longer than the published limit is a property of the REQUEST, so
        // it refuses the whole call rather than reporting a per-entry failure.
        let requested_count = input.snapshot_identifier_list.len();
        if requested_count > st::BATCH_SNAPSHOT_IDENTIFIER_LIMIT {
            return Err(
                BatchModifyClusterSnapshotsErrorModeled::BatchModifyClusterSnapshotsLimitExceededFault(
                    BatchModifyClusterSnapshotsLimitExceededFault {
                        message: Some(format!(
                            "The maximum number of snapshot identifiers is {}; {requested_count} were supplied.",
                            st::BATCH_SNAPSHOT_IDENTIFIER_LIMIT
                        )),
                    },
                )
                .into(),
            );
        }
        // So is a retention period no snapshot could be given: it names no one
        // entry of the list either, so it too refuses the whole call.
        if let Some(period) = input.manual_snapshot_retention_period {
            if !st::manual_retention_admitted(period) {
                return Err(
                    BatchModifyClusterSnapshotsErrorModeled::InvalidRetentionPeriodFault(
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
        }

        let forced = input.force.unwrap_or(false);
        let mut store = state.lock();
        store.settle(started);

        let mut resources: Vec<String> = Vec::new();
        let mut errors: Vec<SnapshotErrorMessage> = Vec::new();

        // Each entry is decided against the store as it stands when that entry is
        // reached, in the order the request lists them, so a list naming one
        // snapshot twice sees its own first change.
        for requested in &input.snapshot_identifier_list {
            // An empty entry names no snapshot, so it locates none. It is
            // reported as this entry's own not-found failure rather than
            // refusing the request the way the one-snapshot path does: in a
            // batch, what ONE entry gets wrong must not fail the entries beside
            // it, and a list is a set of locators, not a single member whose
            // value the request stands or falls by.
            let locator = (!requested.is_empty()).then_some(requested.as_str());
            let found = locator.and_then(|id| store.snapshot_located_position(Some(id), None));
            let Some(index) = found else {
                errors.push(SnapshotErrorMessage {
                    snapshot_identifier: Some(requested.clone()),
                    snapshot_cluster_identifier: None,
                    failure_code: Some(st::SNAPSHOT_FAILURE_NOT_FOUND.to_string()),
                    failure_reason: Some(format!("Cluster snapshot {requested} not found.")),
                });
                continue;
            };

            // Read what the decision needs, then let the borrow go: an entry the
            // request forces removes the very record it was read from.
            let (identifier, cluster_identifier, status, snapshot_type, created_at) = {
                let snapshot = &store.snapshots[index].snapshot;
                (
                    snapshot
                        .snapshot_identifier
                        .clone()
                        .unwrap_or_else(|| requested.clone()),
                    snapshot.cluster_identifier.clone(),
                    snapshot.status.clone().unwrap_or_default(),
                    snapshot.snapshot_type.clone().unwrap_or_default(),
                    snapshot.snapshot_create_time,
                )
            };

            // The same two state rules `ModifyClusterSnapshot` refuses one
            // snapshot by, reported here as that entry's failure: a snapshot not
            // `available` — still being taken, or no longer holding its data —
            // carries no settings to change, and how long Redshift keeps a
            // snapshot it took on its own schedule is the source cluster's
            // `AutomatedSnapshotRetentionPeriod`, not a manual retention period.
            if status != st::STATUS_AVAILABLE {
                errors.push(SnapshotErrorMessage {
                    snapshot_identifier: Some(identifier.clone()),
                    snapshot_cluster_identifier: cluster_identifier,
                    failure_code: Some(st::SNAPSHOT_FAILURE_INVALID_STATE.to_string()),
                    failure_reason: Some(st::invalid_snapshot_state_message(&identifier, &status)),
                });
                continue;
            }
            if snapshot_type != st::SNAPSHOT_TYPE_MANUAL {
                errors.push(SnapshotErrorMessage {
                    snapshot_identifier: Some(identifier.clone()),
                    snapshot_cluster_identifier: cluster_identifier,
                    failure_code: Some(st::SNAPSHOT_FAILURE_INVALID_STATE.to_string()),
                    failure_reason: Some(format!(
                        "Cluster snapshot {identifier} is of type {snapshot_type}, and only a {} snapshot carries a manual retention period.",
                        st::SNAPSHOT_TYPE_MANUAL
                    )),
                });
                continue;
            }

            // A request that names no setting changes none, and still reports the
            // snapshots it named as the ones it acted on.
            let Some(period) = input.manual_snapshot_retention_period else {
                resources.push(identifier);
                continue;
            };

            // A period whose window has already ended would leave this snapshot
            // outside its own retention window: reported as that entry's failure,
            // unless the request forces it, which deletes the snapshot straight
            // away — the one case where this call changes what the store holds.
            if st::manual_retention_has_passed(created_at, period, now) {
                if !forced {
                    errors.push(SnapshotErrorMessage {
                        snapshot_identifier: Some(identifier.clone()),
                        snapshot_cluster_identifier: cluster_identifier,
                        failure_code: Some(
                            st::SNAPSHOT_FAILURE_INVALID_RETENTION_PERIOD.to_string(),
                        ),
                        failure_reason: Some(format!(
                            "Cluster snapshot {identifier} falls outside a manual retention period of {period} day(s), which is already in the past. Specify Force to delete the snapshot."
                        )),
                    });
                    continue;
                }
                store.snapshots.remove(index);
                resources.push(identifier);
                continue;
            }

            let stored = &mut store.snapshots[index];
            st::apply_manual_retention(&mut stored.snapshot, period, now);
            resources.push(identifier);
        }

        Ok(BatchModifyClusterSnapshotsOutput {
            resources: Some(resources),
            errors: Some(errors),
        })
    }
}
