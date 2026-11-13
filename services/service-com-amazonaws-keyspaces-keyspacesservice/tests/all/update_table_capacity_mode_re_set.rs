// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateTable` that carries a `capacitySpecification` does to a table's auto
//! scaling resources, driven end to end over both services' own wire protocols on ONE
//! shared engine.
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! The consequences a capacity-mode update has beyond the stored mode — the
//! `lastUpdateToPayPerRequestTimestamp` stamp, and the retirement of the table's auto
//! scaling settings here and of its scalable targets, scaling policies and their alarms
//! in Application Auto Scaling — follow from the table's throughput mode CHANGING, so
//! each fires only on the transition that produces it: the stamp on a move TO
//! `PAY_PER_REQUEST` (a table CREATED in that mode is stamped by its create), the
//! retirement on a move OUT of `PROVISIONED`. An update that re-sets the mode the table is
//! already in changed no throughput capacity, so it leaves all of them exactly as they
//! were.
//!
//! The rows differ along the mode the table starts in (the modelled default a create with
//! no `capacitySpecification` leaves, and each mode set explicitly), the mode the update
//! requests, which capacities have a scalable target, whether those targets were
//! registered through Amazon Keyspaces or through Application Auto Scaling's own API, the
//! unit bounds and target values, and the keyspace and table names.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::auto_scaling_propagation::{enabled, resource_id, Emulator};
use crate::column_types_and_auto_scaling::assert_member;
use crate::keyspace_table_lifecycle::settle_wait;

/// The capacity specification of a table in `mode`, as a request carries it:
/// `PROVISIONED` "requires readCapacityUnits and writeCapacityUnits as input" and
/// `PAY_PER_REQUEST` takes neither (`CapacitySpecification$throughputMode`,
/// `smithy_query(query="shape", shape="CapacitySpecification")`).
fn capacity(mode: &str, units: i64) -> Value {
    match mode == keyspaces::PROVISIONED_THROUGHPUT_MODE {
        true => json!({
            "throughputMode": mode,
            "readCapacityUnits": units,
            "writeCapacityUnits": units,
        }),
        false => json!({"throughputMode": mode}),
    }
}

/// One capacity's scalable target, as a row asks for it: the capacity, its unit bounds and
/// the target value of the target-tracking policy that steers it.
type Window = (keyspaces::TableCapacity, i64, i64, f64);

/// Where a row's targets come from: the two APIs that write the same resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Through {
    /// Amazon Keyspaces' own `autoScalingSpecification`, which only a table in
    /// provisioned capacity mode accepts.
    Keyspaces,
    /// Application Auto Scaling's `RegisterScalableTarget` + `PutScalingPolicy`, which
    /// registers a target on the table's provisioned throughput — so, like the
    /// specification above, it only applies to a table in provisioned capacity mode: a table
    /// in on-demand mode has no provisioned throughput for a target to scale, and that
    /// service refuses to register one on it.
    ApplicationAutoScaling,
}

/// One row: the mode a table starts in, the targets it holds, the mode an `UpdateTable`
/// then requests, and whether that update MOVES the table out of provisioned mode.
struct Row {
    case: &'static str,
    keyspace: &'static str,
    table: &'static str,
    /// The mode `CreateTable` asks for, or `None` to send no `capacitySpecification` at
    /// all and leave the table in the modelled default mode.
    created_in: Option<&'static str>,
    /// How the row's targets are registered.
    through: Through,
    /// The scalable targets the table holds when the update arrives.
    windows: Vec<Window>,
    /// The mode the `UpdateTable` requests.
    requested: &'static str,
    /// The units the request carries when it asks for provisioned mode.
    units: i64,
}

impl Row {
    /// The mode the table is in when the update arrives: the one the create asked for, or
    /// the modelled default when it asked for none.
    fn starting_mode(&self) -> &'static str {
        self.created_in
            .unwrap_or(keyspaces::DEFAULT_THROUGHPUT_MODE)
    }

    /// Whether this update takes the table OUT of provisioned mode — the one transition
    /// that retires the resources its capacities were scaled on.
    fn moves_out_of_provisioned(&self) -> bool {
        self.starting_mode() == keyspaces::PROVISIONED_THROUGHPUT_MODE
            && self.requested != keyspaces::PROVISIONED_THROUGHPUT_MODE
    }

    /// Whether this update is the move TO `PAY_PER_REQUEST` the summary timestamp is
    /// stamped by.
    fn moves_to_pay_per_request(&self) -> bool {
        self.starting_mode() != keyspaces::PAY_PER_REQUEST_THROUGHPUT_MODE
            && self.requested == keyspaces::PAY_PER_REQUEST_THROUGHPUT_MODE
    }

    /// Whether the table was CREATED in `PAY_PER_REQUEST` mode — explicitly or by the
    /// modelled default — so the create is what stamped the summary timestamp.
    fn starts_in_pay_per_request(&self) -> bool {
        self.starting_mode() == keyspaces::PAY_PER_REQUEST_THROUGHPUT_MODE
    }
}

/// Create `keyspace` and, in it, the row's table in its starting mode, with its scalable
/// targets registered through the API the row names, and wait out the transient statuses
/// so the table is `ACTIVE` and updatable.
async fn given_scaled_table(emulator: &Emulator, row: &Row) {
    let (status, body) = emulator
        .keyspaces("CreateKeyspace", json!({"keyspaceName": row.keyspace}))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateKeyspace to answer 200, got {status} {body}",
        row.case
    );
    let mut request = json!({
        "keyspaceName": row.keyspace,
        "tableName": row.table,
        "schemaDefinition": {
            "allColumns": [{"name": "pk", "type": "text"}],
            "partitionKeys": [{"name": "pk"}],
        },
    });
    if let Some(mode) = row.created_in {
        request["capacitySpecification"] = capacity(mode, 5);
    }
    if row.through == Through::Keyspaces {
        let mut specification = json!({});
        for (capacity, minimum, maximum, target_value) in &row.windows {
            let member = match capacity {
                keyspaces::TableCapacity::Read => "readCapacityAutoScaling",
                keyspaces::TableCapacity::Write => "writeCapacityAutoScaling",
            };
            specification[member] = enabled(*minimum, *maximum, *target_value);
        }
        request["autoScalingSpecification"] = specification;
    }
    let (status, body) = emulator.keyspaces("CreateTable", request).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateTable to answer 200, got {status} {body}",
        row.case
    );

    if row.through == Through::ApplicationAutoScaling {
        for (capacity, minimum, maximum, target_value) in &row.windows {
            let identity = json!({
                "ServiceNamespace": keyspaces::AUTO_SCALING_NAMESPACE,
                "ResourceId": resource_id(row.keyspace, row.table),
                "ScalableDimension": capacity.scalable_dimension(),
            });
            let mut registration = identity.clone();
            registration["MinCapacity"] = json!(minimum);
            registration["MaxCapacity"] = json!(maximum);
            let (status, body) = emulator
                .auto_scaling("RegisterScalableTarget", registration)
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected RegisterScalableTarget to answer 200, got {status} {body}",
                row.case
            );
            let mut put = identity;
            put["PolicyName"] = json!(format!("{}-by-hand", capacity.scalable_dimension()));
            put["PolicyType"] = json!("TargetTrackingScaling");
            put["TargetTrackingScalingPolicyConfiguration"] = json!({
                "TargetValue": target_value,
                "PredefinedMetricSpecification": {
                    "PredefinedMetricType": capacity.predefined_metric(),
                },
            });
            let (status, body) = emulator.auto_scaling("PutScalingPolicy", put).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected PutScalingPolicy to answer 200, got {status} {body}",
                row.case
            );
        }
    }
    // The table is updatable once the owning service reports it ACTIVE again — after its
    // own creation transition and after the capacity Application Auto Scaling moves it to
    // when a registered floor is above the capacity the table was created with.
    emulator
        .given_table_active(row.case, row.keyspace, row.table)
        .await;
}

/// The auto scaling settings Amazon Keyspaces reports for a table, and the table's own
/// throughput summary.
async fn reported(emulator: &Emulator, row: &Row) -> (Value, Value) {
    let (status, settings) = emulator
        .keyspaces(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": row.keyspace, "tableName": row.table}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected GetTableAutoScalingSettings to answer 200, got {status} {settings}",
        row.case
    );
    let (status, table) = emulator
        .keyspaces(
            "GetTable",
            json!({"keyspaceName": row.keyspace, "tableName": row.table}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected GetTable to answer 200, got {status} {table}",
        row.case
    );
    (
        settings
            .get("autoScalingSpecification")
            .cloned()
            .unwrap_or(Value::Null),
        table
            .get("capacitySpecification")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

/// RULE: a capacity-mode update's consequences beyond the stored mode are consequences of
/// the mode CHANGING — the auto scaling resources of the table's capacities are retired
/// only by an update that moves the table OUT of provisioned mode, and the
/// `lastUpdateToPayPerRequestTimestamp` is stamped only by one that moves it TO
/// `PAY_PER_REQUEST` — so an update that re-sets the mode the table is already in leaves
/// the table's auto scaling settings, the Application Auto Scaling targets and policies
/// they are, and that stamp exactly as they were.
#[tokio::test]
async fn update_table_retires_auto_scaling_only_on_a_move_out_of_provisioned_mode() {
    let read = keyspaces::TableCapacity::Read;
    let write = keyspaces::TableCapacity::Write;
    let provisioned = keyspaces::PROVISIONED_THROUGHPUT_MODE;
    let pay_per_request = keyspaces::PAY_PER_REQUEST_THROUGHPUT_MODE;
    let rows = vec![
        Row {
            case: "re-set the default mode a create left the table in",
            keyspace: "re_set_default_ks",
            table: "ledger",
            created_in: None,
            through: Through::ApplicationAutoScaling,
            // A table in on-demand mode holds no scalable target: there is no provisioned
            // throughput for one to scale, so registering one on it is refused.
            windows: vec![],
            requested: pay_per_request,
            units: 0,
        },
        Row {
            case: "re-set an explicit PAY_PER_REQUEST, which holds no target",
            keyspace: "re_set_on_demand_ks9",
            table: "orders_by_customer_1",
            created_in: Some(pay_per_request),
            through: Through::ApplicationAutoScaling,
            windows: vec![],
            requested: pay_per_request,
            units: 0,
        },
        Row {
            case: "re-set PROVISIONED on a provisioned table scaled through Keyspaces",
            keyspace: "re_set_provisioned_ks",
            table: "t",
            created_in: Some(provisioned),
            through: Through::Keyspaces,
            windows: vec![(read, 2, 20, 50.0), (write, 1, 9, 65.0)],
            requested: provisioned,
            units: 11,
        },
        Row {
            case: "move INTO provisioned mode from a table that holds no target",
            keyspace: "move_in_ks",
            table: "Mixed_Case_9",
            created_in: Some(pay_per_request),
            through: Through::ApplicationAutoScaling,
            windows: vec![],
            requested: provisioned,
            units: 8,
        },
        Row {
            case: "move OUT of provisioned mode, both capacities scaled",
            keyspace: "move_out_both_ks",
            table: "scaled_tbl",
            created_in: Some(provisioned),
            through: Through::Keyspaces,
            windows: vec![(read, 5, 50, 60.0), (write, 5, 50, 60.0)],
            requested: pay_per_request,
            units: 0,
        },
        Row {
            case: "move OUT of provisioned mode, one capacity scaled by hand",
            keyspace: "move_out_one_ks",
            table: "ledger_2",
            created_in: Some(provisioned),
            through: Through::ApplicationAutoScaling,
            windows: vec![(read, 9, 99, 75.0)],
            requested: pay_per_request,
            units: 0,
        },
    ];

    for row in &rows {
        let case = row.case;
        // One engine per row, so what the row's own table holds is all that is reported.
        let emulator = Emulator::start();
        given_scaled_table(&emulator, row).await;

        // What the table and Application Auto Scaling hold before the update.
        let targets_before = emulator.cassandra_targets().await;
        let policies_before = emulator.cassandra_policies().await;
        assert_eq!(
            targets_before.len(),
            row.windows.len(),
            "case {case:?}: expected the row's {} target(s) to be registered before the \
             update, got {targets_before:?}",
            row.windows.len()
        );
        assert_eq!(
            policies_before.len(),
            row.windows.len(),
            "case {case:?}: expected the row's {} policy/policies before the update, got \
             {policies_before:?}",
            row.windows.len()
        );
        let (settings_before, capacity_before) = reported(&emulator, row).await;
        let stamp_before = capacity_before
            .get("lastUpdateToPayPerRequestTimestamp")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            stamp_before != Value::Null,
            row.starts_in_pay_per_request(),
            "case {case:?}: before any update, the stamp belongs to a table CREATED in \
             PAY_PER_REQUEST (created in it: {}), got {capacity_before}",
            row.starts_in_pay_per_request()
        );

        let (status, body) = emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": row.keyspace,
                    "tableName": row.table,
                    "capacitySpecification": capacity(row.requested, row.units),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateTable to answer 200, got {status} {body}"
        );
        tokio::time::sleep(settle_wait()).await;

        let targets_after = emulator.cassandra_targets().await;
        let policies_after = emulator.cassandra_policies().await;
        let (settings_after, capacity_after) = reported(&emulator, row).await;

        // Whatever the update did to the auto scaling resources, it applied the mode it
        // was asked for.
        assert_member(
            case,
            &capacity_after,
            "/throughputMode",
            &json!(row.requested),
        );

        if row.moves_out_of_provisioned() {
            assert!(
                targets_after.is_empty(),
                "case {case:?}: expected a move out of provisioned mode to leave no \
                 scalable target, got {targets_after:?}"
            );
            assert!(
                policies_after.is_empty(),
                "case {case:?}: expected a move out of provisioned mode to leave no \
                 scaling policy, got {policies_after:?}"
            );
            assert_eq!(
                settings_after,
                Value::Null,
                "case {case:?}: expected a move out of provisioned mode to leave the table \
                 no auto scaling settings, got {settings_after}"
            );
        } else {
            assert_eq!(
                targets_after, targets_before,
                "case {case:?}: expected the scalable targets to be untouched by an update \
                 that did not move the table out of provisioned mode, got {targets_after:?}"
            );
            assert_eq!(
                policies_after, policies_before,
                "case {case:?}: expected the scaling policies to be untouched by an update \
                 that did not move the table out of provisioned mode, got {policies_after:?}"
            );
            assert_eq!(
                settings_after, settings_before,
                "case {case:?}: expected the reported auto scaling settings to be unchanged, \
                 got {settings_after}"
            );
        }

        // The sibling consequence in the same block, under the same rule: the stamp moves
        // on a move TO `PAY_PER_REQUEST`, and on nothing else — a re-set of the mode the
        // table already holds leaves it exactly where the create put it.
        let stamp = capacity_after
            .get("lastUpdateToPayPerRequestTimestamp")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            stamp != Value::Null,
            row.moves_to_pay_per_request() || row.starts_in_pay_per_request(),
            "case {case:?}: expected lastUpdateToPayPerRequestTimestamp exactly on a table \
             that has been in PAY_PER_REQUEST (moved: {}, created in it: {}), got {stamp}",
            row.moves_to_pay_per_request(),
            row.starts_in_pay_per_request()
        );
        if !row.moves_to_pay_per_request() {
            assert_eq!(
                stamp, stamp_before,
                "case {case:?}: an update that did not move the table TO PAY_PER_REQUEST \
                 leaves the stamp where it was ({stamp_before}), got {stamp}"
            );
        }
    }
}
