// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the RELATIONAL half of the IoT SiteWise query language, as
//! `ExecuteQuery` answers it over HTTP through the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! The rule these suites pin: a query statement is a relational expression over the
//! views this service exposes, so every statement of the language — a join across
//! views, a grouped or ungrouped aggregate, a computed selection, `DISTINCT`, a set
//! operation, a subquery or a `WITH` definition, ordered and paged — is COMPUTED
//! from the rows the store holds, and only a statement outside that language is
//! refused (with one of the operation's modelled errors).
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over:
//! which relations are paired and how, which aggregate reduces them, what the
//! grouping key is, which expression form the projection computes, and which set
//! operation combines two results. Nothing is asserted against a remembered value:
//! each row's expectation follows from the resources the fixture wrote, and the last
//! suite writes one more sample and shows the answers move with it.
//!
//! The HTTP harness (`call`, `router`, `create_model`, …) and the result readers
//! (`query`, `rows`, `column_names`) are the ones the asset and `ExecuteQuery` suites
//! already define; this suite reuses them rather than growing a second.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, create_model, property_id, router};
use crate::execute_query::{column_names, query, rows};
use crate::support::now_seconds;

/// The temperature readings the fixture writes on `Boiler1`, in the order it writes
/// them. Chosen so each aggregate below has a distinct exact value: the sum is 60,
/// the mean 20, the extremes 10 and 30, the population deviation `sqrt(200/3)` and
/// the sample deviation exactly 10.
const READINGS: [f64; 3] = [10.0, 20.0, 30.0];

/// The reading `the_answers_follow_the_store` appends.
const APPENDED: f64 = 40.0;

/// The one reading the fixture writes on `Boiler2`.
const OTHER_READING: f64 = 5.0;

/// The second the fixture's readings are placed from: a whole minute inside the
/// window `BatchPutAssetPropertyValue` accepts, so every reading falls in ONE
/// aggregation window of every resolution and the `precomputed_aggregates` suite can
/// name that window's row. The readings themselves are placed in the seconds AFTER
/// it, because an aggregation window is `(start, end]` and a reading exactly on a
/// boundary belongs to the window ending there. Resolved once per process, so two
/// calls of one case cannot straddle a minute boundary.
fn window_start() -> i64 {
    static ONCE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| (now_seconds() - 600) / 60 * 60)
}

/// The instant `seconds` names, as the text a `TIMESTAMP` column's `scalarValue`
/// carries: `YYYY-MM-DD HH:MM:SS.mmm` in UTC, built here from the runtime's own
/// ISO-8601 formatter so the expectation is computed rather than remembered.
fn timestamp_text(seconds: i64) -> String {
    let formatted = aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable instant");
    format!("{}.000", formatted.trim_end_matches('Z').replace('T', " "))
}

/// What the fixture created: the ids a statement names its rows by.
struct Fixture {
    boiler_model: String,
    boiler_one: String,
    temperature: String,
}

impl Fixture {
    /// The `WHERE` clause naming the ONE stream the fixture wrote [`READINGS`] to.
    ///
    /// A `propertyId` is the asset MODEL property's id, so every asset of one model
    /// carries the same one — which is why a statement about one asset's stream has
    /// to name the asset beside the property.
    fn stream(&self) -> String {
        format!(
            "WHERE asset_id = '{}' AND property_id = '{}'",
            self.boiler_one, self.temperature
        )
    }
}

/// Three assets of two models, with readings on two of the three temperature
/// streams, so every suite below has a store whose rows it can count.
///
/// * `Boiler1` and `Boiler2` are of one model and carry three properties each;
/// * `Pump1` is of another model and carries one, and NO readings at all, which is
///   the row an outer join has to keep;
/// * `Boiler1` carries [`READINGS`] on its temperature and one `Status` reading,
///   `Boiler2` one temperature reading.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let boiler_model = create_model(
        router,
        "Boiler",
        json!([
            {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Cycles", "dataType": "INTEGER", "type": {"measurement": {}}},
            {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let pump_model = create_model(
        router,
        "Pump",
        json!([{"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let boiler_one = create_asset(router, "Boiler1", &boiler_model).await;
    let boiler_two = create_asset(router, "Boiler2", &boiler_model).await;
    let _pump = create_asset(router, "Pump1", &pump_model).await;

    let (_, described) = call(router, "GET", &format!("/assets/{boiler_one}"), None).await;
    let temperature = property_id(&described["assetProperties"], "Temperature");
    let status = property_id(&described["assetProperties"], "Status");
    let (_, other) = call(router, "GET", &format!("/assets/{boiler_two}"), None).await;
    let other_temperature = property_id(&other["assetProperties"], "Temperature");

    for (at, value) in READINGS.iter().enumerate() {
        write_double(
            router,
            &boiler_one,
            &temperature,
            window_start() + 1 + at as i64,
            *value,
        )
        .await;
    }
    write_double(
        router,
        &boiler_two,
        &other_temperature,
        window_start() + 1,
        OTHER_READING,
    )
    .await;
    write_value(
        router,
        &boiler_one,
        &status,
        window_start() + 1,
        json!({"stringValue": "OK"}),
    )
    .await;

    Fixture {
        boiler_model,
        boiler_one,
        temperature,
    }
}

/// Append one `doubleValue` reading to an asset property.
async fn write_double(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    at: i64,
    value: f64,
) {
    write_value(router, asset_id, property_id, at, json!({"doubleValue": value})).await;
}

/// Append one reading, whatever `Variant` member it carries.
async fn write_value(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    at: i64,
    value: Value,
) {
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": format!("e{at}-{property_id}"),
            "assetId": asset_id,
            "propertyId": property_id,
            "propertyValues": [{
                "value": value,
                "timestamp": {"timeInSeconds": at, "offsetInNanos": 0},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "seeding a reading: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "seeding a reading: every entry must be accepted, got {body}"
    );
}

/// Run `statement`, assert it was ANSWERED, and return its rows.
async fn answered(
    router: &MultiServiceRouter,
    case: &str,
    statement: &str,
) -> Vec<Vec<Option<String>>> {
    let (status, body) = query(router, statement).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {statement:?} must be answered, got {status} {body}"
    );
    rows(&body)
}

/// The single cell of a single-row, single-column result.
async fn one_cell(router: &MultiServiceRouter, case: &str, statement: &str) -> Option<String> {
    let rows = answered(router, case, statement).await;
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected one row, got {rows:?}"
    );
    rows.first()
        .and_then(|row| row.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected one column, got {rows:?}"))
}

/// The single cell of a single-row result, as a number.
async fn one_number(router: &MultiServiceRouter, case: &str, statement: &str) -> f64 {
    let cell = one_cell(router, case, statement).await;
    cell.as_deref()
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a number, got {cell:?}"))
}

#[tokio::test]
async fn a_join_pairs_the_rows_of_the_relations_it_names() {
    let router = router();
    let held = fixture(&router).await;

    // The fixture's shape, which every expected count below is read off: 3 assets, 7
    // asset properties (3 + 3 + 1) and 5 stored readings (4 on Boiler1, 1 on
    // Boiler2), of which 3 streams carry a latest value.
    let cases: Vec<(&str, String, usize)> = vec![
        (
            "an inner join on the asset id",
            "SELECT a.asset_name, p.property_name FROM asset a \
             JOIN asset_property p ON a.asset_id = p.asset_id"
                .to_string(),
            7,
        ),
        (
            "the same join written INNER",
            "SELECT a.asset_name FROM asset a \
             INNER JOIN asset_property p ON a.asset_id = p.asset_id"
                .to_string(),
            7,
        ),
        (
            "a join narrowed by a WHERE over either side",
            "SELECT p.property_name FROM asset a \
             JOIN asset_property p ON a.asset_id = p.asset_id \
             WHERE a.asset_name = 'Boiler1'"
                .to_string(),
            3,
        ),
        (
            "two joins, the shape the query language's examples take",
            "SELECT a.asset_name, p.property_name, l.double_value \
             FROM latest_value_time_series l \
             JOIN asset_property p \
             ON l.property_id = p.property_id AND l.asset_id = p.asset_id \
             JOIN asset a ON p.asset_id = a.asset_id"
                .to_string(),
            3,
        ),
        (
            "those joins paired by the property alone, which every asset of one \
             model shares",
            "SELECT a.asset_name, p.property_name, l.double_value \
             FROM latest_value_time_series l \
             JOIN asset_property p ON l.property_id = p.property_id \
             JOIN asset a ON p.asset_id = a.asset_id"
                .to_string(),
            6,
        ),
        (
            "a left join keeps the asset with no readings",
            "SELECT a.asset_name, t.double_value FROM asset a \
             LEFT JOIN raw_time_series t ON a.asset_id = t.asset_id"
                .to_string(),
            6,
        ),
        (
            "a right join keeps it too, written the other way round",
            "SELECT a.asset_name, t.double_value FROM raw_time_series t \
             RIGHT JOIN asset a ON a.asset_id = t.asset_id"
                .to_string(),
            6,
        ),
        (
            "a join pairs every matching pair of rows",
            "SELECT p.property_name, t.event_timestamp FROM asset_property p \
             JOIN raw_time_series t ON p.property_id = t.property_id"
                .to_string(),
            10,
        ),
        (
            "a full outer join keeps the unmatched rows of both sides",
            "SELECT a.asset_name, t.quality FROM asset a \
             FULL OUTER JOIN raw_time_series t ON a.asset_name = t.quality"
                .to_string(),
            8,
        ),
        (
            "a cross join pairs every row with every row",
            "SELECT a.asset_id, p.property_id FROM asset a CROSS JOIN asset_property p"
                .to_string(),
            21,
        ),
        (
            "a comma-separated FROM is that same cross join",
            "SELECT a.asset_id, p.property_id FROM asset a, asset_property p".to_string(),
            21,
        ),
        (
            "a USING join pairs by the named column",
            "SELECT asset_name, property_name FROM asset JOIN asset_property USING (asset_id)"
                .to_string(),
            7,
        ),
        (
            "a NATURAL join pairs by the columns both sides carry",
            "SELECT asset_name, property_name FROM asset NATURAL JOIN asset_property"
                .to_string(),
            7,
        ),
        (
            "a self-join addresses each side by its own alias",
            "SELECT a.asset_name, b.asset_name FROM asset a \
             JOIN asset b ON a.asset_model_id = b.asset_model_id \
             WHERE a.asset_name < b.asset_name"
                .to_string(),
            1,
        ),
        (
            "a join whose ON clause matches nothing answers no rows",
            "SELECT a.asset_id FROM asset a \
             JOIN asset_property p ON a.asset_id = p.property_id"
                .to_string(),
            0,
        ),
        (
            "a join of a view to a subquery",
            format!(
                "SELECT a.asset_name, t.double_value FROM asset a \
                 JOIN (SELECT asset_id, double_value FROM raw_time_series {}) t \
                 ON a.asset_id = t.asset_id",
                held.stream()
            ),
            3,
        ),
    ];
    for (case, statement, expected) in &cases {
        let answered = answered(&router, case, statement).await;
        assert_eq!(
            answered.len(),
            *expected,
            "case {case:?}: expected {expected} row(s), got {answered:?}"
        );
    }

    // The VALUES a join pairs, not only how many rows it answers: each property of
    // one asset, beside that asset's name.
    let paired = answered(
        &router,
        "the values a join pairs",
        "SELECT a.asset_name, p.property_name FROM asset a \
         JOIN asset_property p ON a.asset_id = p.asset_id \
         WHERE a.asset_name = 'Boiler1' ORDER BY p.property_name",
    )
    .await;
    assert_eq!(
        paired,
        vec![
            vec![Some("Boiler1".to_string()), Some("Cycles".to_string())],
            vec![Some("Boiler1".to_string()), Some("Status".to_string())],
            vec![Some("Boiler1".to_string()), Some("Temperature".to_string())],
        ],
        "a join answers the paired rows of its two sides, got {paired:?}"
    );

    // An outer join NULLS the columns of the side a row did not match.
    let unmatched = answered(
        &router,
        "the null a left join leaves",
        "SELECT a.asset_name, t.double_value FROM asset a \
         LEFT JOIN raw_time_series t ON a.asset_id = t.asset_id \
         WHERE a.asset_name = 'Pump1'",
    )
    .await;
    assert_eq!(
        unmatched,
        vec![vec![Some("Pump1".to_string()), None]],
        "an unmatched left row keeps its own columns and nulls the other side, \
         got {unmatched:?}"
    );

    // A `USING` join MERGES the column it pairs by, so `*` reports it once: the
    // asset view's five columns plus the asset-property view's seven, less the one
    // they share.
    let (_, merged) = query(
        &router,
        "SELECT * FROM asset JOIN asset_property USING (asset_id)",
    )
    .await;
    assert_eq!(
        column_names(&merged).len(),
        5 + 7 - 1,
        "a USING join reports the column it pairs by once, got {:?}",
        column_names(&merged)
    );

    // A `USING` list that repeats a name pairs that column ONCE: the list is a set of
    // columns, so the result is the join the single mention asks for — and the width
    // of a row is a function of the two relations, never of how long the caller's
    // list is.
    let repeated = vec!["asset_id"; 13].join(", ");
    let (status, wide) = query(
        &router,
        &format!("SELECT * FROM asset JOIN asset_property USING ({repeated})"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a repeated USING name must be answered, got {status} {wide}"
    );
    assert_eq!(
        column_names(&wide).len(),
        5 + 7 - 1,
        "a repeated USING name merges one column, got {:?}",
        column_names(&wide)
    );
    assert_eq!(
        rows(&wide).len(),
        7,
        "a repeated USING name pairs the same rows the single mention does, got {:?}",
        rows(&wide)
    );

    // A qualified wildcard projects the columns of THAT side of the join.
    let (_, one_side) = query(
        &router,
        "SELECT p.* FROM asset a JOIN asset_property p ON a.asset_id = p.asset_id",
    )
    .await;
    assert_eq!(
        column_names(&one_side).len(),
        7,
        "`p.*` projects the columns of p alone, got {:?}",
        column_names(&one_side)
    );
}

#[tokio::test]
async fn an_aggregate_reduces_the_rows_of_its_group() {
    let router = router();
    let held = fixture(&router).await;

    // Every row reduces the SAME three readings, so each expectation is the function
    // the row names applied to [`READINGS`] and nothing else. The deviations are the
    // two SQL names for it: the sample one divides by one fewer than the population
    // one does.
    let sum: f64 = READINGS.iter().sum();
    let mean = sum / READINGS.len() as f64;
    let population: f64 = (READINGS.iter().map(|value| (value - mean).powi(2)).sum::<f64>()
        / READINGS.len() as f64)
        .sqrt();
    let sample: f64 = (READINGS.iter().map(|value| (value - mean).powi(2)).sum::<f64>()
        / (READINGS.len() - 1) as f64)
        .sqrt();
    let over_readings =
        |call: &str| format!("SELECT {call} FROM raw_time_series {}", held.stream());
    let cases: Vec<(&str, String, f64)> = vec![
        ("COUNT(*)", over_readings("COUNT(*)"), READINGS.len() as f64),
        (
            "COUNT of a column",
            over_readings("COUNT(double_value)"),
            READINGS.len() as f64,
        ),
        (
            "COUNT of a column every row leaves null",
            over_readings("COUNT(string_value)"),
            0.0,
        ),
        (
            "COUNT DISTINCT",
            over_readings("COUNT(DISTINCT quality)"),
            1.0,
        ),
        ("SUM", over_readings("SUM(double_value)"), sum),
        ("AVG", over_readings("AVG(double_value)"), mean),
        (
            "MIN",
            over_readings("MIN(double_value)"),
            READINGS.iter().copied().fold(f64::MAX, f64::min),
        ),
        (
            "MAX",
            over_readings("MAX(double_value)"),
            READINGS.iter().copied().fold(f64::MIN, f64::max),
        ),
        ("STDDEV", over_readings("STDDEV(double_value)"), sample),
        (
            "STDDEV_SAMP",
            over_readings("STDDEV_SAMP(double_value)"),
            sample,
        ),
        (
            "STDDEV_POP",
            over_readings("STDDEV_POP(double_value)"),
            population,
        ),
        (
            "an aggregate of an expression",
            over_readings("SUM(double_value * 2)"),
            sum * 2.0,
        ),
        (
            "an expression over an aggregate",
            over_readings("COUNT(*) + 1"),
            READINGS.len() as f64 + 1.0,
        ),
        // The rows of a whole view, and of a filter that keeps none of them: an
        // ungrouped aggregate answers ONE row either way.
        (
            "COUNT over every asset",
            "SELECT COUNT(*) FROM asset".to_string(),
            3.0,
        ),
        (
            "COUNT over a filter that keeps no row",
            "SELECT COUNT(*) FROM asset WHERE asset_name = 'no such asset'".to_string(),
            0.0,
        ),
        (
            "SUM over a filter that keeps no row",
            "SELECT COUNT(double_value) FROM raw_time_series WHERE quality = 'BAD'".to_string(),
            0.0,
        ),
        (
            "an aggregate over a join",
            "SELECT COUNT(*) FROM asset a JOIN asset_property p ON a.asset_id = p.asset_id"
                .to_string(),
            7.0,
        ),
    ];
    for (case, statement, expected) in &cases {
        let answered = one_number(&router, case, statement).await;
        assert!(
            (answered - expected).abs() < 1e-9,
            "case {case:?}: expected {expected}, got {answered}"
        );
    }

    // An aggregate over a column that is not a number reduces it as the column
    // compares — here the one quality every reading carries.
    let quality = one_cell(
        &router,
        "MIN over a STRING column",
        "SELECT MIN(quality) FROM raw_time_series",
    )
    .await;
    assert_eq!(
        quality,
        Some("GOOD".to_string()),
        "MIN over a STRING column answers its least value, got {quality:?}"
    );

    // A sum over no value at all is null, not zero: there is no value to report.
    let empty = one_cell(
        &router,
        "SUM over no rows",
        "SELECT SUM(double_value) FROM raw_time_series WHERE quality = 'BAD'",
    )
    .await;
    assert_eq!(
        empty, None,
        "an aggregate with no value to reduce answers null, got {empty:?}"
    );

    // The result's column reports the aggregate: the expression as the statement
    // wrote it, typed by what the function computes.
    let (_, counted) = query(&router, "SELECT COUNT(*) FROM asset").await;
    assert_eq!(
        column_names(&counted),
        vec!["COUNT(*)"],
        "an unnamed selection reports the expression it computes, got {counted}"
    );
    assert_eq!(
        counted["columns"][0]["type"]["scalarType"], "INT",
        "a count is a whole number, got {counted}"
    );
    let (_, named) = query(&router, "SELECT COUNT(*) AS assets FROM asset").await;
    assert_eq!(
        column_names(&named),
        vec!["assets"],
        "an alias names the computed column, got {named}"
    );
}

#[tokio::test]
async fn a_group_by_answers_one_row_per_key_and_having_filters_the_groups() {
    let router = router();
    let held = fixture(&router).await;

    // One row per distinct key, carrying that key and the aggregate over its rows:
    // two assets share one model and the third has its own.
    let grouped = answered(
        &router,
        "a group per asset model",
        "SELECT asset_model_id, COUNT(*) AS assets FROM asset \
         GROUP BY asset_model_id ORDER BY assets DESC",
    )
    .await;
    assert_eq!(
        grouped.len(),
        2,
        "one row per distinct key, got {grouped:?}"
    );
    assert_eq!(
        grouped.first().and_then(|row| row.first()).cloned(),
        Some(Some(held.boiler_model.clone())),
        "the busiest group is the model two assets share, got {grouped:?}"
    );
    assert_eq!(
        grouped
            .iter()
            .filter_map(|row| row.get(1).cloned().flatten())
            .collect::<Vec<String>>(),
        vec!["2".to_string(), "1".to_string()],
        "each group counts its own rows, got {grouped:?}"
    );

    let cases: Vec<(&str, String, usize)> = vec![
        (
            "a HAVING clause keeps the groups it admits",
            "SELECT asset_model_id FROM asset GROUP BY asset_model_id HAVING COUNT(*) > 1"
                .to_string(),
            1,
        ),
        (
            "a HAVING clause that admits no group",
            "SELECT asset_model_id FROM asset GROUP BY asset_model_id HAVING COUNT(*) > 2"
                .to_string(),
            0,
        ),
        (
            "a HAVING clause with no GROUP BY filters the one group",
            "SELECT COUNT(*) FROM asset HAVING COUNT(*) = 3".to_string(),
            1,
        ),
        (
            "a group per expression",
            "SELECT UPPER(asset_name), COUNT(*) FROM asset GROUP BY UPPER(asset_name)"
                .to_string(),
            3,
        ),
        (
            "a group per pair of keys",
            "SELECT asset_model_id, asset_name, COUNT(*) FROM asset \
             GROUP BY asset_model_id, asset_name"
                .to_string(),
            3,
        ),
        (
            "a group over a view that holds no row for the key",
            "SELECT property_id, COUNT(*) FROM raw_time_series \
             WHERE quality = 'BAD' GROUP BY property_id"
                .to_string(),
            0,
        ),
        (
            "a group over a filtered view",
            format!(
                "SELECT property_id, COUNT(*) FROM raw_time_series {} \
                 GROUP BY property_id",
                held.stream()
            ),
            1,
        ),
    ];
    for (case, statement, expected) in &cases {
        let answered = answered(&router, case, statement).await;
        assert_eq!(
            answered.len(),
            *expected,
            "case {case:?}: expected {expected} group(s), got {answered:?}"
        );
    }

    // A grouped aggregate over a JOIN: every asset, with the readings it holds —
    // including the asset that holds none, which an outer join keeps and `COUNT` of
    // a column counts as none.
    let per_asset = answered(
        &router,
        "the readings each asset holds",
        "SELECT a.asset_name, COUNT(t.event_timestamp) AS readings FROM asset a \
         LEFT JOIN raw_time_series t ON a.asset_id = t.asset_id \
         GROUP BY a.asset_name ORDER BY a.asset_name",
    )
    .await;
    assert_eq!(
        per_asset,
        vec![
            vec![
                Some("Boiler1".to_string()),
                Some((READINGS.len() + 1).to_string()),
            ],
            vec![Some("Boiler2".to_string()), Some("1".to_string())],
            vec![Some("Pump1".to_string()), Some("0".to_string())],
        ],
        "each group counts the rows of its own asset, got {per_asset:?}"
    );
}

#[tokio::test]
async fn a_selected_expression_is_computed_from_the_row() {
    let router = router();
    let held = fixture(&router).await;
    let over_one_reading = |select: &str| {
        format!(
            "SELECT {select} FROM raw_time_series {} AND double_value = {} ",
            held.stream(),
            READINGS[0]
        )
    };

    // One row per expression FORM the language carries, each over the one reading
    // whose value the fixture fixed, so the expected cell is that value put through
    // the form.
    let first = READINGS[0];
    let cases: Vec<(&str, String, Option<String>)> = vec![
        (
            "a sum of a column and a literal",
            over_one_reading("double_value + 5"),
            Some((first + 5.0).to_string()),
        ),
        (
            "a product",
            over_one_reading("double_value * 2"),
            Some((first * 2.0).to_string()),
        ),
        (
            "a difference",
            over_one_reading("double_value - 0.5"),
            Some((first - 0.5).to_string()),
        ),
        (
            "a quotient",
            over_one_reading("double_value / 4"),
            Some((first / 4.0).to_string()),
        ),
        (
            "a remainder",
            over_one_reading("int_value % 2"),
            None,
        ),
        (
            "a negation",
            over_one_reading("-double_value"),
            Some((-first).to_string()),
        ),
        (
            "a comparison",
            over_one_reading("double_value > 15"),
            Some("false".to_string()),
        ),
        (
            "a CASE over a predicate",
            over_one_reading("CASE WHEN double_value > 15 THEN 'high' ELSE 'low' END"),
            Some("low".to_string()),
        ),
        (
            "a CASE over an operand",
            over_one_reading("CASE quality WHEN 'GOOD' THEN 'trusted' END"),
            Some("trusted".to_string()),
        ),
        (
            "COALESCE over a null column",
            over_one_reading("COALESCE(string_value, quality)"),
            Some("GOOD".to_string()),
        ),
        (
            "ABS of a negated column",
            over_one_reading("ABS(0 - double_value)"),
            Some(first.to_string()),
        ),
        (
            "a concatenation",
            "SELECT asset_name || '-1' FROM asset WHERE asset_name = 'Pump1'".to_string(),
            Some("Pump1-1".to_string()),
        ),
        (
            "LOWER",
            "SELECT LOWER(asset_name) FROM asset WHERE asset_name = 'Pump1'".to_string(),
            Some("pump1".to_string()),
        ),
        (
            "UPPER",
            "SELECT UPPER(asset_name) FROM asset WHERE asset_name = 'Pump1'".to_string(),
            Some("PUMP1".to_string()),
        ),
        (
            "LENGTH",
            "SELECT LENGTH(asset_name) FROM asset WHERE asset_name = 'Pump1'".to_string(),
            Some("5".to_string()),
        ),
        (
            "an expression over a null column answers null",
            over_one_reading("int_value + 1"),
            None,
        ),
        (
            "a literal",
            over_one_reading("'constant'"),
            Some("constant".to_string()),
        ),
    ];
    for (case, statement, expected) in &cases {
        let answered = one_cell(&router, case, statement).await;
        assert_eq!(
            &answered, expected,
            "case {case:?}: {statement:?} expected {expected:?}, got {answered:?}"
        );
    }

    // A computed column reports a type of its own: what the expression computes, not
    // what the column it reads declares.
    let (_, computed) = query(
        &router,
        "SELECT double_value / 4, asset_id || '', LENGTH(quality) FROM raw_time_series",
    )
    .await;
    let types: Vec<String> = computed["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("no columns in {computed}"))
        .iter()
        .map(|column| column["type"]["scalarType"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        types,
        vec![
            "DOUBLE".to_string(),
            "STRING".to_string(),
            "INT".to_string()
        ],
        "a computed column reports the type its expression answers, got {computed}"
    );
}

#[tokio::test]
async fn distinct_and_a_set_operation_combine_results_as_sets() {
    let router = router();
    let _held = fixture(&router).await;

    // Three assets of two models, so a duplicate is exactly what DISTINCT and a set
    // operation have to collapse.
    let cases: Vec<(&str, &str, usize)> = vec![
        (
            "DISTINCT collapses the duplicate keys",
            "SELECT DISTINCT asset_model_id FROM asset",
            2,
        ),
        (
            "DISTINCT over every column of the view keeps every row",
            "SELECT DISTINCT asset_id, asset_model_id FROM asset",
            3,
        ),
        (
            "a UNION answers the rows of both sides, once each",
            "SELECT asset_model_id FROM asset UNION SELECT asset_model_id FROM asset",
            2,
        ),
        (
            "a UNION ALL keeps every row of both sides",
            "SELECT asset_model_id FROM asset UNION ALL SELECT asset_model_id FROM asset",
            6,
        ),
        (
            "an INTERSECT answers the rows both sides hold",
            "SELECT asset_id FROM asset INTERSECT SELECT asset_id FROM asset_property",
            3,
        ),
        (
            "an INTERSECT of sides that share nothing",
            "SELECT asset_name FROM asset INTERSECT SELECT property_name FROM asset_property",
            0,
        ),
        (
            "an EXCEPT answers the left rows the right side does not hold",
            "SELECT asset_id FROM asset EXCEPT SELECT asset_id FROM raw_time_series",
            1,
        ),
        (
            "an EXCEPT of a side that holds them all",
            "SELECT asset_id FROM asset EXCEPT SELECT asset_id FROM asset_property",
            0,
        ),
        (
            "a set operation is ordered and paged by the query around it",
            "SELECT asset_name FROM asset UNION ALL SELECT asset_name FROM asset \
             ORDER BY asset_name LIMIT 2",
            2,
        ),
        (
            "a FETCH clause is a row limit",
            "SELECT asset_name FROM asset ORDER BY asset_name FETCH FIRST 2 ROWS ONLY",
            2,
        ),
        ("a TOP clause is a row limit", "SELECT TOP 1 asset_name FROM asset", 1),
    ];
    for (case, statement, expected) in &cases {
        let answered = answered(&router, case, statement).await;
        assert_eq!(
            answered.len(),
            *expected,
            "case {case:?}: expected {expected} row(s), got {answered:?}"
        );
    }

    // WHICH rows a set operation answers, not only how many: the assets that hold no
    // reading at all.
    let unread = answered(
        &router,
        "the assets no reading names",
        "SELECT asset_name FROM asset EXCEPT \
         SELECT a.asset_name FROM asset a JOIN raw_time_series t ON a.asset_id = t.asset_id",
    )
    .await;
    assert_eq!(
        unread,
        vec![vec![Some("Pump1".to_string())]],
        "an EXCEPT answers the left rows the right side does not hold, got {unread:?}"
    );
}

#[tokio::test]
async fn a_subquery_and_a_with_definition_are_relations_the_statement_reads() {
    let router = router();
    let held = fixture(&router).await;

    let cases: Vec<(&str, String, usize)> = vec![
        (
            "a WITH definition read by the statement",
            "WITH boilers AS (SELECT asset_id, asset_name FROM asset \
             WHERE asset_name LIKE 'Boiler%') \
             SELECT asset_name FROM boilers"
                .to_string(),
            2,
        ),
        (
            "a WITH definition read twice",
            "WITH boilers AS (SELECT asset_id FROM asset WHERE asset_name LIKE 'Boiler%') \
             SELECT one.asset_id FROM boilers one JOIN boilers two \
             ON one.asset_id = two.asset_id"
                .to_string(),
            2,
        ),
        (
            "a WITH definition aggregated over",
            "WITH boilers AS (SELECT asset_id FROM asset WHERE asset_name LIKE 'Boiler%') \
             SELECT COUNT(*) FROM boilers"
                .to_string(),
            1,
        ),
        (
            "a subquery read as a relation",
            "SELECT named.asset_name FROM \
             (SELECT asset_name FROM asset WHERE asset_name = 'Pump1') named"
                .to_string(),
            1,
        ),
        (
            "a subquery whose alias names its columns",
            "SELECT named.label FROM \
             (SELECT asset_name FROM asset) named (label) WHERE named.label = 'Pump1'"
                .to_string(),
            1,
        ),
        (
            "a column read in another case than the alias that named it",
            "SELECT named.ASSETNAME FROM \
             (SELECT asset_name AS AssetName FROM asset) named \
             WHERE named.assetname = 'Pump1'"
                .to_string(),
            1,
        ),
        (
            "an IN subquery",
            "SELECT asset_name FROM asset WHERE asset_id IN \
             (SELECT asset_id FROM raw_time_series)"
                .to_string(),
            2,
        ),
        (
            "a NOT IN subquery",
            "SELECT asset_name FROM asset WHERE asset_id NOT IN \
             (SELECT asset_id FROM raw_time_series)"
                .to_string(),
            1,
        ),
        (
            "an EXISTS subquery",
            format!(
                "SELECT asset_name FROM asset WHERE EXISTS \
                 (SELECT asset_id FROM raw_time_series {})",
                held.stream()
            ),
            3,
        ),
        (
            "a NOT EXISTS subquery over a relation that holds no row",
            "SELECT asset_name FROM asset WHERE NOT EXISTS \
             (SELECT asset_id FROM raw_time_series WHERE quality = 'BAD')"
                .to_string(),
            3,
        ),
        (
            "a scalar subquery compared against a column",
            "SELECT asset_name FROM asset \
             WHERE asset_name = (SELECT MIN(asset_name) FROM asset)"
                .to_string(),
            1,
        ),
    ];
    for (case, statement, expected) in &cases {
        let answered = answered(&router, case, statement).await;
        assert_eq!(
            answered.len(),
            *expected,
            "case {case:?}: expected {expected} row(s), got {answered:?}"
        );
    }

    // The VALUE a scalar subquery answers: the greatest reading of the stream, read
    // back through a statement that names no reading itself.
    let greatest = one_cell(
        &router,
        "the asset holding the greatest reading",
        &format!(
            "SELECT double_value FROM raw_time_series {stream} AND double_value = \
             (SELECT MAX(double_value) FROM raw_time_series {stream})",
            stream = held.stream()
        ),
    )
    .await;
    assert_eq!(
        greatest,
        Some(
            READINGS
                .iter()
                .copied()
                .fold(f64::MIN, f64::max)
                .to_string()
        ),
        "a scalar subquery answers the one value it computes, got {greatest:?}"
    );
}

#[tokio::test]
async fn an_order_by_may_name_an_expression_a_position_or_an_alias() {
    let router = router();
    let held = fixture(&router).await;
    let readings = format!("FROM raw_time_series {}", held.stream());

    // Each row orders the SAME three readings a different way, so the expected
    // sequence is [`READINGS`] sorted by the key the row names.
    let ascending: Vec<Option<String>> = {
        let mut sorted = READINGS.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted.iter().map(|value| Some(value.to_string())).collect()
    };
    let descending: Vec<Option<String>> = ascending.iter().rev().cloned().collect();
    let cases: Vec<(&str, String, Vec<Option<String>>)> = vec![
        (
            "a column of the view",
            format!("SELECT double_value {readings} ORDER BY double_value"),
            ascending.clone(),
        ),
        (
            "that column, descending",
            format!("SELECT double_value {readings} ORDER BY double_value DESC"),
            descending.clone(),
        ),
        (
            "an expression over it",
            format!("SELECT double_value {readings} ORDER BY 0 - double_value"),
            descending.clone(),
        ),
        (
            "the position of a result column",
            format!("SELECT double_value {readings} ORDER BY 1 DESC"),
            descending.clone(),
        ),
        (
            "the alias a projection gave it",
            format!("SELECT double_value AS reading {readings} ORDER BY reading"),
            ascending.clone(),
        ),
        (
            "a column the statement does not project",
            format!("SELECT double_value {readings} ORDER BY event_timestamp"),
            ascending.clone(),
        ),
        (
            "a computed column, by the expression it was written as",
            format!("SELECT double_value * 2 {readings} ORDER BY double_value * 2 DESC"),
            descending
                .iter()
                .map(|value| {
                    value
                        .as_deref()
                        .and_then(|text| text.parse::<f64>().ok())
                        .map(|number| (number * 2.0).to_string())
                })
                .collect(),
        ),
    ];
    for (case, statement, expected) in &cases {
        let answered = answered(&router, case, statement).await;
        let values: Vec<Option<String>> = answered
            .iter()
            .map(|row| row.first().cloned().flatten())
            .collect();
        assert_eq!(
            &values, expected,
            "case {case:?}: {statement:?} expected {expected:?}, got {values:?}"
        );
    }

    // An ORDER BY over an aggregate orders the GROUPS by what the aggregate computed.
    let busiest = answered(
        &router,
        "the groups ordered by their aggregate",
        "SELECT a.asset_name FROM asset a \
         LEFT JOIN raw_time_series t ON a.asset_id = t.asset_id \
         GROUP BY a.asset_name ORDER BY COUNT(t.event_timestamp) DESC",
    )
    .await;
    assert_eq!(
        busiest.first().and_then(|row| row.first()).cloned(),
        Some(Some("Boiler1".to_string())),
        "the group with the most rows comes first, got {busiest:?}"
    );
}

#[tokio::test]
async fn the_precomputed_aggregates_view_answers_the_aggregates_of_the_stored_readings() {
    let router = router();
    let held = fixture(&router).await;

    // Every resolution the view is computed at, with the LENGTH of the window it
    // names. Source: `crate::state::RESOLUTIONS`, itself read off the modelled
    // `Resolution` pattern (^1m|15m|1h|1d$). The fixture's readings fall in ONE
    // window of each, so each answers one row for the stream — stamped with the
    // instant that window ENDS, which is the instant the aggregate operations report
    // a window at.
    for (resolution, seconds) in [("1m", 60), ("15m", 900), ("1h", 3_600), ("1d", 86_400)] {
        let statement = format!(
            "SELECT count_value, sum_value, average_value, minimum_value, maximum_value, \
             event_timestamp \
             FROM precomputed_aggregates {} AND resolution = '{resolution}'",
            held.stream()
        );
        let answered = answered(&router, resolution, &statement).await;
        let sum: f64 = READINGS.iter().sum();
        // Windows are aligned to the epoch and are `(start, end]`, so the one the
        // readings fall in — they sit in the seconds after [`window_start`] — ends
        // at the first multiple of its own length after that minute.
        let ends = window_start() - window_start().rem_euclid(seconds) + seconds;
        assert_eq!(
            answered,
            vec![vec![
                Some((READINGS.len() as f64).to_string()),
                Some(sum.to_string()),
                Some((sum / READINGS.len() as f64).to_string()),
                Some(READINGS.iter().copied().fold(f64::MAX, f64::min).to_string()),
                Some(READINGS.iter().copied().fold(f64::MIN, f64::max).to_string()),
                Some(timestamp_text(ends)),
            ]],
            "case {resolution:?}: the window reduces the stored readings and is \
             stamped with its own end, got {answered:?}"
        );
    }

    // The view carries every column the catalog declares for it, so `*` reports them
    // all.
    let (_, every) = query(&router, "SELECT * FROM precomputed_aggregates").await;
    assert_eq!(
        column_names(&every),
        vec![
            "asset_id",
            "property_id",
            "property_alias",
            "event_timestamp",
            "resolution",
            "average_value",
            "count_value",
            "maximum_value",
            "minimum_value",
            "standard_deviation_value",
            "sum_value",
        ],
        "the view reports its whole column list, got {every}"
    );

    // The same window, read through the operation that reports aggregates: the view
    // is DERIVED from the stored readings by that same engine, so the two cannot
    // disagree — about the values, or about the instant the window is stamped with.
    let (status, reported) = call(
        &router,
        "GET",
        &format!(
            "/properties/aggregates?assetId={}&propertyId={}&resolution=1m\
             &aggregateTypes=SUM&aggregateTypes=COUNT&aggregateTypes=AVERAGE\
             &startDate={}&endDate={}",
            held.boiler_one,
            held.temperature,
            iso(window_start() - 60),
            iso(window_start() + 600),
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetAssetPropertyAggregates: got {status} {reported}"
    );
    let window = &reported["aggregatedValues"][0]["value"];
    let sum: f64 = READINGS.iter().sum();
    assert_eq!(
        (
            window["sum"].as_f64(),
            window["count"].as_f64(),
            window["average"].as_f64()
        ),
        (
            Some(sum),
            Some(READINGS.len() as f64),
            Some(sum / READINGS.len() as f64)
        ),
        "the aggregate operation reports the same window the view answers, \
         got {reported}"
    );
    // And stamps it with the same instant: the view reports what that operation
    // reports, in the text a `TIMESTAMP` cell carries.
    let stamped = one_cell(
        &router,
        "the instant the view stamps the window with",
        &format!(
            "SELECT event_timestamp FROM precomputed_aggregates {} AND resolution = '1m'",
            held.stream()
        ),
    )
    .await;
    assert_eq!(
        stamped.as_deref(),
        reported["aggregatedValues"][0]["timestamp"]
            .as_f64()
            .map(|seconds| timestamp_text(seconds as i64))
            .as_deref(),
        "the view and the aggregate operation stamp one window with one instant, \
         got {stamped:?} and {reported}"
    );
}

#[tokio::test]
async fn a_deeply_nested_statement_is_bounded_rather_than_unbounded() {
    // The rule: how deep a tree a statement can build is bounded BEFORE it is
    // parsed, so every walk of that tree — the one that plans it, the ones that
    // decide what it computes and what type it answers, and the one that evaluates
    // it — is bounded too. A statement inside the bound is answered; one past it is
    // refused with a modelled error rather than consuming the walk.
    let router = router();
    let held = fixture(&router).await;

    // Rows differ along WHERE the nesting is written and how deep it goes: a
    // projection and a predicate, each inside and past the bound.
    let nested = |depth: usize, select: &str, inner: &str| {
        format!(
            "SELECT {}{inner}{} FROM raw_time_series {} {select}",
            "(".repeat(depth),
            ")".repeat(depth),
            held.stream()
        )
    };
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "a shallow nest in the projection",
            nested(50, "", "double_value"),
            200,
        ),
        (
            "a deep nest in the projection",
            nested(400, "", "double_value"),
            400,
        ),
        (
            "a shallow nest around an aggregate",
            nested(50, "", "COUNT(*)"),
            200,
        ),
        (
            "a shallow nest in a predicate",
            format!(
                "SELECT double_value FROM raw_time_series {} AND {}double_value > 0{}",
                held.stream(),
                "(".repeat(50),
                ")".repeat(50)
            ),
            200,
        ),
        (
            "a deep nest in a predicate",
            format!(
                "SELECT double_value FROM raw_time_series {} AND {}double_value > 0{}",
                held.stream(),
                "(".repeat(400),
                ")".repeat(400)
            ),
            400,
        ),
    ];
    for (case, statement, expected) in &cases {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        if *expected == 400 {
            assert_eq!(
                crate::asset_lifecycle::error_code(&body),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_answers_follow_the_store() {
    let router = router();
    let held = fixture(&router).await;
    let counted = format!(
        "SELECT COUNT(*), SUM(double_value) FROM raw_time_series {}",
        held.stream()
    );
    let aggregated = format!(
        "SELECT count_value, sum_value FROM precomputed_aggregates {} \
         AND resolution = '1m'",
        held.stream()
    );
    let sum: f64 = READINGS.iter().sum();
    let before = answered(&router, "before the write", &counted).await;
    assert_eq!(
        before,
        vec![vec![
            Some(READINGS.len().to_string()),
            Some(sum.to_string())
        ]],
        "the aggregate reduces the readings the store holds, got {before:?}"
    );

    // One more reading, in the same window, through the ingestion operation.
    write_double(
        &router,
        &held.boiler_one,
        &held.temperature,
        window_start() + READINGS.len() as i64 + 1,
        APPENDED,
    )
    .await;

    let after = answered(&router, "after the write", &counted).await;
    assert_eq!(
        after,
        vec![vec![
            Some((READINGS.len() + 1).to_string()),
            Some((sum + APPENDED).to_string())
        ]],
        "an aggregate moves with the store, got {after:?}"
    );
    let window = answered(&router, "the window after the write", &aggregated).await;
    assert_eq!(
        window,
        vec![vec![
            Some(((READINGS.len() + 1) as f64).to_string()),
            Some((sum + APPENDED).to_string())
        ]],
        "the precomputed window moves with the store too, got {window:?}"
    );
}

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as.
fn iso(seconds: i64) -> String {
    aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable instant")
}
