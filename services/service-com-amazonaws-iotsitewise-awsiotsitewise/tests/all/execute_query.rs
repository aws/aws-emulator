// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ExecuteQuery`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rules these suites pin:
//!
//! 1. A `queryStatement` a SQL parser cannot read is refused with this operation's
//!    own modelled `InvalidRequestException` (400) — and so is a statement that
//!    parses but names a view or a column this service does not have, or is not a
//!    query at all.
//! 2. A statement that does parse is RUN against the shared store: its rows are the
//!    stored assets, asset properties and samples, so the answer changes exactly
//!    when the store does.
//!
//! The harness (`router`, `call`, `error_code`, …) is the one `asset_lifecycle`
//! already defines, re-used rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router,
};
use crate::support::now_seconds;

/// `POST /queries/execution` with `statement`.
pub(crate) async fn query(router: &MultiServiceRouter, statement: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        "/queries/execution",
        Some(json!({ "queryStatement": statement })),
    )
    .await
}

/// The column names a result reports, in order.
pub(crate) fn column_names(body: &Value) -> Vec<String> {
    body["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("no columns in {body}"))
        .iter()
        .map(|column| column["name"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The rows a result reports, each as the `scalarValue`s of its data points (an
/// absent value for a `nullValue` datum).
pub(crate) fn rows(body: &Value) -> Vec<Vec<Option<String>>> {
    body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(|row| {
            row["data"]
                .as_array()
                .unwrap_or_else(|| panic!("no data in {row}"))
                .iter()
                .map(|datum| {
                    datum["scalarValue"]
                        .as_str()
                        .map(ToString::to_string)
                })
                .collect()
        })
        .collect()
}

/// Two assets of one model, the first carrying two samples on its DOUBLE property.
/// Answers `(assetId, otherAssetId, temperature propertyId)`.
async fn asset_fixture(router: &MultiServiceRouter) -> (String, String, String) {
    let model_id = create_model(
        router,
        "Boiler",
        json!([
            {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(router, "Boiler1", &model_id).await;
    let other_id = create_asset(router, "Boiler2", &model_id).await;
    let (_, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    for (at, value) in [(240, 10.5), (180, 11.5)] {
        let (status, body) = call(
            router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("e{at}"),
                "assetId": asset_id,
                "propertyId": temperature,
                "propertyValues": [{
                    "value": {"doubleValue": value},
                    "timestamp": {"timeInSeconds": now_seconds() - at, "offsetInNanos": 0},
                }],
            }]})),
        )
        .await;
        assert_eq!(status, 200, "seeding a sample: got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "seeding a sample: every entry must be accepted, got {body}"
        );
    }
    (asset_id, other_id, temperature)
}

#[tokio::test]
async fn a_statement_a_sql_parser_cannot_read_is_a_modelled_refusal() {
    let router = router();
    let (asset_id, _other, _temperature) = asset_fixture(&router).await;

    // Rows span the ways a statement is not a query this service can run: syntax
    // errors of several shapes, a statement that is not a SELECT at all, more than
    // one statement, and names the catalog does not hold.
    let cases: Vec<(&str, String)> = vec![
        ("a misspelled keyword", "SELEC asset_name FROM asset".to_string()),
        ("no projection", "SELECT FROM asset".to_string()),
        ("no table", "SELECT * FROM".to_string()),
        ("a dangling WHERE", "SELECT * FROM asset WHERE".to_string()),
        (
            "a comparison with no right-hand side",
            "SELECT * FROM asset WHERE asset_name =".to_string(),
        ),
        ("unbalanced parentheses", "SELECT * FROM (asset".to_string()),
        ("an unterminated string", "SELECT * FROM asset WHERE asset_name = 'x".to_string()),
        ("whitespace only", "   ".to_string()),
        ("not a statement at all", "wind farm 3 temperature".to_string()),
        ("two statements", "SELECT * FROM asset; SELECT * FROM asset".to_string()),
        (
            "a statement that is not a query",
            format!("INSERT INTO asset (asset_id) VALUES ('{asset_id}')"),
        ),
        ("a view the catalog does not hold", "SELECT * FROM turbines".to_string()),
        (
            "a column the view does not hold",
            "SELECT asset_colour FROM asset".to_string(),
        ),
        (
            "a column the view does not hold, in the WHERE",
            "SELECT asset_id FROM asset WHERE asset_colour = 'red'".to_string(),
        ),
        (
            "a column the view does not hold, in the ORDER BY",
            "SELECT asset_id FROM asset ORDER BY asset_colour".to_string(),
        ),
    ];

    for (case, statement) in &cases {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {statement:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code for {statement:?} in {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }

    // The positive control: a syntactically valid statement against the same asset
    // succeeds, so the refusals above are about the statements and not about the
    // operation.
    let (status, body) = query(
        &router,
        &format!("SELECT asset_id, asset_name FROM asset WHERE asset_id = '{asset_id}'"),
    )
    .await;
    assert_eq!(status, 200, "the valid statement: got {status} {body}");
    assert_eq!(
        rows(&body),
        vec![vec![
            Some(asset_id.clone()),
            Some("Boiler1".to_string())
        ]],
        "the valid statement must answer the stored asset, got {body}"
    );
}

#[tokio::test]
async fn a_statement_is_answered_from_the_stored_resources() {
    let router = router();
    let (asset_id, other_id, temperature) = asset_fixture(&router).await;

    // --- the `asset` view -------------------------------------------------
    let (status, body) = query(&router, "SELECT * FROM asset").await;
    assert_eq!(status, 200, "SELECT *: got {status} {body}");
    assert_eq!(
        column_names(&body),
        vec![
            "asset_id",
            "asset_name",
            "asset_description",
            "asset_model_id",
            "asset_external_id"
        ],
        "`*` projects every column of the view, got {body}"
    );
    assert_eq!(
        rows(&body).len(),
        2,
        "both stored assets must be answered, got {body}"
    );
    // A column the view does hold, reported under the alias the statement gives it.
    let (status, body) = query(
        &router,
        "SELECT asset_name AS name FROM asset ORDER BY asset_name DESC",
    )
    .await;
    assert_eq!(status, 200, "ORDER BY DESC: got {status} {body}");
    assert_eq!(
        column_names(&body),
        vec!["name"],
        "an alias names the column in the result, got {body}"
    );
    assert_eq!(
        rows(&body),
        vec![
            vec![Some("Boiler2".to_string())],
            vec![Some("Boiler1".to_string())]
        ],
        "ORDER BY must order the rows, got {body}"
    );
    // A row the predicate keeps out is not answered, and a null column reads as a
    // `nullValue` datum.
    let (status, body) = query(
        &router,
        &format!("SELECT asset_description FROM asset WHERE asset_id = '{other_id}'"),
    )
    .await;
    assert_eq!(status, 200, "a null column: got {status} {body}");
    assert_eq!(
        rows(&body),
        vec![vec![None]],
        "a column the resource does not carry is null, got {body}"
    );
    assert_eq!(
        body["rows"][0]["data"][0]["nullValue"], true,
        "an absent value is reported as nullValue, got {body}"
    );

    // --- the `asset_property` view ---------------------------------------
    let (status, body) = query(
        &router,
        &format!(
            "SELECT property_name, data_type FROM asset_property \
             WHERE asset_id = '{asset_id}' ORDER BY property_name"
        ),
    )
    .await;
    assert_eq!(status, 200, "asset_property: got {status} {body}");
    assert_eq!(
        rows(&body),
        vec![
            vec![Some("Status".to_string()), Some("STRING".to_string())],
            vec![Some("Temperature".to_string()), Some("DOUBLE".to_string())],
        ],
        "every property of the asset must be answered, got {body}"
    );
    // The row count follows the store: both assets' properties, filtered by nothing.
    let (_, body) = query(&router, "SELECT property_id FROM asset_property").await;
    assert_eq!(
        rows(&body).len(),
        4,
        "two assets of a two-property model have four property rows, got {body}"
    );

    // --- the time-series views -------------------------------------------
    let (status, body) = query(
        &router,
        &format!(
            "SELECT double_value FROM raw_time_series \
             WHERE asset_id = '{asset_id}' AND property_id = '{temperature}' \
             ORDER BY event_timestamp"
        ),
    )
    .await;
    assert_eq!(status, 200, "raw_time_series: got {status} {body}");
    assert_eq!(
        rows(&body),
        vec![
            vec![Some("10.5".to_string())],
            vec![Some("11.5".to_string())]
        ],
        "every stored sample must be answered in timestamp order, got {body}"
    );
    let (status, body) = query(
        &router,
        &format!(
            "SELECT double_value, quality FROM latest_value_time_series \
             WHERE asset_id = '{asset_id}' AND property_id = '{temperature}'"
        ),
    )
    .await;
    assert_eq!(status, 200, "latest_value_time_series: got {status} {body}");
    assert_eq!(
        rows(&body),
        vec![vec![Some("11.5".to_string()), Some("GOOD".to_string())]],
        "the latest-value view answers the newest sample only, got {body}"
    );
    // The `event_timestamp` column reports the TIMESTAMP scalar type.
    let (_, body) = query(
        &router,
        "SELECT event_timestamp FROM latest_value_time_series",
    )
    .await;
    assert_eq!(
        body["columns"][0]["type"]["scalarType"], "TIMESTAMP",
        "event_timestamp is a TIMESTAMP column, got {body}"
    );

    // --- predicates spanning the operators ------------------------------
    // `Boiler2` carries no description, so the rows below also pin SQL's
    // three-valued logic over a nullable column: UNKNOWN keeps no row, negating it
    // leaves it unknown, and only `IS [NOT] NULL` answers a null.
    let predicates: Vec<(&str, &str, usize)> = vec![
        ("equality", "asset_name = 'Boiler1'", 1),
        ("inequality", "asset_name <> 'Boiler1'", 1),
        ("ordering", "asset_name >= 'Boiler1'", 2),
        ("a conjunction", "asset_name = 'Boiler1' AND asset_description IS NULL", 1),
        ("a disjunction", "asset_name = 'Boiler1' OR asset_name = 'Boiler2'", 2),
        ("a negation", "NOT asset_name = 'Boiler1'", 1),
        ("a list", "asset_name IN ('Boiler1', 'Boiler9')", 1),
        ("a pattern", "asset_name LIKE 'Boiler%'", 2),
        ("a one-character pattern", "asset_name LIKE 'Boiler_'", 2),
        ("a pattern that matches nothing", "asset_name LIKE 'Turbine%'", 0),
        ("a null test", "asset_description IS NOT NULL", 0),
        ("parentheses", "(asset_name = 'Boiler1')", 1),
        // Over the nullable column — neither asset carries a description — each of
        // these is UNKNOWN, so no row is kept whether the form is negated or not,
        // and an UNKNOWN side leaves a conjunction unknown while it makes a
        // disjunction follow its other side.
        ("a comparison with a null", "asset_description = 'anything'", 0),
        ("a negated comparison with a null", "NOT asset_description = 'anything'", 0),
        ("an inequality with a null", "asset_description <> 'anything'", 0),
        ("a negated pattern with a null", "asset_description NOT LIKE 'x%'", 0),
        ("a negated list with a null", "asset_description NOT IN ('x')", 0),
        (
            "an unknown side of a conjunction",
            "asset_description <> 'x' AND asset_name = 'Boiler1'",
            0,
        ),
        (
            "an unknown side of a disjunction",
            "asset_description = 'x' OR asset_name = 'Boiler2'",
            1,
        ),
        // And the row the null IS answered for: `IS [NOT] NULL` is never unknown.
        ("a null test on both rows", "asset_description IS NULL", 2),
    ];
    for (case, predicate, expected) in &predicates {
        let (status, body) = query(
            &router,
            &format!("SELECT asset_id FROM asset WHERE {predicate}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {predicate} got {status} {body}"
        );
        assert_eq!(
            rows(&body).len(),
            *expected,
            "case {case:?}: {predicate} must answer {expected} rows, got {body}"
        );
    }

    // --- LIMIT, OFFSET and the page token --------------------------------
    let (_, body) = query(&router, "SELECT asset_id FROM asset ORDER BY asset_name LIMIT 1").await;
    assert_eq!(
        rows(&body).len(),
        1,
        "LIMIT must bound the answer, got {body}"
    );
    let (_, body) = query(
        &router,
        "SELECT asset_name FROM asset ORDER BY asset_name LIMIT 1 OFFSET 1",
    )
    .await;
    assert_eq!(
        rows(&body),
        vec![vec![Some("Boiler2".to_string())]],
        "OFFSET must skip the rows before it, got {body}"
    );
    let (_, first) = call(
        &router,
        "POST",
        "/queries/execution",
        Some(json!({
            "queryStatement": "SELECT asset_name FROM asset ORDER BY asset_name",
            "maxResults": 1,
        })),
    )
    .await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a bounded page must carry a nextToken, got {first}"))
        .to_string();
    let (status, second) = call(
        &router,
        "POST",
        "/queries/execution",
        Some(json!({
            "queryStatement": "SELECT asset_name FROM asset ORDER BY asset_name",
            "maxResults": 1,
            "nextToken": token,
        })),
    )
    .await;
    assert_eq!(status, 200, "the second page: got {status} {second}");
    assert_eq!(
        [rows(&first), rows(&second)].concat(),
        vec![
            vec![Some("Boiler1".to_string())],
            vec![Some("Boiler2".to_string())]
        ],
        "the pages must walk every row exactly once, got {first} then {second}"
    );
    assert!(
        second["nextToken"].as_str().is_none(),
        "the last page carries no token, got {second}"
    );
    // A page token names a position IN the enumeration it was issued for, which here
    // is the rows the STATEMENT answers: replayed against another statement it names
    // no page, rather than silently serving that statement's result from the
    // position and reporting its leading rows in no page at all.
    for (case, statement) in [
        (
            "another statement over the same view",
            "SELECT asset_id FROM asset ORDER BY asset_name",
        ),
        (
            "another statement over another view",
            "SELECT asset_id, property_name FROM asset_property",
        ),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            "/queries/execution",
            Some(json!({
                "queryStatement": statement,
                "nextToken": token,
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token of another statement names no page, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // --- a qualifier that names no relation of the statement -------------
    for (case, statement) in [
        ("a qualified column", "SELECT other_view.asset_id FROM asset"),
        ("a qualified wildcard", "SELECT other_view.* FROM asset"),
        (
            "a qualifier in the WHERE",
            "SELECT asset_id FROM asset WHERE other_view.asset_name = 'Boiler1'",
        ),
    ] {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {statement:?} got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    // The view's own name and the alias the FROM clause gives it both qualify, in
    // the single-column form and the wildcard form alike.
    for (case, statement) in [
        ("the view's own name", "SELECT asset.asset_id FROM asset"),
        ("the view's own name, wildcard", "SELECT asset.* FROM asset"),
        ("the alias", "SELECT a.asset_id FROM asset a WHERE a.asset_name = 'Boiler1'"),
        ("the alias, wildcard", "SELECT a.* FROM asset a"),
    ] {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {statement:?} got {status} {body}"
        );
    }

    // --- a LIMIT or OFFSET that is not a row count ------------------------
    for (case, statement) in [
        ("a negative limit", "SELECT asset_id FROM asset LIMIT -1"),
        ("a fractional limit", "SELECT asset_id FROM asset LIMIT 1.5"),
        ("a column as the limit", "SELECT asset_id FROM asset LIMIT asset_name"),
        ("a negative offset", "SELECT asset_id FROM asset LIMIT 1 OFFSET -2"),
    ] {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {statement:?} got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // --- a comparison follows the COLUMN's declared type ------------------
    // The same rule the sort follows: a STRING column compares as text, so a name
    // that looks like a number is not quietly compared as one, while a numeric
    // column compares by number whichever way the literal is written.
    for (case, predicate, expected) in [
        ("a padded literal against a string column", "asset_name = '0Boiler1'", 0),
        ("a string column against its own value", "asset_name = 'Boiler1'", 1),
    ] {
        let (status, body) = query(
            &router,
            &format!("SELECT asset_id FROM asset WHERE {predicate}"),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            rows(&body).len(),
            expected,
            "case {case:?}: {predicate} must answer {expected} rows, got {body}"
        );
    }
    for (case, predicate, expected) in [
        ("a numeric column against a number", "double_value = 11.5", 1),
        ("a numeric column against a padded number", "double_value = 011.50", 1),
        ("a numeric column against another value", "double_value = 12", 0),
    ] {
        let (status, body) = query(
            &router,
            &format!("SELECT double_value FROM latest_value_time_series WHERE {predicate}"),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            rows(&body).len(),
            expected,
            "case {case:?}: {predicate} must answer {expected} rows, got {body}"
        );
    }

    // --- the order a sort imposes ----------------------------------------
    // A STRING column orders as TEXT, whatever its values look like, and the order
    // is total however odd they are: a name that parses as a float (`NaN`, `1e3`)
    // is still just a name. Asserted on its own store, so the row sets above are
    // untouched.
    let odd = crate::asset_lifecycle::router();
    let odd_model = create_model(
        &odd,
        "Boiler",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    for name in ["NaN", "10", "9", "inf", "Boiler"] {
        create_asset(&odd, name, &odd_model).await;
    }
    let (status, body) = query(&odd, "SELECT asset_name FROM asset ORDER BY asset_name").await;
    assert_eq!(status, 200, "ordering odd names: got {status} {body}");
    assert_eq!(
        rows(&body)
            .into_iter()
            .map(|row| row[0].clone().unwrap_or_default())
            .collect::<Vec<String>>(),
        vec!["10", "9", "Boiler", "NaN", "inf"],
        "a STRING column orders as text, got {body}"
    );
    // And the same statement twice answers the same order, so the comparison is a
    // total order and not one a sort may resolve differently per run.
    let (_, again) = query(&odd, "SELECT asset_name FROM asset ORDER BY asset_name").await;
    assert_eq!(
        rows(&body),
        rows(&again),
        "the order must be the same on every run"
    );

    // A read changes nothing a later caller sees: the same statement answers the
    // same rows.
    let (_, again) = query(&router, "SELECT * FROM asset").await;
    let (_, once_more) = query(&router, "SELECT * FROM asset").await;
    assert_eq!(
        rows(&again),
        rows(&once_more),
        "a query must not change what the next one sees"
    );
}

#[tokio::test]
async fn a_row_page_token_is_honoured_only_for_a_row_the_result_still_has() {
    // The rule: a page token names a position in the enumeration it was issued for,
    // and this operation issues one only for a row the result HAS, so an offset at
    // or past the end of the result it is presented against is a token it never
    // issued and is refused with the modelled `InvalidRequestException` (400) —
    // never answered with an empty page, which would report absence as success.
    let router = router();
    let model_id = create_model(
        &router,
        "Boiler",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    // Three assets, so a token can be minted at more than one position of the
    // enumeration the statement's rows form.
    let mut ids: Vec<(&str, String)> = Vec::new();
    for name in ["Boiler1", "Boiler2", "Boiler3"] {
        ids.push((name, create_asset(&router, name, &model_id).await));
    }
    let statement = "SELECT asset_name FROM asset ORDER BY asset_name";

    // Tokens this operation DID issue, minted at two positions of the result: the
    // rule is about which rows a position names, so it is exercised at more than
    // one position.
    let mut minted: Vec<(usize, String)> = Vec::new();
    for at in [1usize, 2] {
        let (status, body) = call(
            &router,
            "POST",
            "/queries/execution",
            Some(json!({ "queryStatement": statement, "maxResults": at })),
        )
        .await;
        assert_eq!(status, 200, "a page of {at} row(s): got {status} {body}");
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("a page of {at} of 3 row(s) must carry a nextToken, got {body}")
            })
            .to_string();
        minted.push((at, token));
    }

    // The control: while the result still HAS the row a token names, that token is
    // honoured and resumes the result exactly there.
    let (_, whole_body) = query(&router, statement).await;
    let whole = rows(&whole_body);
    assert_eq!(
        whole.len(),
        3,
        "the fixture must answer three rows to page over, got {whole_body}"
    );
    for (at, token) in &minted {
        let (status, body) = call(
            &router,
            "POST",
            "/queries/execution",
            Some(json!({ "queryStatement": statement, "nextToken": token })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {at}: its own token must resume the result, got {status} {body}"
        );
        assert_eq!(
            rows(&body),
            whole[*at..].to_vec(),
            "case {at}: the token must resume the result at row {at}, got {body}"
        );
    }

    // Now SHRINK the result, one row at a time off the end of its order, so each
    // token above comes to name a row the result no longer has: an offset at the
    // end, an offset past the end, and — once nothing is left — every offset at all.
    // Each row lists the offsets the shrunken result must refuse; the others are
    // still rows it has, so they must still be honoured.
    let cases: Vec<(&str, &str, Vec<usize>)> = vec![
        ("an offset at the end of the result", "Boiler3", vec![2]),
        ("an offset past the end of the result", "Boiler2", vec![1, 2]),
        ("every offset into a result of no rows", "Boiler1", vec![1, 2]),
    ];
    for (case, removed, refused) in &cases {
        let id = ids
            .iter()
            .find(|(name, _)| name == removed)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("case {case:?}: no fixture asset named {removed}"));
        let (status, body) = call(&router, "DELETE", &format!("/assets/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: removing {removed}: got {status} {body}"
        );
        let (status, held_body) = query(&router, statement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the shrunken result must still be answered, got {status} {held_body}"
        );
        let held = rows(&held_body);
        // A result holding no rows is still ANSWERED to a caller that presents no
        // token: one empty page that resumes nowhere.
        if held.is_empty() {
            assert!(
                held_body["nextToken"].is_null(),
                "case {case:?}: a result of no rows resumes nowhere, got {held_body}"
            );
        }
        for (at, token) in &minted {
            let (status, body) = call(
                &router,
                "POST",
                "/queries/execution",
                Some(json!({ "queryStatement": statement, "nextToken": token })),
            )
            .await;
            if refused.contains(at) {
                assert_eq!(
                    status,
                    400,
                    "case {case:?}: offset {at} of a result of {} row(s) is a token this \
                     operation never issued, got {status} {body}",
                    held.len()
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidRequestException",
                    "case {case:?}: offset {at}: unexpected error code in {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some("Invalid pagination token encountered"),
                    "case {case:?}: offset {at}: this operation's own fixed refusal text, \
                     carrying no part of the request; got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: offset {at} is still a row this result has, \
                     got {status} {body}"
                );
                assert_eq!(
                    rows(&body),
                    held[*at..].to_vec(),
                    "case {case:?}: offset {at} must resume the result there, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_statement_past_the_engine_budget_ends_in_a_modelled_error() {
    let nested_router = router();

    // --- the depth bound ------------------------------------------------
    // Rows nest the statement past each of the two depth ceilings this engine
    // states: the one counted off the statement before it is parsed at all, and the
    // one the evaluator counts as it walks the parsed statement. Both are refused
    // with a modelled 400 rather than consuming the stack.
    let (_, _other, _temperature) = asset_fixture(&nested_router).await;
    for (case, depth) in [
        ("past the parse ceiling", 150),
        ("far past the parse ceiling", 2_000),
    ] {
        let nested = format!(
            "SELECT asset_id FROM asset WHERE {}asset_name = 'Boiler1'{}",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let (status, body) = query(&nested_router, &nested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a statement nested {depth} deep got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    // A chain of prefix operators opens one level each, so a long chain is refused
    // by the same pre-parse bound.
    let chained = format!(
        "SELECT asset_id FROM asset WHERE {}asset_name = 'Boiler1'",
        "NOT ".repeat(200)
    );
    let (status, body) = query(&nested_router, &chained).await;
    assert_eq!(status, 400, "a long prefix chain: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a long prefix chain: unexpected error code in {body}"
    );

    // A statement nested well inside the ceilings still runs, so the bounds refuse
    // only what is past them — nesting and prefix chains alike.
    let shallow = format!(
        "SELECT asset_id FROM asset WHERE {}asset_name = 'Boiler1'{}",
        "(".repeat(8),
        ")".repeat(8)
    );
    let (status, body) = query(&nested_router, &shallow).await;
    assert_eq!(status, 200, "a shallow nest: got {status} {body}");
    assert_eq!(rows(&body).len(), 1, "a shallow nest must run, got {body}");
    let short_chain = format!(
        "SELECT asset_id FROM asset WHERE {}asset_name = 'Boiler1'",
        "NOT ".repeat(10)
    );
    let (status, body) = query(&nested_router, &short_chain).await;
    assert_eq!(status, 200, "a short prefix chain: got {status} {body}");
    assert_eq!(
        rows(&body).len(),
        1,
        "ten negations leave the predicate as it was, got {body}"
    );
    // A statement that is WIDE rather than deep is not nested: a hundred separately
    // bracketed predicates close each bracket before the next opens, so the
    // pre-parse bound does not refuse them and the walk stays inside its own.
    let wide_predicate = (0..100)
        .map(|at| format!("(asset_name <> 'absent-{at}')"))
        .collect::<Vec<String>>()
        .join(" AND ");
    let (status, body) = query(
        &nested_router,
        &format!("SELECT asset_id FROM asset WHERE {wide_predicate}"),
    )
    .await;
    assert_eq!(status, 200, "a wide predicate: got {status} {body}");
    assert_eq!(
        rows(&body).len(),
        2,
        "a wide predicate must run and keep every row, got {body}"
    );
    // A chain long enough that the WALK over it would pass its own ceiling is
    // refused before it is even parsed — a left-associative chain needs no bracket
    // and no keyword, and the tree it builds is as deep as the chain is long, so
    // the operator count is what bounds it.
    let endless_predicate = (0..300)
        .map(|at| format!("asset_name <> 'absent-{at}'"))
        .collect::<Vec<String>>()
        .join(" AND ");
    let (status, body) = query(
        &nested_router,
        &format!("SELECT asset_id FROM asset WHERE {endless_predicate}"),
    )
    .await;
    assert_eq!(status, 400, "a chain past the walk's ceiling: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a chain past the walk's ceiling: unexpected error code in {body}"
    );
    // The same for a chain of arithmetic operators, which carries no keyword at all
    // and which the parser builds iteratively: thousands of levels of tree inside
    // the modelled statement length, refused on the operator count.
    let arithmetic = "1+".repeat(2_000);
    let (status, body) = query(
        &nested_router,
        &format!("SELECT asset_id FROM asset WHERE {arithmetic}1 = 1"),
    )
    .await;
    assert_eq!(status, 400, "an arithmetic chain: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "an arithmetic chain: unexpected error code in {body}"
    );
    // The same chain behind a COMMENT carrying an apostrophe: a comment is not
    // structure and what it contains must not be read as any, so the bound still
    // sees the chain.
    for (case, comment) in [
        ("a block comment", format!("/* ' */ {arithmetic}1 = 1")),
        (
            "a line comment",
            format!("-- it's a chain\n{arithmetic}1 = 1"),
        ),
    ] {
        let (status, body) = query(
            &nested_router,
            &format!("SELECT asset_id FROM asset WHERE {comment}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a chain behind a comment got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    // And a comment around a SHORT statement changes nothing: the skip removes the
    // comment, not the statement.
    for (case, statement) in [
        (
            "a block comment",
            "SELECT asset_id FROM asset /* ' a comment */ WHERE asset_name = 'Boiler1'",
        ),
        (
            "a line comment",
            "SELECT asset_id FROM asset -- it's a comment\nWHERE asset_name = 'Boiler1'",
        ),
    ] {
        let (status, body) = query(&nested_router, statement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {statement:?} got {status} {body}"
        );
        assert_eq!(
            rows(&body).len(),
            1,
            "case {case:?}: the commented statement must still run, got {body}"
        );
    }

    // The bound is taken off the statement's TOKENS, so no shape that only a lexer
    // can tell apart slips a chain past it: an apostrophe inside a DELIMITED
    // identifier is one token's content and not a string, and a cast chain's `::`
    // is an operator like any other. Each row carries a chain past the operator
    // bound and must be refused ON THAT BOUND, which the message names.
    for (case, statement) in [
        (
            "a quote inside a delimited identifier",
            format!(
                "SELECT asset_id FROM asset WHERE `it's` = 1 AND {}1 = 1",
                "1+".repeat(400)
            ),
        ),
        (
            "a cast chain",
            format!(
                "SELECT asset_id FROM asset WHERE 1{} = 1",
                "::INT".repeat(400)
            ),
        ),
    ] {
        let (status, body) = query(&nested_router, &statement).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("operators")),
            "case {case:?}: the operator bound must be the one that refused it, got {body}"
        );
    }

    // --- the per-statement list ceilings ---------------------------------
    // The projection and the ORDER BY both multiply the work done per row, so each
    // carries a ceiling; a statement at the ceiling still runs and one past it is
    // refused.
    let items = |count: usize| {
        (0..count)
            .map(|_| "asset_id".to_string())
            .collect::<Vec<String>>()
            .join(", ")
    };
    // The same for the two per-row multipliers a single token can carry: an `IN`
    // list, whose every element is evaluated against every surviving row, and a
    // wildcard, which stands for every column of the view — so the ceiling that
    // matters there is the EXPANDED width of the result row, not the number of
    // items the statement wrote. (`asset` has five columns, so thirteen wildcards
    // expand to sixty-five.)
    let values = |count: usize| {
        (0..count)
            .map(|at| format!("'absent-{at}'"))
            .collect::<Vec<String>>()
            .join(", ")
    };
    let wildcards = |count: usize| {
        (0..count)
            .map(|_| "*".to_string())
            .collect::<Vec<String>>()
            .join(", ")
    };
    for (case, statement, expected) in [
        (
            "a projection at the ceiling",
            format!("SELECT {} FROM asset", items(64)),
            200,
        ),
        (
            "a projection past the ceiling",
            format!("SELECT {} FROM asset", items(65)),
            400,
        ),
        (
            "an ORDER BY at the ceiling",
            format!("SELECT asset_id FROM asset ORDER BY {}", items(8)),
            200,
        ),
        (
            "an ORDER BY past the ceiling",
            format!("SELECT asset_id FROM asset ORDER BY {}", items(9)),
            400,
        ),
        (
            "an IN list at the ceiling",
            format!(
                "SELECT asset_id FROM asset WHERE asset_name IN ({})",
                values(64)
            ),
            200,
        ),
        (
            "an IN list past the ceiling",
            format!(
                "SELECT asset_id FROM asset WHERE asset_name IN ({})",
                values(65)
            ),
            400,
        ),
        (
            "wildcards expanding inside the column ceiling",
            format!("SELECT {} FROM asset", wildcards(12)),
            200,
        ),
        (
            "wildcards expanding past the column ceiling",
            format!("SELECT {} FROM asset", wildcards(13)),
            400,
        ),
    ] {
        let (status, body) = query(&nested_router, &statement).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {body}"
            );
        }
    }

    // --- the step budget ------------------------------------------------
    // A view wide enough that scanning it passes the row budget: one model of many
    // properties, instantiated by enough assets that the `asset_property` view has
    // more rows than a query may scan.
    let wide = router();
    let properties: Vec<Value> = (0..200)
        .map(|at| json!({"name": format!("P{at}"), "dataType": "DOUBLE",
                         "type": {"measurement": {}}}))
        .collect();
    let model_id = create_model(&wide, "Wide", Value::Array(properties)).await;
    for at in 0..101 {
        create_asset(&wide, &format!("Wide{at}"), &model_id).await;
    }
    let (status, body) = query(&wide, "SELECT property_id FROM asset_property").await;
    assert_eq!(
        status, 400,
        "a scan past the row budget: got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "QueryTimeoutException",
        "a scan past the row budget: unexpected error code in {body}"
    );
    // A narrower scan of the same store still answers, so the budget bounds the
    // work and does not disable the view.
    let (status, body) = query(&wide, "SELECT asset_id FROM asset").await;
    assert_eq!(status, 200, "a scan inside the budget: got {status} {body}");
    assert_eq!(
        rows(&body).len(),
        101,
        "a scan inside the budget answers every row, got {body}"
    );
}

#[tokio::test]
async fn a_construct_outside_the_query_language_is_refused_with_a_modelled_error() {
    let router = router();
    let (_asset_id, _other, _temperature) = asset_fixture(&router).await;

    // These statements carry clauses the IoT SiteWise query language does not carry,
    // so the REQUEST cannot be read — which is exactly what this operation models
    // `InvalidRequestException` for. An error outside the operation's modelled set is
    // one no generated client carries an exception class for, so a caller's
    // `except InvalidRequestException` would never fire.
    let cases: Vec<(&str, String)> = vec![
        (
            "a QUALIFY clause",
            "SELECT asset_id FROM asset QUALIFY asset_name = 'Boiler1'".to_string(),
        ),
        (
            "a SORT BY clause",
            "SELECT asset_id FROM asset SORT BY asset_name".to_string(),
        ),
        (
            "a CLUSTER BY clause",
            "SELECT asset_id FROM asset CLUSTER BY asset_name".to_string(),
        ),
        (
            "a DISTRIBUTE BY clause",
            "SELECT asset_id FROM asset DISTRIBUTE BY asset_name".to_string(),
        ),
        (
            "an ORDER BY ALL clause",
            "SELECT asset_id FROM asset ORDER BY ALL".to_string(),
        ),
        (
            "a GROUP BY ALL clause",
            "SELECT asset_id FROM asset GROUP BY ALL".to_string(),
        ),
        (
            "a recursive WITH clause",
            "WITH RECURSIVE held AS (SELECT asset_id FROM asset) \
             SELECT asset_id FROM held"
                .to_string(),
        ),
        (
            "a subquery read as a relation with no alias",
            "SELECT asset_id FROM (SELECT asset_id FROM asset)".to_string(),
        ),
        (
            "a function the language does not carry",
            "SELECT SQRT(asset_id) FROM asset".to_string(),
        ),
        (
            "a window function",
            "SELECT COUNT(*) OVER () FROM asset".to_string(),
        ),
        (
            "a column that is neither grouped nor aggregated",
            "SELECT asset_name, COUNT(*) FROM asset GROUP BY asset_model_id".to_string(),
        ),
        (
            "a column two joined relations both carry",
            "SELECT asset_id FROM asset a JOIN asset_property p ON a.asset_id = p.asset_id"
                .to_string(),
        ),
        (
            "more than one row limit",
            "SELECT asset_id FROM asset LIMIT 1 FETCH FIRST 1 ROW ONLY".to_string(),
        ),
        (
            "a relation name carrying a catalog or schema qualifier",
            "SELECT asset_id FROM some_catalog.asset".to_string(),
        ),
    ];
    for (case, statement) in &cases {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {statement:?} got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }
}

#[tokio::test]
async fn a_like_pattern_honours_the_escape_character_the_statement_names() {
    let router = router();
    // Three assets, one of whose names carries the `%` the pattern language uses as
    // its wildcard, so an escaped `%` and an unescaped one select different rows.
    let model_id = create_model(
        &router,
        "Boiler",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    for name in ["Boiler1", "Boiler2", "Boiler%x"] {
        create_asset(&router, name, &model_id).await;
    }

    // Rows: the same pattern with and without an ESCAPE, and the escape applied to
    // each of the two wildcards. The escaped symbol is a LITERAL, so it selects the
    // one asset whose name really carries it.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "an unescaped wildcard",
            "asset_name LIKE 'Boiler%'",
            vec!["Boiler%x", "Boiler1", "Boiler2"],
        ),
        (
            "an escaped percent",
            "asset_name LIKE 'Boiler!%%' ESCAPE '!'",
            vec!["Boiler%x"],
        ),
        (
            "an escaped percent with no wildcard after it",
            "asset_name LIKE 'Boiler!%x' ESCAPE '!'",
            vec!["Boiler%x"],
        ),
        (
            "an escaped underscore",
            "asset_name LIKE 'Boiler!_%' ESCAPE '!'",
            vec![],
        ),
        (
            "an unescaped underscore",
            "asset_name LIKE 'Boiler_' ESCAPE '!'",
            vec!["Boiler1", "Boiler2"],
        ),
    ];
    for (case, predicate, expected) in &cases {
        let (status, body) = query(
            &router,
            &format!("SELECT asset_name FROM asset WHERE {predicate} ORDER BY asset_name"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {predicate} got {status} {body}"
        );
        let names: Vec<String> = rows(&body)
            .into_iter()
            .map(|row| row[0].clone().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            expected.iter().map(ToString::to_string).collect::<Vec<String>>(),
            "case {case:?}: {predicate} must select exactly those rows, got {body}"
        );
    }
}

/// How many characters of comparison count as one unit of the query engine's work
/// budget. Source: `CHARS_PER_WORK_UNIT` in
/// `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/execute_query.rs`.
const CHARS_PER_WORK_UNIT: usize = 4_096;

/// Append one `stringValue` sample to an asset property.
async fn ingest_string(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    entry_id: &str,
    value: &str,
    age: i64,
) {
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": entry_id,
            "assetId": asset_id,
            "propertyId": property_id,
            "propertyValues": [{
                "value": {"stringValue": value},
                "timestamp": {"timeInSeconds": now_seconds() - age, "offsetInNanos": 0},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "seeding {entry_id}: got {status}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "seeding {entry_id}: every entry must be accepted, got {body}"
    );
}

#[tokio::test]
async fn the_work_a_cell_length_costs_is_charged_against_the_engine_budget() {
    // The rule: the work a statement causes is charged in CHARACTERS as well as in
    // rows, so work proportional to a stored cell's LENGTH — a `LIKE` match, a sort
    // key, a rebuilt row — is bounded by the engine's budget and a statement past it
    // ends in the modelled `QueryTimeoutException` (400), while one inside it is
    // answered. `stringValue` carries no modelled length bound, so a cell is the one
    // input whose size the model does not limit.
    let router = router();
    let model_id = create_model(
        &router,
        "Log",
        json!([{"name": "Message", "dataType": "STRING", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Logger1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let message = property_id(&asset["assetProperties"], "Message");

    // Two samples, each many work units long, differing in their first character so
    // an ordering over them is decided well inside the sort key the engine keeps.
    let cell_chars = 25 * CHARS_PER_WORK_UNIT;
    for (entry, filler, age) in [("e-a", 'a', 240), ("e-b", 'b', 180)] {
        let value: String = ::std::iter::repeat_n(filler, cell_chars).collect();
        ingest_string(&router, &asset_id, &message, entry, &value, age).await;
    }

    // Rows differ along the two lengths whose PRODUCT is what a `LIKE` costs: the
    // stored cell's length and the pattern's. Neither length alone decides — only
    // the pair whose product passes the budget is refused.
    let pattern = |symbols: usize| format!("%{}%", "a".repeat(symbols));
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "a short pattern over long cells",
            format!(
                "SELECT quality FROM raw_time_series WHERE string_value LIKE '{}'",
                pattern(4)
            ),
            200,
        ),
        (
            "a long pattern over short cells",
            format!(
                "SELECT asset_name FROM asset WHERE asset_name LIKE '{}'",
                pattern(2_000)
            ),
            200,
        ),
        (
            "a long pattern over long cells",
            format!(
                "SELECT quality FROM raw_time_series WHERE string_value LIKE '{}'",
                pattern(2_000)
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
                error_code(&body),
                "QueryTimeoutException",
                "case {case:?}: the work budget must be what refused it, got {body}"
            );
        } else {
            // A statement inside the budget is ANSWERED — with the rows it selects,
            // which for a pattern matching nothing is none of them.
            assert!(
                !column_names(&body).is_empty(),
                "case {case:?}: a statement inside the budget must answer a result, got {body}"
            );
        }
    }
    // The positive control for the refused pair: the same long cells answer the same
    // pattern as a plain comparison, so what the budget refused is the WORK and not
    // the statement.
    let (status, body) = query(
        &router,
        "SELECT quality FROM raw_time_series WHERE string_value LIKE 'a%'",
    )
    .await;
    assert_eq!(status, 200, "a cheap pattern over long cells: got {status}");
    assert_eq!(
        rows(&body).len(),
        1,
        "exactly the sample whose value starts with 'a' matches, got {body}"
    );

    // The budget bounds the work without changing the answer: an ordering over the
    // same long cells still runs and still orders them by their text, which is what
    // the bounded sort key has to keep.
    let (status, body) = query(
        &router,
        "SELECT string_value FROM raw_time_series ORDER BY string_value",
    )
    .await;
    assert_eq!(status, 200, "ordering by a long cell: got {status}");
    let firsts: Vec<Option<char>> = rows(&body)
        .into_iter()
        .map(|row| row[0].as_ref().and_then(|value| value.chars().next()))
        .collect();
    assert_eq!(
        firsts,
        vec![Some('a'), Some('b')],
        "ordering by a long cell must keep the rows in text order, got {firsts:?}"
    );

    // And a second read answers the same thing: charging work changes no state.
    let (status, again) = query(
        &router,
        "SELECT quality FROM raw_time_series ORDER BY event_timestamp",
    )
    .await;
    assert_eq!(status, 200, "a second read: got {status} {again}");
    assert_eq!(
        rows(&again).len(),
        2,
        "both samples must still be there, got {again}"
    );
}

#[tokio::test]
async fn a_timestamp_column_is_answered_as_a_formatted_utc_instant() {
    // The rule: a TIMESTAMP column's `Datum$scalarValue` is the INSTANT that cell
    // holds, written `YYYY-MM-DD HH:MM:SS.mmm` in UTC — the layout is fixed and
    // only the digits come from the instant — never the count of nanoseconds behind
    // it.
    let router = router();
    let model_id = create_model(
        &router,
        "Chiller",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Chiller1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");

    // Rows are instants that differ along what the layout has to render: the whole
    // second, a sub-second offset that rounds down to the millisecond, one that
    // lands on a whole millisecond, and a second on a minute boundary. Each is
    // written with its own value, so a row cannot borrow another's.
    let samples: [(&str, i64, i32, f64); 4] = [
        ("a whole second", 300, 0, 1.5),
        ("a sub-millisecond offset", 240, 1, 2.5),
        ("a whole millisecond", 180, 250_000_000, 3.5),
        ("a minute boundary", 120, 0, 4.5),
    ];
    for (case, behind, offset, value) in samples {
        // Each row is written one whole minute apart, on the minute for the boundary
        // row and one second past it otherwise, so the rows differ in what the
        // layout must render rather than in which minute they fall.
        let minute = {
            let at = now_seconds() - behind;
            at - at.rem_euclid(60)
        };
        let at = minute + i64::from(behind != 120);
        let (status, body) = call(
            &router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("e{behind}"),
                "assetId": asset_id,
                "propertyId": temperature,
                "propertyValues": [{
                    "value": {"doubleValue": value},
                    "timestamp": {"timeInSeconds": at, "offsetInNanos": offset},
                }],
            }]})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: seeding got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the sample must be accepted, got {body}"
        );
    }

    let (status, body) = query(
        &router,
        &format!(
            "SELECT event_timestamp, double_value FROM raw_time_series \
             WHERE asset_id = '{asset_id}' AND property_id = '{temperature}' \
             ORDER BY event_timestamp ASC"
        ),
    )
    .await;
    assert_eq!(status, 200, "the raw time series: got {status} {body}");
    assert_eq!(
        body["columns"][0]["type"]["scalarType"], "TIMESTAMP",
        "event_timestamp is a TIMESTAMP column, got {body}"
    );

    // Every cell carries the fixed layout, and nothing else: ten digits of date, a
    // space, eight of clock time, a dot and three of millisecond.
    let answered = rows(&body);
    assert_eq!(
        answered.len(),
        samples.len(),
        "every ingested point must be answered, got {body}"
    );
    for row in &answered {
        let cell = row[0]
            .as_deref()
            .unwrap_or_else(|| panic!("a TIMESTAMP cell with no scalarValue in {body}"));
        let shape: String = cell
            .chars()
            .map(|character| if character.is_ascii_digit() { 'd' } else { character })
            .collect();
        assert_eq!(
            shape, "dddd-dd-dd dd:dd:dd.ddd",
            "a TIMESTAMP cell is written YYYY-MM-DD HH:MM:SS.mmm in UTC, got {cell:?}"
        );
    }

    // And the cells are the instants themselves: ordering by the column puts them
    // in ascending text order, because that layout sorts exactly as the instants do.
    let mut sorted = answered
        .iter()
        .map(|row| row[0].clone())
        .collect::<Vec<Option<String>>>();
    sorted.sort();
    assert_eq!(
        answered
            .iter()
            .map(|row| row[0].clone())
            .collect::<Vec<Option<String>>>(),
        sorted,
        "ORDER BY event_timestamp ASC answers the points oldest first, got {body}"
    );
    assert_eq!(
        answered
            .iter()
            .map(|row| row[1].clone())
            .collect::<Vec<Option<String>>>(),
        vec![
            Some("1.5".to_string()),
            Some("2.5".to_string()),
            Some("3.5".to_string()),
            Some("4.5".to_string()),
        ],
        "each row keeps its own value beside its own instant, got {body}"
    );
}
