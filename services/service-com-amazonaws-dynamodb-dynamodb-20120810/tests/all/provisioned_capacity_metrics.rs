// SPDX-License-Identifier: CC0-1.0

//! The capacity a table HOLDS, as CloudWatch carries it.
//!
//! The rule under test: a table whose capacity mode provisions capacity publishes the
//! `AWS/DynamoDB` metrics `ProvisionedReadCapacityUnits` and `ProvisionedWriteCapacityUnits`
//! for every resource that provisions some — the table itself under `{TableName}` alone, and
//! each global secondary index that carries its own throughput under
//! `{TableName, GlobalSecondaryIndexName}` — and the figures those series carry are the ones
//! in force on that resource, so a capacity change moves the series to the new figure and the
//! series never carries a figure the resource never held. A table in on-demand mode provisions
//! no capacity and so publishes neither metric at all, which is how the documented distinction
//! between the two billing modes is read off CloudWatch.
//!
//! The catalog spans what the rule generalises over: which billing mode the table was created
//! in, which halves of the throughput a change moves and in which direction, a change of
//! billing mode in either direction, and whether an index provisions capacity of its own
//! beside the table's — rather than one table at one capacity.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{describe_when_active, emulator_with_cloudwatch, ok, ok_cloudwatch};

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that reports how many read capacity units a resource has provisioned.
const PROVISIONED_READ: &str = "ProvisionedReadCapacityUnits";

/// The metric that reports how many write capacity units it has provisioned.
const PROVISIONED_WRITE: &str = "ProvisionedWriteCapacityUnits";

/// The dimension that names the table a series belongs to.
const TABLE_NAME: &str = "TableName";

/// The dimension that names the global secondary index a series belongs to.
const GLOBAL_SECONDARY_INDEX_NAME: &str = "GlobalSecondaryIndexName";

/// The global secondary index the row that declares one carries.
const INDEX: &str = "by-group";

/// How close two capacity figures must be to be the same figure: both sides are whole numbers
/// of capacity units, so any difference the rule is about is far wider than this.
const TOLERANCE: f64 = 1e-9;

/// A read/write pair of capacity units.
type Capacity = (i64, i64);

/// One row of the catalog: a table, how it was created, what was then done to it, and every
/// figure its series must carry afterwards.
struct Row {
    case: &'static str,
    /// The capacity the table is CREATED with; `None` creates it on demand, which provisions
    /// nothing.
    created: Option<Capacity>,
    /// The capacity a global secondary index of its own is created with, for a row that
    /// declares one.
    index: Option<Capacity>,
    /// What an `UpdateTable` moves the table to, for a row that makes one: `Some(None)`
    /// switches it to on-demand.
    updated: Option<Option<Capacity>>,
    /// Every figure the table's own `{TableName}` read series must carry, and no other.
    /// Empty for a table that has never provisioned capacity, whose series does not exist.
    read_values: &'static [i64],
    /// The same for its write series.
    write_values: &'static [i64],
}

/// The catalog.
const ROWS: &[Row] = &[
    Row {
        case: "a provisioned table, created and left alone",
        created: Some((1, 3)),
        index: None,
        updated: None,
        read_values: &[1],
        write_values: &[3],
    },
    Row {
        case: "a provisioned table whose halves are equal",
        created: Some((5, 5)),
        index: None,
        updated: None,
        read_values: &[5],
        write_values: &[5],
    },
    Row {
        case: "a write capacity increase",
        created: Some((1, 3)),
        index: None,
        updated: Some(Some((1, 4))),
        read_values: &[1],
        write_values: &[3, 4],
    },
    Row {
        case: "a read capacity decrease, the write half left where it was",
        created: Some((4, 4)),
        index: None,
        updated: Some(Some((2, 4))),
        read_values: &[2, 4],
        write_values: &[4],
    },
    Row {
        case: "both halves lowered at once",
        created: Some((10, 7)),
        index: None,
        updated: Some(Some((1, 1))),
        read_values: &[1, 10],
        write_values: &[1, 7],
    },
    Row {
        case: "an on-demand table, which provisions nothing",
        created: None,
        index: None,
        updated: None,
        read_values: &[],
        write_values: &[],
    },
    Row {
        case: "a switch from on-demand to provisioned",
        created: None,
        index: None,
        updated: Some(Some((2, 6))),
        read_values: &[2],
        write_values: &[6],
    },
    Row {
        case: "a switch from provisioned to on-demand",
        created: Some((3, 3)),
        index: None,
        updated: Some(None),
        read_values: &[3],
        write_values: &[3],
    },
    Row {
        case: "a table whose global secondary index provisions capacity of its own",
        created: Some((1, 2)),
        index: Some((3, 6)),
        updated: None,
        read_values: &[1],
        write_values: &[2],
    },
];

#[tokio::test]
async fn the_provisioned_series_of_each_table_carry_the_capacity_in_force_on_it() {
    let router = emulator_with_cloudwatch();
    // Every table is created first and then waited on, so the one creation span they share is
    // waited out once rather than once per row.
    for (index, row) in ROWS.iter().enumerate() {
        create(&router, row, &table_name(index)).await;
    }
    for (index, row) in ROWS.iter().enumerate() {
        describe_when_active(&router, row.case, &table_name(index)).await;
    }
    for (index, row) in ROWS.iter().enumerate() {
        if let Some(target) = row.updated {
            update(&router, &table_name(index), target).await;
        }
    }

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let table = table_name(index);

        // WHICH series exist: the table's own while it provisions capacity, one per index
        // that provisions its own, and none at all for a table that never has.
        let mut expected: Vec<Vec<(String, String)>> = Vec::new();
        if !row.read_values.is_empty() {
            expected.push(dimension_pairs(&table, None));
        }
        if row.index.is_some() {
            expected.push(dimension_pairs(&table, Some(INDEX)));
        }
        expected.sort();
        for metric in [PROVISIONED_READ, PROVISIONED_WRITE] {
            let listed = listed_dimension_sets(&router, metric, &table).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected ListMetrics({NAMESPACE}, {metric}, \
                 {TABLE_NAME}={table:?}) to report the dimension sets {expected:?}, got {listed:?}"
            );
        }

        // WHAT they carry: the figures the resource has held, and no other.
        for (metric, values) in [
            (PROVISIONED_READ, row.read_values),
            (PROVISIONED_WRITE, row.write_values),
        ] {
            let carried = carried_values(&router, metric, &table, None).await;
            assert_carries(case, metric, &table, &carried, values);
        }

        // An index's capacity belongs to the index's own series and is no part of the table's,
        // which the figures above already pin: the table's series carries what the TABLE
        // provisions even though the index provisions more.
        if let Some((read, write)) = row.index {
            for (metric, units) in [(PROVISIONED_READ, read), (PROVISIONED_WRITE, write)] {
                let carried = carried_values(&router, metric, &table, Some(INDEX)).await;
                assert_carries(case, metric, &format!("{table}/{INDEX}"), &carried, &[units]);
            }
        }
    }
}

/// The issue's own shape, end to end: a provisioned table and an on-demand twin beside it in
/// one emulator, so the leg that must report nothing is not vacuous, and a capacity change
/// read back through the series the metric is published under.
#[tokio::test]
async fn an_update_moves_the_series_to_the_capacity_the_table_now_reports() {
    let router = emulator_with_cloudwatch();
    let case = "a provisioned table beside an on-demand twin";
    let provisioned = "pcm-provisioned";
    let on_demand = "pcm-on-demand";
    create(
        &router,
        &Row {
            case,
            created: Some((1, 3)),
            index: None,
            updated: None,
            read_values: &[],
            write_values: &[],
        },
        provisioned,
    )
    .await;
    create(
        &router,
        &Row {
            case,
            created: None,
            index: None,
            updated: None,
            read_values: &[],
            write_values: &[],
        },
        on_demand,
    )
    .await;
    describe_when_active(&router, case, provisioned).await;
    describe_when_active(&router, case, on_demand).await;

    let listed = listed_dimension_sets(&router, PROVISIONED_WRITE, provisioned).await;
    assert_eq!(
        listed,
        vec![dimension_pairs(provisioned, None)],
        "case {case:?}: expected the provisioned table's {PROVISIONED_WRITE} series to be \
         listed under its {TABLE_NAME} alone, got {listed:?}"
    );
    let twin = listed_dimension_sets(&router, PROVISIONED_WRITE, on_demand).await;
    assert!(
        twin.is_empty(),
        "case {case:?}: expected no {PROVISIONED_WRITE} dimension set for the on-demand twin, \
         got {twin:?}"
    );

    let described = update(&router, provisioned, Some((1, 4))).await;
    let reported = described["TableDescription"]["ProvisionedThroughput"]["WriteCapacityUnits"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!("case {case:?}: UpdateTable reported no write capacity in {described}")
        });
    let carried = carried_values(&router, PROVISIONED_WRITE, provisioned, None).await;
    let highest = carried
        .iter()
        .map(|(_, maximum)| *maximum)
        .fold(f64::MIN, f64::max);
    assert!(
        (highest - reported).abs() < TOLERANCE,
        "case {case:?}: expected the {PROVISIONED_WRITE} series of {provisioned:?} to reach the \
         {reported} units the table now reports, got {carried:?}"
    );
    // The twin still provisions nothing, whatever happened to the table beside it.
    let twin = carried_values(&router, PROVISIONED_WRITE, on_demand, None).await;
    assert!(
        twin.is_empty(),
        "case {case:?}: expected the on-demand twin to publish no {PROVISIONED_WRITE} \
         datapoint, got {twin:?}"
    );
}

/// The table name of the row at `index`.
fn table_name(index: usize) -> String {
    format!("pcm-{index:02}")
}

/// Hold one series to the figures it must carry: every datapoint carries one of them, and the
/// lowest and highest are the lowest and highest of them — so a series that never moved to a
/// new figure, or that carries a figure the resource never held, fails. An empty expectation
/// is a series that must not exist at all.
fn assert_carries(
    case: &str,
    metric: &str,
    described: &str,
    carried: &[(f64, f64)],
    values: &[i64],
) {
    if values.is_empty() {
        assert!(
            carried.is_empty(),
            "case {case:?}: expected {metric} of {described:?} to report no datapoint, got \
             {carried:?}"
        );
        return;
    }
    assert!(
        !carried.is_empty(),
        "case {case:?}: expected {metric} of {described:?} to carry {values:?}, got no datapoint"
    );
    let admissible: Vec<f64> = values.iter().map(|units| *units as f64).collect();
    for (minimum, maximum) in carried {
        for (statistic, figure) in [("Minimum", minimum), ("Maximum", maximum)] {
            assert!(
                admissible
                    .iter()
                    .any(|value| (value - figure).abs() < TOLERANCE),
                "case {case:?}: expected every {statistic} of {metric} of {described:?} to be \
                 one of the {values:?} units it provisioned, got {figure}"
            );
        }
    }
    let lowest = carried
        .iter()
        .map(|(minimum, _)| *minimum)
        .fold(f64::MAX, f64::min);
    let highest = carried
        .iter()
        .map(|(_, maximum)| *maximum)
        .fold(f64::MIN, f64::max);
    let expected_lowest = admissible.iter().copied().fold(f64::MAX, f64::min);
    let expected_highest = admissible.iter().copied().fold(f64::MIN, f64::max);
    assert!(
        (lowest - expected_lowest).abs() < TOLERANCE,
        "case {case:?}: expected the lowest {metric} figure of {described:?} to be \
         {expected_lowest}, got {lowest} over {carried:?}"
    );
    assert!(
        (highest - expected_highest).abs() < TOLERANCE,
        "case {case:?}: expected the highest {metric} figure of {described:?} to be \
         {expected_highest}, got {highest} over {carried:?}"
    );
}

/// Create the table one row describes, in the billing mode and at the capacity it names.
async fn create(router: &MultiServiceRouter, row: &Row, name: &str) {
    let mut body = json!({
        "TableName": name,
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
    });
    match row.created {
        None => body["BillingMode"] = json!("PAY_PER_REQUEST"),
        Some((read, write)) => {
            body["BillingMode"] = json!("PROVISIONED");
            body["ProvisionedThroughput"] = throughput(read, write);
        }
    }
    if let Some((read, write)) = row.index {
        body["AttributeDefinitions"] = json!([
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "gk", "AttributeType": "S"},
        ]);
        body["GlobalSecondaryIndexes"] = json!([{
            "IndexName": INDEX,
            "KeySchema": [{"AttributeName": "gk", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "ALL"},
            "ProvisionedThroughput": throughput(read, write),
        }]);
    }
    ok(router, "CreateTable", body).await;
}

/// Move one table to `target` — a capacity pair, or on-demand — answering the description
/// `UpdateTable` reported.
async fn update(router: &MultiServiceRouter, name: &str, target: Option<Capacity>) -> Value {
    let body = match target {
        None => json!({"TableName": name, "BillingMode": "PAY_PER_REQUEST"}),
        Some((read, write)) => json!({
            "TableName": name,
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": throughput(read, write),
        }),
    };
    ok(router, "UpdateTable", body).await
}

/// One `ProvisionedThroughput` member.
fn throughput(read: i64, write: i64) -> Value {
    json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write})
}

/// The `(name, value)` pairs of the dimension set one series is published under, in name
/// order.
fn dimension_pairs(table: &str, index: Option<&str>) -> Vec<(String, String)> {
    let mut pairs = vec![(TABLE_NAME.to_string(), table.to_string())];
    if let Some(index) = index {
        pairs.push((
            GLOBAL_SECONDARY_INDEX_NAME.to_string(),
            index.to_string(),
        ));
    }
    pairs.sort();
    pairs
}

/// Every dimension set CloudWatch lists for `metric` under a `TableName` filter naming
/// `table`, each as its `(name, value)` pairs in name order, the sets sorted.
async fn listed_dimension_sets(
    router: &MultiServiceRouter,
    metric: &str,
    table: &str,
) -> Vec<Vec<(String, String)>> {
    let answered = ok_cloudwatch(
        router,
        "ListMetrics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": TABLE_NAME, "Value": table}],
        }),
    )
    .await;
    let mut sets: Vec<Vec<(String, String)>> = answered["Metrics"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|metric| {
            let mut pairs: Vec<(String, String)> = metric["Dimensions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|dimension| {
                    (
                        dimension["Name"].as_str().unwrap_or_default().to_string(),
                        dimension["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            pairs.sort();
            pairs
        })
        .collect();
    sets.sort();
    sets
}

/// The `(Minimum, Maximum)` of every datapoint one series holds, over a window wide enough for
/// everything this suite published.
async fn carried_values(
    router: &MultiServiceRouter,
    metric: &str,
    table: &str,
    index: Option<&str>,
) -> Vec<(f64, f64)> {
    let dimensions: Vec<Value> = dimension_pairs(table, index)
        .into_iter()
        .map(|(name, value)| json!({"Name": name, "Value": value}))
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let answered = ok_cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": now - 3600.0,
            "EndTime": now + 3600.0,
            "Period": 60,
            "Statistics": ["Minimum", "Maximum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|point| {
            (
                point["Minimum"].as_f64().unwrap_or_default(),
                point["Maximum"].as_f64().unwrap_or_default(),
            )
        })
        .collect()
}
