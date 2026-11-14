// SPDX-License-Identifier: CC0-1.0

//! The session's PARAMETERS and the workload manager's view of what ran:
//! `SET`, `RESET` and `SHOW`, the cluster parameter a reboot applies, and the
//! queue a labelled query is assigned to.
//!
//! The rules under test, each over a table of inputs:
//!
//! 1. `SHOW` reports the value in force for a parameter — the session's own
//!    where `SET` gave it one, the cluster's where its engine was given one, and
//!    the published default otherwise — and a parameter the server has no notion
//!    of is refused rather than kept;
//! 2. a change to a STATIC cluster parameter is not what the engine is running
//!    with until the cluster reboots, and a change to a dynamic one is;
//! 3. `SET query_group` labels the queries that follow and assigns them to the
//!    WLM queue whose configuration names that label, both of which the
//!    cluster's own system tables report.
//!
//! The harness is the one `warehouse_data_plane` owns, so these suites drive the
//! same emulator every other suite of this pair does.

use serde_json::json;

use crate::warehouse_data_plane::{field_text, Harness, DATABASE, DB_USER};

/// `SHOW` reports the value in force, and `SET`/`RESET` are what move it.
///
/// The rows differ in the KIND of parameter — a boolean, a string, a number, the
/// label a query carries — and in how its value got there, so the rule is
/// asserted over the class of parameters rather than over one of them.
#[tokio::test]
async fn show_reports_the_value_in_force_and_set_is_what_moves_it() {
    let harness = Harness::new();
    let cluster = "session-parameters";
    harness.create_cluster(cluster).await;

    /// One parameter: the statement run before the read, and the value `SHOW`
    /// must report afterwards.
    struct Row {
        case: &'static str,
        parameter: &'static str,
        applied: Option<&'static str>,
        expected: &'static str,
    }

    let rows = [
        Row {
            case: "a boolean parameter nothing has set",
            parameter: "enable_case_sensitive_identifier",
            applied: None,
            expected: "false",
        },
        Row {
            case: "a string parameter nothing has set",
            parameter: "datestyle",
            applied: None,
            expected: "ISO, MDY",
        },
        Row {
            case: "a numeric parameter nothing has set",
            parameter: "wlm_query_slot_count",
            applied: None,
            expected: "1",
        },
        Row {
            case: "the label a query carries, before anything sets it",
            parameter: "query_group",
            applied: None,
            expected: "default",
        },
        Row {
            case: "a parameter this session set",
            parameter: "query_group",
            applied: Some("SET query_group TO 'reporting'"),
            expected: "reporting",
        },
        Row {
            case: "a parameter set with the equals form",
            parameter: "wlm_query_slot_count",
            applied: Some("SET wlm_query_slot_count = 3"),
            expected: "3",
        },
        Row {
            case: "a parameter set to a list of values",
            parameter: "datestyle",
            applied: Some("SET datestyle TO ISO, DMY"),
            expected: "ISO, DMY",
        },
        Row {
            case: "a parameter given its default back",
            parameter: "query_group",
            applied: Some("SET query_group TO 'reporting'; RESET query_group"),
            expected: "default",
        },
        Row {
            case: "a parameter set to DEFAULT, which resets it",
            parameter: "query_group",
            applied: Some("SET query_group TO 'reporting'; SET query_group TO DEFAULT"),
            expected: "default",
        },
        Row {
            case: "every parameter given its default back at once",
            parameter: "query_group",
            applied: Some("SET query_group TO 'reporting'; RESET ALL"),
            expected: "default",
        },
    ];

    for row in &rows {
        // One submission carries the statements that set the value and the read
        // of it, because a parameter belongs to the CONNECTION they share.
        let sql = match row.applied {
            Some(applied) => format!("{applied}; SHOW {}", row.parameter),
            None => format!("SHOW {}", row.parameter),
        };
        let reported = harness.one_value(row.case, cluster, &sql).await;
        assert_eq!(
            reported, row.expected,
            "case {:?}: SHOW {} must report the value in force",
            row.case, row.parameter
        );
    }

    // A parameter nothing publishes is not a parameter: the server refuses to
    // report it and refuses to set it.
    for (case, sql) in [
        ("a read of a parameter nothing publishes", "SHOW not_a_parameter"),
        (
            "a set of a parameter nothing publishes",
            "SET not_a_parameter TO 'x'",
        ),
    ] {
        let error = harness.run_failing(case, cluster, sql).await;
        assert_eq!(
            error, "ERROR: unrecognized configuration parameter \"not_a_parameter\"",
            "case {case:?}: an unknown parameter is refused in the server's own wording"
        );
    }

    // And a parameter the server reads only at start-up cannot be changed by a
    // session: its value comes from the cluster's parameter group and a reboot.
    let error = harness
        .run_failing(
            "a set of a parameter the server reads at start-up",
            cluster,
            "SET require_ssl TO true",
        )
        .await;
    assert_eq!(
        error,
        "ERROR: parameter \"require_ssl\" cannot be changed without restarting the server",
        "a start-up parameter is refused in the server's own wording"
    );

    // `SHOW ALL` reports the whole set, each parameter with the value in force.
    let listed = harness
        .query_rows("every parameter at once", cluster, "SHOW ALL")
        .await;
    let reported: Vec<(String, String)> = listed
        .iter()
        .map(|row| {
            (
                field_text(row.first().unwrap_or(&json!(null))),
                field_text(row.get(1).unwrap_or(&json!(null))),
            )
        })
        .collect();
    assert!(
        reported
            .iter()
            .any(|(name, value)| name == "query_group" && value == "default")
            && reported.len() > 5,
        "SHOW ALL must report every parameter with the value in force, got {reported:?}"
    );
}

/// A change to a STATIC cluster parameter reaches the engine only when the
/// cluster reboots; a change to a dynamic one is in force without a restart.
///
/// This is the one rule two parameters of the same group make observable at
/// once, so both are changed together and read together: what differs between
/// the rows is only the parameter's own apply type.
#[tokio::test]
async fn a_static_parameter_change_reaches_the_engine_only_after_a_reboot() {
    let harness = Harness::new();
    let cluster = "parameter-reboot";
    let group = "reboot-group";
    harness.create_cluster(cluster).await;

    // A table in the cluster's database, so the reboot can be seen to leave the
    // cluster's data where it was. Its columns carry the STORAGE attributes a
    // Redshift client writes — the compression of a column, the distribution of
    // the table, the key it is sorted on — none of which changes what the table
    // holds.
    harness
        .run_ok(
            "the cluster's own data",
            cluster,
            "CREATE TABLE kept (id INTEGER NOT NULL ENCODE az64 DISTKEY, name VARCHAR(16) \
             ENCODE lzo) DISTSTYLE KEY COMPOUND SORTKEY (id)",
        )
        .await;
    harness
        .run_ok(
            "the cluster's own data",
            cluster,
            "INSERT INTO kept (id, name) VALUES (7, 'seven')",
        )
        .await;

    harness
        .redshift_ok(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", group),
                ("ParameterGroupFamily", "redshift-1.0"),
                ("Description", "the group this cluster is configured with"),
            ],
        )
        .await;
    harness
        .redshift_ok(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", cluster),
                ("ClusterParameterGroupName", group),
            ],
        )
        .await;
    harness
        .await_cluster_available(cluster, "configured with its own parameter group")
        .await;

    // One static parameter and one dynamic one, changed in the same call.
    harness
        .redshift_ok(
            "ModifyClusterParameterGroup",
            &[
                ("ParameterGroupName", group),
                (
                    "Parameters.Parameter.1.ParameterName",
                    "enable_case_sensitive_identifier",
                ),
                ("Parameters.Parameter.1.ParameterValue", "true"),
                ("Parameters.Parameter.2.ParameterName", "extra_float_digits"),
                ("Parameters.Parameter.2.ParameterValue", "2"),
            ],
        )
        .await;

    for (case, parameter, expected) in [
        (
            "a static parameter the cluster has not restarted for",
            "enable_case_sensitive_identifier",
            "false",
        ),
        (
            "a dynamic parameter, which needs no restart",
            "extra_float_digits",
            "2",
        ),
    ] {
        let reported = harness
            .one_value(case, cluster, &format!("SHOW {parameter}"))
            .await;
        assert_eq!(
            reported, expected,
            "case {case:?}: SHOW {parameter} must report what the engine is running with"
        );
    }

    harness
        .redshift_ok("RebootCluster", &[("ClusterIdentifier", cluster)])
        .await;
    harness
        .await_cluster_available(cluster, "restarted")
        .await;

    for (case, parameter, expected) in [
        (
            "the static parameter, after the restart that applied it",
            "enable_case_sensitive_identifier",
            "true",
        ),
        (
            "the dynamic parameter, unchanged by the restart",
            "extra_float_digits",
            "2",
        ),
    ] {
        let reported = harness
            .one_value(case, cluster, &format!("SHOW {parameter}"))
            .await;
        assert_eq!(
            reported, expected,
            "case {case:?}: SHOW {parameter} must report what the engine is running with"
        );
    }

    // The restart applied a parameter; it did not take the cluster's data with
    // it, which is what a client reads back through the catalog and the data.
    let described = harness
        .data_ok(
            "DescribeTable",
            &json!({
                "ClusterIdentifier": cluster,
                "Database": DATABASE,
                "DbUser": DB_USER,
                "Table": "kept",
            }),
        )
        .await;
    assert_eq!(
        described["TableName"], "kept",
        "a restart leaves the cluster's tables where they were: {described}"
    );
    let kept = harness
        .one_value("the data a restart kept", cluster, "SELECT id FROM kept")
        .await;
    assert_eq!(kept, "7", "a restart leaves the cluster's rows where they were");
}

/// `SET query_group` labels the queries that follow it and assigns them to the
/// WLM queue whose configuration names that label — and the cluster's own system
/// tables report both.
///
/// The rows differ in the label a query carries: one the configuration assigns
/// to its own queue, one it matches by pattern, and one it matches not at all,
/// which goes to the default queue.
#[tokio::test]
async fn the_queue_a_labelled_query_runs_in_is_in_the_wlm_system_tables() {
    let harness = Harness::new();
    let cluster = "wlm-queues";
    let group = "wlm-group";
    harness.create_cluster(cluster).await;
    harness
        .redshift_ok(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", group),
                ("ParameterGroupFamily", "redshift-1.0"),
                ("Description", "a group with queues of its own"),
            ],
        )
        .await;
    // Three queues: one for the `etl` label, one for every label starting with
    // `report`, and the default queue every other query goes to.
    let configuration = json!([
        {"name": "etl", "query_group": ["etl"], "query_concurrency": 2},
        {
            "name": "reporting",
            "query_group": ["report%"],
            "query_group_wild_card": 1,
            "query_concurrency": 2,
        },
        {"name": "default", "query_concurrency": 5},
    ])
    .to_string();
    harness
        .redshift_ok(
            "ModifyClusterParameterGroup",
            &[
                ("ParameterGroupName", group),
                (
                    "Parameters.Parameter.1.ParameterName",
                    "wlm_json_configuration",
                ),
                ("Parameters.Parameter.1.ParameterValue", &configuration),
            ],
        )
        .await;
    harness
        .redshift_ok(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", cluster),
                ("ClusterParameterGroupName", group),
            ],
        )
        .await;
    harness
        .await_cluster_available(cluster, "configured with its own queues")
        .await;
    // The configuration is a static parameter, so the cluster has to restart
    // before its engine runs with those queues.
    harness
        .redshift_ok("RebootCluster", &[("ClusterIdentifier", cluster)])
        .await;
    harness
        .await_cluster_available(cluster, "restarted with its own queues")
        .await;

    /// One labelled query: the label the batch sets, the query it then runs, and
    /// the queue that label puts it in.
    struct Row {
        case: &'static str,
        label: &'static str,
        marker: &'static str,
        queue: &'static str,
        service_class: &'static str,
    }

    let rows = [
        Row {
            case: "a label the configuration names in a queue of its own",
            label: "etl",
            marker: "etl_marker",
            queue: "etl",
            service_class: "6",
        },
        Row {
            case: "a label the configuration matches by pattern",
            label: "reporting_daily",
            marker: "report_marker",
            queue: "reporting",
            service_class: "7",
        },
        Row {
            case: "a label no queue names, which runs in the default queue",
            label: "ad-hoc",
            marker: "adhoc_marker",
            queue: "default",
            service_class: "8",
        },
    ];

    for row in &rows {
        harness
            .run_ok(
                row.case,
                cluster,
                &format!(
                    "SET query_group TO '{}'; SELECT 1 AS {}",
                    row.label, row.marker
                ),
            )
            .await;
        let logged = harness
            .query_rows(
                row.case,
                cluster,
                &format!(
                    "SELECT q.label, w.service_class, c.name, w.final_state FROM stl_query q \
                     JOIN stl_wlm_query w ON w.query = q.query JOIN \
                     stv_wlm_service_class_config c ON c.service_class = w.service_class WHERE \
                     q.querytxt LIKE '%{}%' AND q.querytxt NOT LIKE '%stl_query%'",
                    row.marker
                ),
            )
            .await;
        let cells: Vec<String> = logged
            .first()
            .map(|cells| cells.iter().map(field_text).collect())
            .unwrap_or_default();
        assert_eq!(
            cells,
            vec![
                row.label.to_string(),
                row.service_class.to_string(),
                row.queue.to_string(),
                "Completed".to_string(),
            ],
            "case {:?}: the query log and the WLM log must report the label, the queue it \
             assigned the query to and how it ended",
            row.case
        );
        // And the recent-queries view reports the same query, as a client
        // watching the cluster reads it.
        let recent = harness
            .one_value(
                row.case,
                cluster,
                &format!(
                    "SELECT count(*) FROM stv_recents WHERE query LIKE 'SELECT 1 AS {}%' AND \
                     status = 'Done'",
                    row.marker
                ),
            )
            .await;
        assert_eq!(
            recent, "1",
            "case {:?}: the query must be one of the cluster's recent queries",
            row.case
        );
    }
}
