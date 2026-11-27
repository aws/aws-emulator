// SPDX-License-Identifier: CC0-1.0

//! The connection test a data source's create and update settle on, what a data set may
//! be built on, and the connection target a write must declare before either runs.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! request, QuickSight over restJson1 and Athena over awsJson1_1 on the same emulator —
//! so what is asserted is what an SDK client would see.
//!
//! The first rule under test: a write does not register a connection target the service
//! cannot route to. An address outside the globally routable unicast space is refused
//! synchronously with `InvalidParameterValueException` naming that address, and nothing
//! is written — unless the request supplies the VPC connection that reaches it. A target
//! named by anything but a bare address carries no address to classify.
//!
//! The second rule under test: the TERMINAL `ResourceStatus` of a data source is the
//! outcome of the connection test of the source its `DataSourceParameters` describe. A
//! test that resolves settles the resource successfully with no `ErrorInfo`; one that
//! names a resource the owning service does not hold settles it in the failed status of
//! that write — `CREATION_FAILED` for a create, `UPDATE_FAILED` for an update — carrying
//! `ErrorInfo` of type `GENERIC_SQL_FAILURE` whose message names what was missing. A
//! data source in a failed status cannot back a data set, so a physical table naming
//! one is refused with `InvalidParameterValueException`.
//!
//! The third rule under test: `DataSourceParameters` is a variant type whose member IS
//! the kind of source it configures, so a write whose parameter set belongs to a kind
//! other than the data source's `DataSourceType` is refused synchronously with
//! `InvalidParameterValueException` naming the member supplied and the kind it
//! configures, and writes nothing — on the create against the requested kind and on the
//! update against the kind the stored data source is of. A refused create leaves no
//! record at all, so the id it named reads back as not found and is still free; that
//! cross-check is asserted for every kind the model's `DataSourceType` enum names.
//!
//! The rows therefore vary what the rule generalises over: which parameter set the
//! union carries, whether the named workgroup exists (the account's default, one
//! Athena's own `CreateWorkGroup` registered, and names nothing holds), whether the
//! name is submitted at all, the write that ran the test (create or update), the
//! physical-table kind and the import mode of the data set built on the result, and
//! the account.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::ops::types as qs_types;
use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account every case uses unless it is about account keying.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

/// `ResourceStatus` values (model enum `com.amazonaws.quicksight#ResourceStatus`).
const CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";
const CREATION_SUCCESSFUL: &str = "CREATION_SUCCESSFUL";
const CREATION_FAILED: &str = "CREATION_FAILED";
const UPDATE_SUCCESSFUL: &str = "UPDATE_SUCCESSFUL";
const UPDATE_FAILED: &str = "UPDATE_FAILED";

/// The `DataSourceErrorInfoType` a refused connection test reports (model enum
/// `com.amazonaws.quicksight#DataSourceErrorInfoType`, whose members are
/// ACCESS_DENIED, COPY_SOURCE_NOT_FOUND, TIMEOUT, ENGINE_VERSION_NOT_SUPPORTED,
/// UNKNOWN_HOST, GENERIC_SQL_FAILURE, CONFLICT and UNKNOWN).
const GENERIC_SQL_FAILURE: &str = "GENERIC_SQL_FAILURE";

/// Athena's own default workgroup: the one every account has without creating it.
const DEFAULT_WORK_GROUP: &str = "primary";

fn data_source_arn(account: &str, id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:datasource/{id}")
}

async fn create_data_source(harness: &Harness, account: &str, body: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(body),
        )
        .await
}

async fn update_data_source(
    harness: &Harness,
    account: &str,
    id: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/accounts/{account}/data-sources/{id}"),
            Some(body),
        )
        .await
}

async fn describe_data_source(harness: &Harness, account: &str, id: &str) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/data-sources/{id}"),
            None,
        )
        .await
}

async fn create_data_set(
    harness: &Harness,
    account: &str,
    id: &str,
    import_mode: &str,
    physical_table_map: Value,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sets"),
            Some(json!({
                "DataSetId": id,
                "Name": id,
                "ImportMode": import_mode,
                "PhysicalTableMap": physical_table_map,
            })),
        )
        .await
}

/// One row of the connection-test table.
struct Case {
    /// What this row varies, named in every assertion it makes.
    label: &'static str,
    /// The `DataSourceType` the data source is of — the kind the row's parameter set
    /// belongs to, since a parameter set of another kind is refused outright.
    kind: &'static str,
    /// The `DataSourceParameters` the create submits, or `None` for a create that
    /// submits none at all.
    parameters: Option<Value>,
    /// The workgroup name the failure message has to name, or `None` when the test is
    /// expected to pass.
    missing_work_group: Option<&'static str>,
}

/// The rows: the union member the parameters carry, and — for the member that names a
/// resource this emulator holds — whether the account holds it.
///
/// `reporting` is registered through Athena's own `CreateWorkGroup` by the test below;
/// `primary` is the default workgroup Athena gives every account.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the account's default Athena workgroup",
            kind: "ATHENA",
            parameters: Some(json!({ "AthenaParameters": { "WorkGroup": DEFAULT_WORK_GROUP } })),
            missing_work_group: None,
        },
        Case {
            label: "an Athena workgroup the account created",
            kind: "ATHENA",
            parameters: Some(json!({ "AthenaParameters": { "WorkGroup": "reporting" } })),
            missing_work_group: None,
        },
        Case {
            label: "no WorkGroup at all, so the default one",
            kind: "ATHENA",
            parameters: Some(json!({
                "AthenaParameters": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/quicksight-athena")
                }
            })),
            missing_work_group: None,
        },
        Case {
            label: "no DataSourceParameters at all",
            kind: "ATHENA",
            parameters: None,
            missing_work_group: None,
        },
        Case {
            label: "a parameter set naming a source outside this emulator",
            kind: "POSTGRESQL",
            parameters: Some(json!({
                "PostgreSqlParameters": { "Host": "db.internal", "Port": 5432, "Database": "sales" }
            })),
            missing_work_group: None,
        },
        Case {
            label: "an Athena workgroup nothing created",
            kind: "ATHENA",
            parameters: Some(json!({ "AthenaParameters": { "WorkGroup": "no-such-workgroup" } })),
            missing_work_group: Some("no-such-workgroup"),
        },
        Case {
            label: "another Athena workgroup nothing created, so another message",
            kind: "ATHENA",
            parameters: Some(json!({
                "AthenaParameters": {
                    "WorkGroup": "finance-eu",
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/quicksight-athena")
                }
            })),
            missing_work_group: Some("finance-eu"),
        },
        Case {
            label: "a workgroup whose name differs from a held one only in case",
            kind: "ATHENA",
            parameters: Some(json!({ "AthenaParameters": { "WorkGroup": "PRIMARY" } })),
            missing_work_group: Some("PRIMARY"),
        },
    ]
}

/// The `ErrorInfo` a read reports, asserted against what the row expects.
fn assert_error_info(label: &str, source: &Value, missing_work_group: Option<&str>) {
    match missing_work_group {
        None => assert_eq!(
            source["ErrorInfo"],
            Value::Null,
            "case {label:?}: a connection test that passed carries no ErrorInfo, got {source}"
        ),
        Some(work_group) => {
            assert_eq!(
                source["ErrorInfo"]["Type"],
                json!(GENERIC_SQL_FAILURE),
                "case {label:?}: expected ErrorInfo.Type {GENERIC_SQL_FAILURE}, got {source}"
            );
            let message = source["ErrorInfo"]["Message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: expected an ErrorInfo.Message: {source}"));
            assert!(
                message.contains(work_group),
                "case {label:?}: expected the message to name the workgroup {work_group:?}, got {message:?}"
            );
            assert!(
                message.to_lowercase().contains("workgroup"),
                "case {label:?}: expected the message to be about the workgroup, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_settles_on_the_outcome_of_the_connection_test() {
    let harness = Harness::new();
    harness.create_athena_work_group("reporting").await;

    for (index, case) in cases().into_iter().enumerate() {
        let Case {
            label,
            kind,
            parameters,
            missing_work_group,
        } = case;
        let id = format!("ds-create-{index}");
        let mut body = json!({ "DataSourceId": id, "Name": id, "Type": kind });
        if let Some(parameters) = parameters.clone() {
            body["DataSourceParameters"] = parameters;
        }

        let (status, response) = create_data_source(&harness, ACCOUNT, body).await;
        assert_eq!(
            status, 202,
            "case {label:?}: the create is accepted as asynchronous work, got {status} with {response}"
        );
        assert_eq!(
            response["CreationStatus"],
            json!(CREATION_IN_PROGRESS),
            "case {label:?}: the create reports the non-terminal status, got {response}"
        );

        // Until the clock owes the transition, the test has no reported outcome: the
        // status is the non-terminal one and no ErrorInfo is reported, whichever way
        // the test will settle.
        let (_, in_flight) = describe_data_source(&harness, ACCOUNT, &id).await;
        assert_eq!(
            in_flight["DataSource"]["Status"],
            json!(CREATION_IN_PROGRESS),
            "case {label:?}: before the span is over the create is still in progress: {in_flight}"
        );
        assert_eq!(
            in_flight["DataSource"]["ErrorInfo"],
            Value::Null,
            "case {label:?}: a write in flight reports no ErrorInfo yet: {in_flight}"
        );

        harness.settle_data_source(ACCOUNT, &id);
        let (status, settled) = describe_data_source(&harness, ACCOUNT, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: describe expected 200, got {status} with {settled}"
        );
        let source = &settled["DataSource"];
        let expected_status = if missing_work_group.is_some() {
            CREATION_FAILED
        } else {
            CREATION_SUCCESSFUL
        };
        assert_eq!(
            source["Status"],
            json!(expected_status),
            "case {label:?}: expected the settled Status {expected_status}, got {settled}"
        );
        assert_eq!(
            source["DataSourceParameters"],
            parameters.unwrap_or(Value::Null),
            "case {label:?}: the submitted parameters are stored verbatim: {settled}"
        );
        assert_error_info(label, source, missing_work_group);
    }
}

#[tokio::test]
async fn an_update_settles_on_the_outcome_of_its_own_connection_test() {
    let harness = Harness::new();
    harness.create_athena_work_group("reporting").await;

    for (index, case) in cases().into_iter().enumerate() {
        let Case {
            label,
            kind,
            parameters,
            missing_work_group,
        } = case;
        let id = format!("ds-update-{index}");
        // Every row starts from a data source of the row's own kind whose create PASSED
        // its test, so what the row observes is the outcome of the update's own test:
        // the account's default Athena workgroup for an ATHENA row, and for a row of
        // another kind its own parameters, which name a source outside this emulator and
        // so have nothing to resolve.
        let fixture_parameters = if kind == "ATHENA" {
            json!({ "AthenaParameters": { "WorkGroup": DEFAULT_WORK_GROUP } })
        } else {
            parameters
                .clone()
                .unwrap_or_else(|| panic!("case {label:?}: a row of another kind submits its own parameters"))
        };
        let (status, response) = create_data_source(
            &harness,
            ACCOUNT,
            json!({
                "DataSourceId": id,
                "Name": id,
                "Type": kind,
                "DataSourceParameters": fixture_parameters,
            }),
        )
        .await;
        assert_eq!(status, 202, "case {label:?}: fixture create: {response}");
        harness.settle_data_source(ACCOUNT, &id);

        let mut body = json!({ "Name": id });
        if let Some(parameters) = parameters.clone() {
            body["DataSourceParameters"] = parameters;
        }
        let (status, response) = update_data_source(&harness, ACCOUNT, &id, body).await;
        assert_eq!(
            status, 202,
            "case {label:?}: the update is accepted, got {status} with {response}"
        );

        harness.settle_data_source(ACCOUNT, &id);
        let (_, settled) = describe_data_source(&harness, ACCOUNT, &id).await;
        let source = &settled["DataSource"];
        let expected_status = if missing_work_group.is_some() {
            UPDATE_FAILED
        } else {
            UPDATE_SUCCESSFUL
        };
        assert_eq!(
            source["Status"],
            json!(expected_status),
            "case {label:?}: expected the settled Status {expected_status}, got {settled}"
        );
        assert_error_info(label, source, missing_work_group);
    }
}

#[tokio::test]
async fn an_update_that_passes_clears_the_error_a_failed_create_left() {
    let harness = Harness::new();
    harness.create_athena_work_group("reporting").await;

    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({
            "DataSourceId": "ds-recovers",
            "Name": "recovers",
            "Type": "ATHENA",
            "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "not-yet-created" } },
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");
    harness.settle_data_source(ACCOUNT, "ds-recovers");
    let (_, failed) = describe_data_source(&harness, ACCOUNT, "ds-recovers").await;
    assert_eq!(
        failed["DataSource"]["Status"],
        json!(CREATION_FAILED),
        "the fixture must start from a failed connection test: {failed}"
    );

    // Rows: the same data source pointed at each workgroup the account holds, then at
    // one it does not — so the error follows the current test, not the first one.
    let rows: [(&str, &str, &str); 3] = [
        ("onto the default workgroup", DEFAULT_WORK_GROUP, UPDATE_SUCCESSFUL),
        ("onto a created workgroup", "reporting", UPDATE_SUCCESSFUL),
        ("back onto a missing workgroup", "gone-again", UPDATE_FAILED),
    ];
    for (label, work_group, expected_status) in rows {
        let (status, body) = update_data_source(
            &harness,
            ACCOUNT,
            "ds-recovers",
            json!({
                "Name": "recovers",
                "DataSourceParameters": { "AthenaParameters": { "WorkGroup": work_group } },
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: the update is accepted, got {status} with {body}"
        );
        harness.settle_data_source(ACCOUNT, "ds-recovers");
        let (_, settled) = describe_data_source(&harness, ACCOUNT, "ds-recovers").await;
        let source = &settled["DataSource"];
        assert_eq!(
            source["Status"],
            json!(expected_status),
            "case {label:?}: expected the settled Status {expected_status}, got {settled}"
        );
        let expected_missing = (expected_status == UPDATE_FAILED).then_some(work_group);
        match expected_missing {
            None => assert_eq!(
                source["ErrorInfo"],
                Value::Null,
                "case {label:?}: a test that passed clears the stored ErrorInfo, got {settled}"
            ),
            Some(work_group) => {
                let message = source["ErrorInfo"]["Message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(work_group),
                    "case {label:?}: expected the message to name {work_group:?}, got {message:?}"
                );
            }
        }
    }
}

/// A workgroup Athena registers AFTER a data source failed on it does not rewrite that
/// data source's settled outcome: the test ran once, and only another write runs it
/// again.
#[tokio::test]
async fn a_settled_failure_is_not_rewritten_by_a_later_athena_create() {
    let harness = Harness::new();
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({
            "DataSourceId": "ds-late-workgroup",
            "Name": "late",
            "Type": "ATHENA",
            "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "late-arrival" } },
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");
    harness.settle_data_source(ACCOUNT, "ds-late-workgroup");
    harness.create_athena_work_group("late-arrival").await;

    let (_, settled) = describe_data_source(&harness, ACCOUNT, "ds-late-workgroup").await;
    assert_eq!(
        settled["DataSource"]["Status"],
        json!(CREATION_FAILED),
        "a read must not re-run the connection test: {settled}"
    );
}

/// A one-entry `PhysicalTableMap` of each modelled kind, on `arn`.
fn physical_table_maps(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CustomSql",
            json!({ "tbl1": { "CustomSql": {
                "DataSourceArn": arn, "Name": "q1", "SqlQuery": "SELECT 1 AS col",
                "Columns": [{ "Name": "col", "Type": "INTEGER" }] } } }),
        ),
        (
            "RelationalTable",
            json!({ "tbl1": { "RelationalTable": {
                "DataSourceArn": arn, "Schema": "public", "Name": "orders",
                "InputColumns": [{ "Name": "order_id", "Type": "INTEGER" }] } } }),
        ),
        (
            "S3Source",
            json!({ "tbl1": { "S3Source": {
                "DataSourceArn": arn,
                "InputColumns": [{ "Name": "col", "Type": "STRING" }] } } }),
        ),
    ]
}

#[tokio::test]
async fn a_data_set_on_a_data_source_whose_connection_failed_is_refused() {
    let harness = Harness::new();

    // Two failed data sources, one failed by its create and one by its update, so the
    // refusal reports whichever failed status the data source is actually in.
    for (id, status_after) in [("ds-bad-create", CREATION_FAILED), ("ds-bad-update", UPDATE_FAILED)]
    {
        let (code, body) = create_data_source(
            &harness,
            ACCOUNT,
            json!({
                "DataSourceId": id,
                "Name": id,
                "Type": "ATHENA",
                "DataSourceParameters": { "AthenaParameters": { "WorkGroup": match status_after {
                    CREATION_FAILED => "missing-at-create",
                    _ => DEFAULT_WORK_GROUP,
                } } },
            }),
        )
        .await;
        assert_eq!(code, 202, "fixture {id}: create got {code} with {body}");
        harness.settle_data_source(ACCOUNT, id);
        if status_after == UPDATE_FAILED {
            let (code, body) = update_data_source(
                &harness,
                ACCOUNT,
                id,
                json!({
                    "Name": id,
                    "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "missing-at-update" } },
                }),
            )
            .await;
            assert_eq!(code, 202, "fixture {id}: update got {code} with {body}");
            harness.settle_data_source(ACCOUNT, id);
        }
        let (_, settled) = describe_data_source(&harness, ACCOUNT, id).await;
        assert_eq!(
            settled["DataSource"]["Status"],
            json!(status_after),
            "fixture {id}: expected {status_after}, got {settled}"
        );
    }

    // Rows: each physical-table kind, each import mode, and each of the two failed
    // statuses — the rule is about the data source's status, not about either.
    let mut data_set_index = 0;
    for (source_id, source_status) in
        [("ds-bad-create", CREATION_FAILED), ("ds-bad-update", UPDATE_FAILED)]
    {
        let arn = data_source_arn(ACCOUNT, source_id);
        for (kind, physical_table_map) in physical_table_maps(&arn) {
            for import_mode in ["SPICE", "DIRECT_QUERY"] {
                let label = format!("{source_status} / {kind} / {import_mode}");
                data_set_index += 1;
                let data_set_id = format!("dset-{data_set_index}");
                let (status, body) = create_data_set(
                    &harness,
                    ACCOUNT,
                    &data_set_id,
                    import_mode,
                    physical_table_map.clone(),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected 400 for a failed data source, got {status} with {body}"
                );
                assert_eq!(
                    body["__type"],
                    json!("InvalidParameterValueException"),
                    "case {label:?}: expected InvalidParameterValueException, got {body}"
                );
                assert_eq!(
                    body["Message"],
                    json!(format!("DataSource {arn} is in status {source_status}")),
                    "case {label:?}: unexpected refusal message: {body}"
                );
                // The refused call stored nothing.
                let (status, body) = harness
                    .call(
                        "GET",
                        &format!("/accounts/{ACCOUNT}/data-sets/{data_set_id}"),
                        None,
                    )
                    .await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must store no data set, got {status} with {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_data_set_on_a_data_source_whose_connection_passed_is_accepted() {
    let harness = Harness::new();
    harness.create_athena_work_group("reporting").await;

    // Rows: a source that passed its create's test, one that passed an update's test,
    // one with no parameters at all, and one belonging to another account — each may
    // back a data set. The last row proves the refusal is not about the account.
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({
            "DataSourceId": "ds-ok",
            "Name": "ok",
            "Type": "ATHENA",
            "DataSourceParameters": { "AthenaParameters": { "WorkGroup": DEFAULT_WORK_GROUP } },
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture ds-ok: got {status} with {body}");
    harness.settle_data_source(ACCOUNT, "ds-ok");

    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({ "DataSourceId": "ds-updated", "Name": "updated", "Type": "ATHENA" }),
    )
    .await;
    assert_eq!(status, 202, "fixture ds-updated: got {status} with {body}");
    harness.settle_data_source(ACCOUNT, "ds-updated");
    let (status, body) = update_data_source(
        &harness,
        ACCOUNT,
        "ds-updated",
        json!({
            "Name": "updated",
            "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "reporting" } },
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture ds-updated: update got {status} with {body}");
    harness.settle_data_source(ACCOUNT, "ds-updated");

    let (status, body) = create_data_source(
        &harness,
        OTHER_ACCOUNT,
        json!({ "DataSourceId": "ds-elsewhere", "Name": "elsewhere", "Type": "S3" }),
    )
    .await;
    assert_eq!(status, 202, "fixture ds-elsewhere: got {status} with {body}");
    harness.settle_data_source(OTHER_ACCOUNT, "ds-elsewhere");

    let rows: [(&str, &str, &str, &str); 4] = [
        ("a create that passed", ACCOUNT, "ds-ok", "SPICE"),
        ("an update that passed", ACCOUNT, "ds-updated", "SPICE"),
        ("a source still being created", ACCOUNT, "ds-in-flight", "DIRECT_QUERY"),
        ("another account's source", OTHER_ACCOUNT, "ds-elsewhere", "DIRECT_QUERY"),
    ];
    for (index, (label, account, source_id, import_mode)) in rows.into_iter().enumerate() {
        if source_id == "ds-in-flight" {
            // A create whose span is not over has not failed its test, so it is not
            // refused: only a FAILED status is.
            let (status, body) = create_data_source(
                &harness,
                account,
                json!({ "DataSourceId": source_id, "Name": source_id, "Type": "ATHENA" }),
            )
            .await;
            assert_eq!(status, 202, "case {label:?}: fixture create: {body}");
            let (_, in_flight) = describe_data_source(&harness, account, source_id).await;
            assert_eq!(
                in_flight["DataSource"]["Status"],
                json!(CREATION_IN_PROGRESS),
                "case {label:?}: the fixture must still be in flight: {in_flight}"
            );
        }
        let arn = data_source_arn(account, source_id);
        let data_set_id = format!("dset-ok-{index}");
        let (status, body) = create_data_set(
            &harness,
            account,
            &data_set_id,
            import_mode,
            json!({ "tbl1": { "CustomSql": {
                "DataSourceArn": arn, "Name": "q1", "SqlQuery": "SELECT 1 AS col",
                "Columns": [{ "Name": "col", "Type": "INTEGER" }] } } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the data set to be created, got {status} with {body}"
        );
        let (status, read_back) = harness
            .call(
                "GET",
                &format!("/accounts/{account}/data-sets/{data_set_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the created data set must read back, got {status} with {read_back}"
        );
        assert_eq!(
            read_back["DataSet"]["ImportMode"],
            json!(import_mode),
            "case {label:?}: the data set lost its ImportMode: {read_back}"
        );
    }
}

/// The failed outcome is in the STORE, not only in the response: the record carries the
/// failed settled status and its error, which is what every reader answers from.
#[tokio::test]
async fn the_failed_outcome_is_stored_on_the_record() {
    let harness = Harness::new();
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({
            "DataSourceId": "ds-stored",
            "Name": "stored",
            "Type": "ATHENA",
            "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "absent-workgroup" } },
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");

    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    let record = store
        .data_sources
        .get(&(ACCOUNT.to_string(), "ds-stored".to_string()))
        .expect("the created data source");
    assert_eq!(
        record.settled_status, CREATION_FAILED,
        "expected the stored settled status {CREATION_FAILED}, got {record:?}"
    );
    let error_info = record
        .error_info
        .as_ref()
        .unwrap_or_else(|| panic!("expected the stored record to carry an error: {record:?}"));
    assert_eq!(
        error_info.r#type.as_deref(),
        Some(GENERIC_SQL_FAILURE),
        "expected the stored error type {GENERIC_SQL_FAILURE}, got {error_info:?}"
    );
    assert!(
        error_info
            .message
            .as_deref()
            .is_some_and(|message| message.contains("absent-workgroup")),
        "expected the stored message to name the workgroup, got {error_info:?}"
    );
}

/// The refusal a write whose declared connection target cannot be routed to carries:
/// fixed text and the address the request submitted, with no trailing period.
fn unroutable_message(host: &str) -> String {
    format!("Unable to route to the host address {host}")
}

/// How many data sources the store holds.
fn held_data_sources(harness: &Harness) -> usize {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    store.data_sources.len()
}

/// The rows of the unroutable-address table: the parameter set carrying the address and
/// the address it carries.
///
/// The addresses span the blocks reserved for something other than global unicast — a
/// private network, the loopback, the link-local scope, the unspecified address, the
/// shared (carrier) space, documentation, benchmarking, the reserved top blocks and the
/// broadcast address, in both address families — and each is carried by a DIFFERENT
/// `DataSourceParameters` member, since every member that names its target by an address
/// names it in the same `Host` field. Each row's `Type` is a member of the model's
/// `DataSourceType` enum.
/// Source: smithy_query(query="shape", shape="DataSourceParameters") and
/// smithy_query(query="shape", shape="DataSourceType").
fn unroutable_rows() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "private-10",
            "MYSQL",
            "10.255.255.1",
            json!({ "MySqlParameters": { "Host": "10.255.255.1", "Port": 3306, "Database": "app" } }),
        ),
        (
            "private-172-16",
            "POSTGRESQL",
            "172.16.0.5",
            json!({ "PostgreSqlParameters": { "Host": "172.16.0.5", "Port": 5432, "Database": "sales" } }),
        ),
        (
            "private-192-168",
            "AURORA",
            "192.168.1.10",
            json!({ "AuroraParameters": { "Host": "192.168.1.10", "Port": 3306, "Database": "app" } }),
        ),
        (
            "loopback",
            "MARIADB",
            "127.0.0.1",
            json!({ "MariaDbParameters": { "Host": "127.0.0.1", "Port": 3306, "Database": "app" } }),
        ),
        (
            "link-local",
            "ORACLE",
            "169.254.169.254",
            json!({ "OracleParameters": { "Host": "169.254.169.254", "Port": 1521, "Database": "orcl" } }),
        ),
        (
            "unspecified",
            "SQLSERVER",
            "0.0.0.0",
            json!({ "SqlServerParameters": { "Host": "0.0.0.0", "Port": 1433, "Database": "app" } }),
        ),
        (
            "shared-carrier-space",
            "TERADATA",
            "100.64.0.1",
            json!({ "TeradataParameters": { "Host": "100.64.0.1", "Port": 1025, "Database": "app" } }),
        ),
        (
            "documentation",
            "PRESTO",
            "192.0.2.1",
            json!({ "PrestoParameters": { "Host": "192.0.2.1", "Port": 8080, "Catalog": "hive" } }),
        ),
        (
            "benchmarking",
            "SPARK",
            "198.18.0.1",
            json!({ "SparkParameters": { "Host": "198.18.0.1", "Port": 10000 } }),
        ),
        (
            "reserved-top-block",
            "SNOWFLAKE",
            "240.0.0.1",
            json!({ "SnowflakeParameters": { "Host": "240.0.0.1", "Database": "app", "Warehouse": "wh" } }),
        ),
        (
            "broadcast",
            "EXASOL",
            "255.255.255.255",
            json!({ "ExasolParameters": { "Host": "255.255.255.255", "Port": 8563 } }),
        ),
        (
            "ipv6-loopback",
            "TRINO",
            "::1",
            json!({ "TrinoParameters": { "Host": "::1", "Port": 8080, "Catalog": "hive" } }),
        ),
        (
            "ipv6-unique-local",
            "DATABRICKS",
            "fd00::1",
            json!({ "DatabricksParameters": { "Host": "fd00::1", "Port": 443, "SqlEndpointPath": "/sql/1.0/wh" } }),
        ),
        (
            "ipv6-link-local",
            "STARBURST",
            "fe80::1",
            json!({ "StarburstParameters": { "Host": "fe80::1", "Port": 443, "Catalog": "hive" } }),
        ),
        (
            "ipv6-documentation",
            "AURORA_POSTGRESQL",
            "2001:db8::1",
            json!({ "AuroraPostgreSqlParameters": { "Host": "2001:db8::1", "Port": 5432, "Database": "app" } }),
        ),
        (
            "optional-host-member",
            "REDSHIFT",
            "10.1.2.3",
            json!({ "RedshiftParameters": { "Host": "10.1.2.3", "Port": 5439, "Database": "dev" } }),
        ),
        // The same IPv4 hosts written in IPv6 form name the same hosts, so they are
        // read the same way: the mapped form, the compatible form and the 6to4 prefix
        // that tunnels to a private address.
        (
            "ipv4-mapped-private",
            "MYSQL",
            "::ffff:10.0.0.1",
            json!({ "MySqlParameters": { "Host": "::ffff:10.0.0.1", "Port": 3306, "Database": "app" } }),
        ),
        (
            "ipv4-compatible-loopback",
            "POSTGRESQL",
            "::127.0.0.1",
            json!({ "PostgreSqlParameters": { "Host": "::127.0.0.1", "Port": 5432, "Database": "sales" } }),
        ),
        (
            "6to4-to-a-private-address",
            "TERADATA",
            "2002:a00:1::1",
            json!({ "TeradataParameters": { "Host": "2002:a00:1::1", "Port": 1025, "Database": "app" } }),
        ),
        (
            "ipv6-discard-only",
            "SPARK",
            "100::1",
            json!({ "SparkParameters": { "Host": "100::1", "Port": 10000 } }),
        ),
    ]
}

#[tokio::test]
async fn a_create_naming_an_unroutable_address_is_refused_and_registers_nothing() {
    let harness = Harness::new();

    for (case, kind, host, parameters) in unroutable_rows() {
        let data_source_id = format!("ds-{case}");
        let before = held_data_sources(&harness);
        let (status, body) = create_data_source(
            &harness,
            ACCOUNT,
            json!({
                "DataSourceId": data_source_id,
                "Name": data_source_id,
                "Type": kind,
                "DataSourceParameters": parameters,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create naming an address with no route expected 400, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidParameterValueException"),
            "case {case:?}: the refused parameter value is what is reported: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(unroutable_message(host)),
            "case {case:?}: the refusal must name the address the request carried: {body}"
        );
        assert!(
            body.get("RequestId").is_none(),
            "case {case:?}: a refusal document reports no RequestId — the id of the call \
             travels in the x-amzn-RequestId header: {body}"
        );

        // The refused create registered nothing, so the id resolves to no data source.
        assert_eq!(
            held_data_sources(&harness),
            before,
            "case {case:?}: the refused create must write no record"
        );
        let (status, body) = describe_data_source(&harness, ACCOUNT, &data_source_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a data source the refused create did not register expected \
             404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: an id nothing was registered under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "DataSource {} is not found",
                data_source_arn(ACCOUNT, &data_source_id)
            )),
            "case {case:?}: the miss must name the data source by its ARN: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            Value::Null,
            "case {case:?}: the data-source miss reports no ResourceType: {body}"
        );
    }
}

#[tokio::test]
async fn a_target_the_service_can_reach_is_accepted() {
    let harness = Harness::new();

    // Rows: a target named by a DNS name (nothing here resolves one, so there is no
    // routing failure to report), one named by a globally routable address in either
    // family, one named by something that is no address at all, a parameter set that
    // declares no target, and a private address submitted WITH the VPC connection that
    // is how a caller reaches a source inside a VPC.
    let rows: Vec<(&str, &str, Value, Option<Value>)> = vec![
        (
            "dns-name",
            "MYSQL",
            json!({ "MySqlParameters": { "Host": "db.example.com", "Port": 3306, "Database": "app" } }),
            None,
        ),
        (
            "public-ipv4",
            "POSTGRESQL",
            json!({ "PostgreSqlParameters": { "Host": "8.8.8.8", "Port": 5432, "Database": "sales" } }),
            None,
        ),
        (
            "public-ipv6",
            "ORACLE",
            json!({ "OracleParameters": { "Host": "2606:4700::1111", "Port": 1521, "Database": "orcl" } }),
            None,
        ),
        (
            "ipv4-mapped-public",
            "MARIADB",
            json!({ "MariaDbParameters": { "Host": "::ffff:8.8.8.8", "Port": 3306, "Database": "app" } }),
            None,
        ),
        (
            "6to4-to-a-public-address",
            "SQLSERVER",
            json!({ "SqlServerParameters": { "Host": "2002:808:808::1", "Port": 1433, "Database": "app" } }),
            None,
        ),
        (
            "target-that-is-not-an-address",
            "REDSHIFT",
            json!({ "RedshiftParameters": { "ClusterId": "warehouse-1", "Port": 5439, "Database": "dev" } }),
            None,
        ),
        (
            "parameters-naming-no-address",
            "S3",
            json!({ "S3Parameters": { "ManifestFileLocation": { "Bucket": "b", "Key": "k" } } }),
            None,
        ),
        (
            "private-address-through-a-vpc-connection",
            "MYSQL",
            json!({ "MySqlParameters": { "Host": "10.0.0.7", "Port": 3306, "Database": "app" } }),
            Some(json!({
                "VpcConnectionArn":
                    format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:vpcConnection/vpc-conn-1"),
            })),
        ),
    ];

    for (case, kind, parameters, vpc_connection_properties) in rows {
        let data_source_id = format!("ds-ok-{case}");
        let mut request = json!({
            "DataSourceId": data_source_id,
            "Name": data_source_id,
            "Type": kind,
            "DataSourceParameters": parameters,
        });
        if let Some(properties) = vpc_connection_properties {
            request["VpcConnectionProperties"] = properties;
        }
        let (status, body) = create_data_source(&harness, ACCOUNT, request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a create the service can route expected 202, got {status} \
             with {body}"
        );

        let (status, body) = describe_data_source(&harness, ACCOUNT, &data_source_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created data source must read back, got {status} with \
             {body}"
        );
        assert_eq!(
            body["DataSource"]["Arn"],
            json!(data_source_arn(ACCOUNT, &data_source_id)),
            "case {case:?}: the read must report the created source: {body}"
        );
    }
}

#[tokio::test]
async fn an_update_naming_an_unroutable_address_is_refused_and_changes_nothing() {
    let harness = Harness::new();
    let data_source_id = "ds-rehosted";
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({
            "DataSourceId": data_source_id,
            "Name": "rehosted",
            "Type": "MYSQL",
            "DataSourceParameters": {
                "MySqlParameters": { "Host": "db.example.com", "Port": 3306, "Database": "app" }
            },
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");
    harness.settle_data_source(ACCOUNT, data_source_id);

    // An update re-states the target, so it is validated the same way a create is: the
    // rows are the same classes of address, each submitted to the update.
    for (case, host, parameters) in [
        (
            "private",
            "10.0.0.9",
            json!({ "MySqlParameters": { "Host": "10.0.0.9", "Port": 3306, "Database": "app" } }),
        ),
        (
            "loopback",
            "127.0.0.1",
            json!({ "MySqlParameters": { "Host": "127.0.0.1", "Port": 3306, "Database": "app" } }),
        ),
        (
            "ipv6-unique-local",
            "fd00::9",
            json!({ "MySqlParameters": { "Host": "fd00::9", "Port": 3306, "Database": "app" } }),
        ),
    ] {
        let (status, body) = update_data_source(
            &harness,
            ACCOUNT,
            data_source_id,
            json!({ "Name": "rehosted", "DataSourceParameters": parameters }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an update naming an address with no route expected 400, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidParameterValueException"),
            "case {case:?}: the refused parameter value is what is reported: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(unroutable_message(host)),
            "case {case:?}: the refusal must name the address the request carried: {body}"
        );

        // The refused update left the stored target where it was.
        let (status, body) = describe_data_source(&harness, ACCOUNT, data_source_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the data source must still read back, got {status} with {body}"
        );
        assert_eq!(
            body["DataSource"]["DataSourceParameters"]["MySqlParameters"]["Host"],
            json!("db.example.com"),
            "case {case:?}: the refused update must not change the stored target: {body}"
        );
        assert_eq!(
            body["DataSource"]["Status"],
            json!(CREATION_SUCCESSFUL),
            "case {case:?}: the refused update must not move the status: {body}"
        );
    }
}

/// One row of the parameters-against-kind table: the kind the data source is of, the
/// parameter set the write submits, and — for a set belonging to another kind — the union
/// member it is submitted under together with the kind that member configures.
struct KindRow {
    /// What this row varies, named in every assertion it makes.
    label: &'static str,
    /// The `DataSourceType` of the data source the write addresses.
    kind: &'static str,
    /// The `DataSourceParameters` the write submits, or `None` for a write that submits
    /// none.
    parameters: Option<Value>,
    /// The union member and the kind it configures, for a row the service must refuse;
    /// `None` for a row it must accept.
    refused_as: Option<(&'static str, &'static str)>,
}

/// The rows: each pairs a `DataSourceType` with a `DataSourceParameters` member, varying
/// which member carries the parameters, which kind the write states, whether the two
/// agree, whether the kind's enum member is one word or several, and whether the member
/// belongs to a kind the enum names at all.
///
/// Source: smithy_query(query="shape", shape="DataSourceParameters") for the member names
/// and smithy_query(query="shape", shape="DataSourceType") for the kinds — `RdsParameters`
/// and `ImpalaParameters` are members of the union that the enum names no kind for.
fn kind_rows() -> Vec<KindRow> {
    vec![
        KindRow {
            label: "Athena parameters on a Presto source",
            kind: "PRESTO",
            parameters: Some(json!({ "AthenaParameters": { "WorkGroup": DEFAULT_WORK_GROUP } })),
            refused_as: Some(("AthenaParameters", "ATHENA")),
        },
        KindRow {
            label: "PostgreSQL parameters on a MySQL source",
            kind: "MYSQL",
            parameters: Some(json!({
                "PostgreSqlParameters": { "Host": "db.example.com", "Port": 5432, "Database": "sales" }
            })),
            refused_as: Some(("PostgreSqlParameters", "POSTGRESQL")),
        },
        KindRow {
            label: "S3 parameters on an Athena source",
            kind: "ATHENA",
            parameters: Some(json!({
                "S3Parameters": {
                    "ManifestFileLocation": { "Bucket": "bucket-1", "Key": "manifest.json" }
                }
            })),
            refused_as: Some(("S3Parameters", "S3")),
        },
        KindRow {
            label: "Snowflake parameters on a Redshift source",
            kind: "REDSHIFT",
            parameters: Some(json!({
                "SnowflakeParameters": {
                    "Host": "snowflake.example.com",
                    "Database": "app",
                    "Warehouse": "wh"
                }
            })),
            refused_as: Some(("SnowflakeParameters", "SNOWFLAKE")),
        },
        KindRow {
            // A near miss: the two kinds differ only in the engine, and the kind the
            // member configures is spelled with an underscore.
            label: "Aurora PostgreSQL parameters on an Aurora source",
            kind: "AURORA",
            parameters: Some(json!({
                "AuroraPostgreSqlParameters": {
                    "Host": "aurora.example.com",
                    "Port": 5432,
                    "Database": "app"
                }
            })),
            refused_as: Some(("AuroraPostgreSqlParameters", "AURORA_POSTGRESQL")),
        },
        KindRow {
            label: "S3 Tables parameters on an S3 source",
            kind: "S3",
            parameters: Some(json!({
                "S3TablesParameters": {
                    "TableBucketArn":
                        format!("arn:aws:s3tables:{REGION}:{ACCOUNT}:bucket/analytics")
                }
            })),
            refused_as: Some(("S3TablesParameters", "S3_TABLES")),
        },
        KindRow {
            label: "the parameters of the kind the source is of",
            kind: "ATHENA",
            parameters: Some(json!({ "AthenaParameters": { "WorkGroup": DEFAULT_WORK_GROUP } })),
            refused_as: None,
        },
        KindRow {
            label: "another agreeing pair, whose kind is spelled with an underscore",
            kind: "S3_TABLES",
            parameters: Some(json!({
                "S3TablesParameters": {
                    "TableBucketArn":
                        format!("arn:aws:s3tables:{REGION}:{ACCOUNT}:bucket/analytics")
                }
            })),
            refused_as: None,
        },
        KindRow {
            // An RDS instance is reached as the engine it runs, so the enum names no
            // kind this parameter set configures and there is nothing to disagree with.
            label: "a parameter set the kind enum names no member for",
            kind: "MYSQL",
            parameters: Some(json!({
                "RdsParameters": { "InstanceId": "rds-1", "Database": "app" }
            })),
            refused_as: None,
        },
        KindRow {
            label: "another parameter set the kind enum names no member for",
            kind: "PRESTO",
            parameters: Some(json!({
                "ImpalaParameters": {
                    "Host": "impala.example.com",
                    "Port": 21050,
                    "SqlEndpointPath": "/cliservice"
                }
            })),
            refused_as: None,
        },
        KindRow {
            label: "no parameters at all, so no kind to disagree with",
            kind: "SNOWFLAKE",
            parameters: None,
            refused_as: None,
        },
    ]
}

/// The refusal a write whose parameter set belongs to another kind carries: fixed text
/// around the union member the request supplied and the kind that member configures.
fn mismatched_message(member: &str, owning_kind: &str) -> String {
    format!("{member} should only be set for {owning_kind} source")
}

/// Every kind a row names is a member of the model's own `DataSourceType` enum, so no
/// row asserts a kind the service does not have.
fn assert_modelled_kind(label: &str, kind: &str) {
    assert!(
        qs_types::DATA_SOURCE_TYPE_VALUES.contains(&kind),
        "case {label:?}: {kind:?} is not a member of the model's DataSourceType enum"
    );
}

/// A create registers no data source whose parameter set belongs to another kind.
///
/// The rule: `DataSourceParameters` is a variant type whose member IS the kind of source
/// it configures, so a write submitting a parameter set of a kind other than the data
/// source's `DataSourceType` is refused with `InvalidParameterValueException` naming that
/// member and the kind it belongs to, and writes nothing; a set that agrees with the
/// kind, or that belongs to no kind the enum names, is accepted.
#[tokio::test]
async fn a_create_whose_parameters_belong_to_another_kind_is_refused_and_registers_nothing() {
    let harness = Harness::new();

    for (index, row) in kind_rows().into_iter().enumerate() {
        let KindRow {
            label,
            kind,
            parameters,
            refused_as,
        } = row;
        assert_modelled_kind(label, kind);
        if let Some((_, owning_kind)) = refused_as {
            assert_modelled_kind(label, owning_kind);
        }

        let id = format!("ds-kind-create-{index}");
        let mut request = json!({ "DataSourceId": id, "Name": id, "Type": kind });
        if let Some(parameters) = parameters.clone() {
            request["DataSourceParameters"] = parameters;
        }
        let before = held_data_sources(&harness);
        let (status, body) = create_data_source(&harness, ACCOUNT, request).await;

        let Some((member, owning_kind)) = refused_as else {
            assert_eq!(
                status, 202,
                "case {label:?}: a create whose parameters are of its own kind expected \
                 202, got {status} with {body}"
            );
            let (status, read) = describe_data_source(&harness, ACCOUNT, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the accepted create must read back, got {status} with {read}"
            );
            assert_eq!(
                read["DataSource"]["DataSourceParameters"],
                parameters.unwrap_or(Value::Null),
                "case {label:?}: the submitted parameters are stored verbatim: {read}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {label:?}: a create whose parameters belong to {owning_kind} expected \
             400, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidParameterValueException"),
            "case {label:?}: the refused parameter value is what is reported: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(mismatched_message(member, owning_kind)),
            "case {label:?}: the refusal must name the member supplied and the kind it \
             configures: {body}"
        );
        assert!(
            body.get("RequestId").is_none(),
            "case {label:?}: a refusal document reports no RequestId — the id of the call \
             travels in the x-amzn-RequestId header: {body}"
        );

        // The refused create registered nothing, so the id resolves to no data source.
        assert_eq!(
            held_data_sources(&harness),
            before,
            "case {label:?}: the refused create must write no record"
        );
        let (status, read) = describe_data_source(&harness, ACCOUNT, &id).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a data source the refused create did not register expected \
             404, got {status} with {read}"
        );
        assert_eq!(
            read["Message"],
            json!(format!(
                "DataSource {} is not found",
                data_source_arn(ACCOUNT, &id)
            )),
            "case {label:?}: the miss must name the data source by its ARN: {read}"
        );
    }
}

/// The same rule on the update, where the kind is the one the data source WAS created
/// with: `Type` is not an input there, so the parameter set is checked against the kind
/// the stored data source is of, and a refused update changes nothing.
#[tokio::test]
async fn an_update_whose_parameters_belong_to_another_kind_is_refused_and_changes_nothing() {
    let harness = Harness::new();

    for (index, row) in kind_rows().into_iter().enumerate() {
        let KindRow {
            label,
            kind,
            parameters,
            refused_as,
        } = row;
        let id = format!("ds-kind-update-{index}");
        // The fixture states the kind and no parameters at all, so what the update
        // observes is its parameter set against the kind the data source IS.
        let (status, body) = create_data_source(
            &harness,
            ACCOUNT,
            json!({ "DataSourceId": id, "Name": id, "Type": kind }),
        )
        .await;
        assert_eq!(status, 202, "case {label:?}: fixture create: {body}");
        harness.settle_data_source(ACCOUNT, &id);

        let mut request = json!({ "Name": id });
        if let Some(parameters) = parameters.clone() {
            request["DataSourceParameters"] = parameters;
        }
        let (status, body) = update_data_source(&harness, ACCOUNT, &id, request).await;

        let Some((member, owning_kind)) = refused_as else {
            assert_eq!(
                status, 202,
                "case {label:?}: an update whose parameters are of the data source's kind \
                 expected 202, got {status} with {body}"
            );
            harness.settle_data_source(ACCOUNT, &id);
            let (_, read) = describe_data_source(&harness, ACCOUNT, &id).await;
            assert_eq!(
                read["DataSource"]["DataSourceParameters"],
                parameters.unwrap_or(Value::Null),
                "case {label:?}: the accepted update stores what it carried: {read}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {label:?}: an update whose parameters belong to {owning_kind} expected \
             400, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidParameterValueException"),
            "case {label:?}: the refused parameter value is what is reported: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(mismatched_message(member, owning_kind)),
            "case {label:?}: the refusal must name the member supplied and the kind it \
             configures: {body}"
        );

        // The refused update left the data source as the create made it.
        let (status, read) = describe_data_source(&harness, ACCOUNT, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the data source must still read back, got {status} with {read}"
        );
        assert_eq!(
            read["DataSource"]["DataSourceParameters"],
            Value::Null,
            "case {label:?}: the refused update must not store its parameters: {read}"
        );
        assert_eq!(
            read["DataSource"]["Status"],
            json!(CREATION_SUCCESSFUL),
            "case {label:?}: the refused update must not move the status: {read}"
        );
    }
}

/// One parameter set the catalog pass below probes every kind with: the union member it
/// is submitted under, the single kind whose sources that member configures, and the set
/// itself.
struct ParameterProbe {
    member: &'static str,
    owning_kind: &'static str,
    parameters: Value,
}

/// The parameter sets the catalog pass probes with — one naming its target by a workgroup
/// and one by a host — so the pass varies the form of the set as well as the kind it is
/// submitted on.
/// Source: smithy_query(query="shape", shape="DataSourceParameters") for the member names
/// and smithy_query(query="shape", shape="DataSourceType") for the kind each configures.
fn parameter_probes() -> Vec<ParameterProbe> {
    vec![
        ParameterProbe {
            member: "AthenaParameters",
            owning_kind: "ATHENA",
            parameters: json!({ "AthenaParameters": { "WorkGroup": DEFAULT_WORK_GROUP } }),
        },
        ParameterProbe {
            member: "PostgreSqlParameters",
            owning_kind: "POSTGRESQL",
            parameters: json!({
                "PostgreSqlParameters": {
                    "Host": "db.example.com",
                    "Port": 5432,
                    "Database": "sales"
                }
            }),
        },
    ]
}

/// The same cross-check over the WHOLE catalog of kinds rather than a chosen few, and the
/// state consequence of a refusal.
///
/// The rule: a create whose `DataSourceParameters` member configures a kind other than
/// the requested `DataSourceType` is refused with `InvalidParameterValueException` (400)
/// naming that member and the kind it configures, and nothing is written — so the id it
/// named is absent from a read (404, naming it by its ARN) and still free for a later
/// create, while the set that agrees with the requested kind is accepted.
///
/// The kinds are the generated catalog of the model's own `DataSourceType` enum, so every
/// kind the service names is exercised, not just the ones a recording happened to use.
#[tokio::test]
async fn the_parameter_kind_cross_check_holds_for_every_modelled_kind() {
    let harness = Harness::new();
    assert!(
        qs_types::DATA_SOURCE_TYPE_VALUES.len() > 1,
        "the model's DataSourceType enum must name more than one kind, got {:?}",
        qs_types::DATA_SOURCE_TYPE_VALUES
    );

    for probe in parameter_probes() {
        assert_modelled_kind(probe.member, probe.owning_kind);

        for (index, kind) in qs_types::DATA_SOURCE_TYPE_VALUES.iter().enumerate() {
            let case = format!("{} on a {kind} source", probe.member);
            let id = format!("ds-catalog-{}-{index}", probe.owning_kind.to_lowercase());
            let before = held_data_sources(&harness);
            let (status, body, headers) = harness
                .call_reporting_headers(
                    REGION,
                    "POST",
                    &format!("/accounts/{ACCOUNT}/data-sources"),
                    Some(json!({
                        "DataSourceId": id,
                        "Name": id,
                        "Type": kind,
                        "DataSourceParameters": probe.parameters.clone(),
                    })),
                )
                .await;

            if *kind == probe.owning_kind {
                assert_eq!(
                    status, 202,
                    "case {case:?}: a create whose parameters configure the kind it asks \
                     for expected 202, got {status} with {body}"
                );
                let (status, read) = describe_data_source(&harness, ACCOUNT, &id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the accepted create must read back, got {status} with \
                     {read}"
                );
                assert_eq!(
                    read["DataSource"]["DataSourceParameters"],
                    probe.parameters,
                    "case {case:?}: the submitted parameters are stored verbatim: {read}"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case:?}: a create whose parameters configure {} expected 400, got \
                 {status} with {body}",
                probe.owning_kind
            );
            assert_eq!(
                headers.get("x-amzn-errortype").and_then(|v| v.to_str().ok()),
                Some("InvalidParameterValueException"),
                "case {case:?}: the refused class travels in x-amzn-errortype, got \
                 {headers:?}"
            );
            assert_eq!(
                body["__type"],
                json!("InvalidParameterValueException"),
                "case {case:?}: the refused parameter value is what is reported: {body}"
            );
            assert_eq!(
                body["Message"],
                json!(mismatched_message(probe.member, probe.owning_kind)),
                "case {case:?}: the refusal must name the member supplied and the kind it \
                 configures: {body}"
            );
            assert_eq!(
                held_data_sources(&harness),
                before,
                "case {case:?}: the refused create must write no record"
            );

            let (status, read) = describe_data_source(&harness, ACCOUNT, &id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: an id the refused create did not register expected 404, got \
                 {status} with {read}"
            );
            assert_eq!(
                read["Message"],
                json!(format!(
                    "DataSource {} is not found",
                    data_source_arn(ACCOUNT, &id)
                )),
                "case {case:?}: the miss must name the data source by its ARN: {read}"
            );

            // No phantom record was left behind, so the id is still free: the same id,
            // submitted with no parameter set to disagree with its kind, is registered
            // rather than reported as one that already exists.
            let (status, body) = create_data_source(
                &harness,
                ACCOUNT,
                json!({ "DataSourceId": id, "Name": id, "Type": kind }),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: the id the refused create named must still be free, got \
                 {status} with {body}"
            );
            let (status, read) = describe_data_source(&harness, ACCOUNT, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the create that followed the refusal must read back, got \
                 {status} with {read}"
            );
            assert_eq!(
                read["DataSource"]["Type"],
                json!(kind),
                "case {case:?}: the stored data source is of the kind the create asked \
                 for: {read}"
            );
        }
    }
}
