// SPDX-License-Identifier: CC0-1.0

//! What an asset-model write holds a property definition to before it stores
//! anything.
//!
//! The rule these suites pin: `CreateAssetModel` and `UpdateAssetModel` resolve
//! and parse the definition they are handed and refuse it with the modelled
//! `InvalidRequestException` (HTTP 400), storing nothing, when
//!
//! * two of one model's properties carry one NAME — a name is a handle a
//!   same-model variable resolves by
//!   (`smithy_query(query="doc", shape="VariableValue", member="propertyId")`:
//!   "You can use the property `name` if it's from the same asset model");
//! * two of one model's HIERARCHIES carry one name — a name is a handle a
//!   variable resolves a hierarchy by for the same reason
//!   (`VariableValue.hierarchyId`: "You can use the hierarchy's name instead of
//!   the hierarchy's ID");
//! * a variable names a `hierarchyId` the definition declares no hierarchy for,
//!   or a `propertyId` its resolution scope does not hold — the scope being the
//!   model being written, or the CHILD model of the hierarchy the variable names
//!   (`VariableValue.hierarchyId`: "The ID of the hierarchy to query for the
//!   property ID");
//! * a formula names an identifier that is neither a declared variable nor a
//!   function applied to arguments, or declares a variable the formula never
//!   names (`Metric.variables` / `Transform.variables`: "The list of variables
//!   used in the expression");
//! * a tumbling window's interval names a span outside the published limits
//!   (`smithy_query(query="doc", shape="TumblingWindow", member="interval")`:
//!   "The interval time must be between 1 minute and 1 week");
//! * an id-less property definition carries the NAME of a property the model
//!   already holds, which would re-create that property under a new id
//!   (`AssetModelProperty.id`: "If you are calling UpdateAssetModel to modify an
//!   existing property: This can be either the actual ID in UUID format, or else
//!   `externalId:` followed by the external ID").
//!
//! Each suite is a table whose rows differ along what its rule generalises over —
//! where the duplicated name sits, how a variable spells the property and the
//! hierarchy it names, which identifier form the expression uses, how the
//! interval is spelled and how long it is, and whether the resubmitted property
//! carries its id — and each table carries the rows the rule ACCEPTS beside the
//! ones it refuses, so a suite cannot pass by refusing everything. The wire
//! harness lives in `super::asset_lifecycle`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, property_id, router};

/// A measurement property definition, the smallest one a formula can read.
fn measurement(name: &str) -> Value {
    json!({"name": name, "dataType": "DOUBLE", "type": {"measurement": {}}})
}

/// A `metric` property definition: an expression over `variables`, aggregated
/// over the tumbling window `interval`.
fn metric(name: &str, expression: &str, variables: Value, interval: &str) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"metric": {
            "expression": expression,
            "variables": variables,
            "window": {"tumbling": {"interval": interval}},
        }},
    })
}

/// A `transform` property definition: an expression over `variables`, with no
/// window of its own.
fn transform(name: &str, expression: &str, variables: Value) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"transform": {"expression": expression, "variables": variables}},
    })
}

/// `POST /asset-models` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/asset-models", Some(body)).await
}

/// `PUT /asset-models/{model_id}` with `body`.
async fn update(router: &MultiServiceRouter, model_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(body),
    )
    .await
}

/// `GET /asset-models/{model_id}`, asserting it answered.
async fn describe(router: &MultiServiceRouter, model_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel {model_id}: got {status} {body}"
    );
    body
}

/// The property names a model reports, in definition order.
async fn described_names(router: &MultiServiceRouter, model_id: &str) -> Vec<String> {
    describe(router, model_id).await["assetModelProperties"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|property| property["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Whether the account holds an asset model named `name`.
async fn holds_name(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, body) = call(router, "GET", "/asset-models?maxResults=250", None).await;
    assert_eq!(status, 200, "ListAssetModels: got {status} {body}");
    body["assetModelSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|summary| summary["name"] == name)
}

/// Assert a refused write: the modelled bad request, carrying `message`.
fn assert_refused(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
    assert_eq!(
        error_code(body),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(message),
        "case {case:?}: expected the message {message:?}, got {body:?}"
    );
}

/// RULE: two properties of one asset model may not carry one name, whichever
/// property types they are and wherever in the list the repeat sits; the refusal
/// names the repeated name and the model is not created.
#[tokio::test]
async fn a_repeated_property_name_is_refused_and_creates_nothing() {
    let router = router();

    // Rows differ along WHERE the repeat sits (adjacent, apart, at the end), the
    // property TYPES that share the name, and the name's own value; the last two
    // rows are the accepted controls — names that differ only in case or by one
    // character are different names.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "two adjacent measurements",
            json!([measurement("ActivePower"), measurement("ActivePower")]),
            Some("ActivePower"),
        ),
        (
            "a repeat two properties later",
            json!([
                measurement("ActivePower"),
                measurement("Cycles"),
                measurement("ActivePower"),
            ]),
            Some("ActivePower"),
        ),
        (
            "a measurement and a metric sharing a name",
            json!([
                measurement("power"),
                metric(
                    "power",
                    "avg(power)",
                    json!([{"name": "power", "value": {"propertyId": "power"}}]),
                    "1h",
                ),
            ]),
            Some("power"),
        ),
        (
            "a repeated name carrying punctuation and a space",
            json!([
                measurement("Active Power (kW)"),
                measurement("Active Power (kW)"),
            ]),
            Some("Active Power (kW)"),
        ),
        (
            "names differing in case only",
            json!([measurement("ActivePower"), measurement("activepower")]),
            None,
        ),
        (
            "every name distinct",
            json!([measurement("ActivePower"), measurement("Cycles")]),
            None,
        ),
    ];

    for (at, (case, properties, duplicate)) in rows.into_iter().enumerate() {
        // A name of its own per row: `assetModelName` is unique per account.
        let name = format!("Turbine-{at}");
        let (status, body) = create(
            &router,
            json!({"assetModelName": &name, "assetModelProperties": properties}),
        )
        .await;
        match duplicate {
            Some(duplicate) => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!("Property {duplicate} name is not unique in the Asset Model."),
                );
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
            None => assert_eq!(
                status, 202,
                "case {case:?}: distinct names must be accepted, got {status} {body}"
            ),
        }
    }
}

/// RULE: two HIERARCHIES of one asset model may not carry one name either —
/// a hierarchy's name is a handle a variable may resolve it by, exactly as a
/// property's name is — and the refusal names the repeated hierarchy, on the
/// create and on the update alike, storing nothing.
#[tokio::test]
async fn a_repeated_hierarchy_name_is_refused_and_creates_nothing() {
    let router = router();
    // The child model every hierarchy below names. Its own name is distinct, so
    // only the hierarchy names are under test.
    let (status, created) = create(
        &router,
        json!({"assetModelName": "HierarchyChild", "assetModelProperties": []}),
    )
    .await;
    assert_eq!(status, 202, "the child model: got {status} {created}");
    let child = created["assetModelId"]
        .as_str()
        .expect("the child model's id")
        .to_string();

    let hierarchy = |name: &str| json!({"name": name, "childAssetModelId": child.clone()});

    // Rows differ along WHERE the repeat sits (adjacent, two later) and the
    // name's own value; the last two are the accepted controls — names that
    // differ only in case or by one character are different names.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "two adjacent hierarchies sharing a name",
            json!([hierarchy("Turbines"), hierarchy("Turbines")]),
            Some("Turbines"),
        ),
        (
            "a repeat two hierarchies later",
            json!([
                hierarchy("Turbines"),
                hierarchy("Spares"),
                hierarchy("Turbines"),
            ]),
            Some("Turbines"),
        ),
        (
            "a repeated name carrying punctuation and a space",
            json!([
                hierarchy("Spare Turbines (cold)"),
                hierarchy("Spare Turbines (cold)"),
            ]),
            Some("Spare Turbines (cold)"),
        ),
        (
            "names differing in case only",
            json!([hierarchy("Turbines"), hierarchy("turbines")]),
            None,
        ),
        (
            "every name distinct",
            json!([hierarchy("Turbines"), hierarchy("Spares")]),
            None,
        ),
    ];

    for (at, (case, hierarchies, duplicate)) in rows.into_iter().enumerate() {
        // A name of its own per row: `assetModelName` is unique per account.
        let name = format!("Farm-{at}");
        let (status, body) = create(
            &router,
            json!({"assetModelName": &name, "assetModelHierarchies": hierarchies.clone()}),
        )
        .await;
        match duplicate {
            Some(duplicate) => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!("Hierarchy {duplicate} name is not unique."),
                );
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
            None => assert_eq!(
                status, 202,
                "case {case:?}: distinct hierarchy names must be accepted, got {status} {body}"
            ),
        }

        // The UPDATE is held to the same rule, against a model that already
        // stores one hierarchy: the definition a write is handed is checked by
        // the one shared rule, whichever write hands it over.
        let updated = format!("Farm-updated-{at}");
        let (status, created) = create(
            &router,
            json!({
                "assetModelName": &updated,
                "assetModelHierarchies": [hierarchy("Held")],
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the model to update: got {status} {created}"
        );
        let model_id = created["assetModelId"]
            .as_str()
            .expect("the model's id")
            .to_string();
        let (status, body) = update(
            &router,
            &model_id,
            json!({"assetModelName": &updated, "assetModelHierarchies": hierarchies}),
        )
        .await;
        match duplicate {
            Some(duplicate) => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!("Hierarchy {duplicate} name is not unique."),
                );
                // The refused update stored nothing: the model still carries the
                // hierarchy it was created with.
                let described = describe(&router, &model_id).await;
                let held: Vec<String> = described["assetModelHierarchies"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no hierarchies in {described}"))
                    .iter()
                    .map(|hierarchy| hierarchy["name"].as_str().unwrap_or_default().to_string())
                    .collect();
                assert_eq!(
                    held,
                    vec!["Held".to_string()],
                    "case {case:?}: a refused update must change nothing, got {held:?}"
                );
            }
            None => assert_eq!(
                status, 202,
                "case {case:?}: distinct hierarchy names must be accepted by the update, got \
                 {status} {body}"
            ),
        }
    }
}

/// The child model a hierarchy row points at, and the id of the metric property
/// it carries: a property of ANOTHER model, which is what a variable may reach
/// only through a hierarchy.
async fn child_model(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, created) = create(
        router,
        json!({
            "assetModelName": name,
            "assetModelProperties": [
                {
                    "name": "ChildPower",
                    "externalId": "child_power",
                    "dataType": "DOUBLE",
                    "type": {"measurement": {}},
                },
                metric(
                    "ChildAverage",
                    "avg(power)",
                    json!([{"name": "power", "value": {"propertyId": "ChildPower"}}]),
                    "1h",
                ),
            ],
        }),
    )
    .await;
    assert_eq!(status, 202, "child model {name}: got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("child model {name}: no assetModelId in {created}"))
        .to_string();
    let described = describe(router, &model_id).await;
    let property = property_id(&described["assetModelProperties"], "ChildAverage");
    (model_id, property)
}

/// RULE: a variable resolves in ONE scope — the model being written, or the child
/// model of the hierarchy the variable names — so a `hierarchyId` the definition
/// declares no hierarchy for and a `propertyId` the scope does not hold are both
/// refused, naming the label the request sent. The NAME spelling of a
/// `propertyId` is accepted by the model being written ALONE, since it is
/// documented for a property "from the same asset model", so a reference through
/// a hierarchy has to be an identifier and anything else is an id invalid for
/// that variable.
#[tokio::test]
async fn a_variable_resolves_only_inside_its_scope() {
    let router = router();
    let (child, child_property) = child_model(&router, "Sensor").await;

    /// What a row's refusal must say, or that the row is accepted.
    enum Expected {
        Accepted,
        NoProperty(String),
        NoHierarchy(&'static str),
        /// The label is not an identifier at all, in a scope that accepts only
        /// those: the refusal names the label and the formula member carrying it.
        InvalidId(String),
    }

    // Rows differ along HOW the variable addresses its property (by id, by name,
    // by `externalId:`, by a path, not at all) and which scope it asks for (the
    // model being written, a hierarchy named by name or by label, a hierarchy
    // that is not declared).
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "a same-model property named by its id",
            json!({"propertyId": "own-id-placeholder"}),
            Expected::Accepted,
        ),
        (
            "a same-model property named by its name",
            json!({"propertyId": "LocalPower"}),
            Expected::Accepted,
        ),
        (
            "a same-model property named by its externalId",
            json!({"propertyId": "externalId:local_power"}),
            Expected::Accepted,
        ),
        (
            "no propertyId at all, the property addressed by its path",
            json!({"propertyPath": [{"name": "LocalPower"}]}),
            Expected::Accepted,
        ),
        (
            "the child model's property through the hierarchy, by hierarchy name",
            json!({"propertyId": child_property.clone(), "hierarchyId": "Turbines"}),
            Expected::Accepted,
        ),
        (
            "the child model's property with no hierarchyId",
            json!({"propertyId": child_property.clone()}),
            Expected::NoProperty(child_property.clone()),
        ),
        (
            "a hierarchyId naming no declared hierarchy",
            json!({"propertyId": child_property.clone(), "hierarchyId": "Nope"}),
            Expected::NoHierarchy("Nope"),
        ),
        (
            "a property id nothing holds",
            json!({"propertyId": "11111111-2222-3333-4444-555555555555"}),
            Expected::NoProperty("11111111-2222-3333-4444-555555555555".to_string()),
        ),
        (
            "a same-model property misspelled by name",
            json!({"propertyId": "LocalPowerr"}),
            Expected::NoProperty("LocalPowerr".to_string()),
        ),
        (
            "the child model's own property name through the hierarchy",
            json!({"propertyId": "ChildPower", "hierarchyId": "Turbines"}),
            Expected::InvalidId("ChildPower".to_string()),
        ),
        (
            "the model's OWN property name through the hierarchy",
            json!({"propertyId": "LocalPower", "hierarchyId": "Turbines"}),
            Expected::InvalidId("LocalPower".to_string()),
        ),
        (
            "an id-shaped label the child model does not hold, through the hierarchy",
            json!({
                "propertyId": "11111111-2222-3333-4444-555555555555",
                "hierarchyId": "Turbines",
            }),
            Expected::NoProperty("11111111-2222-3333-4444-555555555555".to_string()),
        ),
        (
            "the child model's own property externalId through the hierarchy",
            json!({"propertyId": "externalId:child_power", "hierarchyId": "Turbines"}),
            Expected::Accepted,
        ),
    ];

    for (at, (case, value, expected)) in rows.into_iter().enumerate() {
        let name = format!("Plant-{at}");
        // The model's own property carries a fixed id and an external id, so a row
        // can address it in each of the three ways the member documents.
        let own_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let value = if value["propertyId"] == "own-id-placeholder" {
            json!({"propertyId": own_id})
        } else {
            value
        };
        let (status, body) = create(
            &router,
            json!({
                "assetModelName": &name,
                "assetModelProperties": [
                    {
                        "id": own_id, "externalId": "local_power", "name": "LocalPower",
                        "dataType": "DOUBLE", "type": {"measurement": {}},
                    },
                    metric("Average", "avg(power)", json!([{"name": "power", "value": value}]), "1h"),
                ],
                "assetModelHierarchies": [{"name": "Turbines", "childAssetModelId": &child}],
            }),
        )
        .await;
        match expected {
            Expected::Accepted => assert_eq!(
                status, 202,
                "case {case:?}: a variable inside its scope must be accepted, got {status} {body}"
            ),
            Expected::NoProperty(property) => {
                assert_refused(case, status, &body, &format!("Property {property} does not exist."));
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
            Expected::NoHierarchy(hierarchy) => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!("AssetModelHierarchy {hierarchy} does not exist."),
                );
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
            Expected::InvalidId(label) => {
                // The formula member is the one the row's property carries: every
                // row above states its variable on a `metric`.
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!("Property Id {label} invalid for variables in Metric property."),
                );
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
        }
    }
}

/// RULE: a formula is parsed against the variables it declares — every identifier
/// in it is a declared variable or a function applied to arguments, and every
/// declared variable appears in it — and the refusal names the expression, the
/// offending token and where in the expression it starts.
#[tokio::test]
async fn a_formula_is_parsed_against_the_variables_it_declares() {
    let router = router();

    /// What a row's refusal must say, or that the row is accepted.
    enum Expected {
        Accepted,
        /// `(token, position)` of the identifier nothing declares.
        Unknown(&'static str, usize),
        /// The declared variable the expression never names.
        Unused(&'static str),
    }

    // Rows differ along the FORM of the expression: an arithmetic formula, a
    // function applied to a variable, a function applied to an undeclared token,
    // a constant formula, a quoted literal, a number with an exponent, and a
    // formula naming two variables of which one is unused.
    let rows: Vec<(&str, &str, Vec<&str>, Expected)> = vec![
        ("an aggregate of a declared variable", "avg(power)", vec!["power"], Expected::Accepted),
        ("arithmetic over a declared variable", "power * 9 / 5 + 32", vec!["power"], Expected::Accepted),
        (
            "nested functions over both declared variables",
            "max(avg(power), sum(cycles))",
            vec!["power", "cycles"],
            Expected::Accepted,
        ),
        (
            "a quoted literal beside a declared variable",
            "if(power > 0, 'up', 'down')",
            vec!["power"],
            Expected::Accepted,
        ),
        (
            "a number carrying an exponent",
            "power * 1e3",
            vec!["power"],
            Expected::Accepted,
        ),
        (
            "a boolean literal beside a declared variable",
            "if(power > 0, true, false)",
            vec!["power"],
            Expected::Accepted,
        ),
        (
            "an aggregate of an undeclared token",
            "avg(nope)",
            vec!["power"],
            Expected::Unknown("nope", 4),
        ),
        (
            "an undeclared token in an arithmetic formula",
            "power + nope",
            vec!["power"],
            Expected::Unknown("nope", 8),
        ),
        (
            "an undeclared token ahead of the declared one",
            "nope - power",
            vec!["power"],
            Expected::Unknown("nope", 0),
        ),
        ("a constant formula", "1 + 1", vec!["power"], Expected::Unused("power")),
        (
            "one of two declared variables unused",
            "avg(power)",
            vec!["power", "cycles"],
            Expected::Unused("cycles"),
        ),
    ];

    for (at, (case, expression, variables, expected)) in rows.into_iter().enumerate() {
        let name = format!("Mill-{at}");
        let declared: Vec<Value> = variables
            .iter()
            .map(|variable| json!({"name": variable, "value": {"propertyId": "Power"}}))
            .collect();
        let (status, body) = create(
            &router,
            json!({
                "assetModelName": &name,
                "assetModelProperties": [
                    measurement("Power"),
                    metric("Average", expression, json!(declared), "1h"),
                ],
            }),
        )
        .await;
        match expected {
            Expected::Accepted => assert_eq!(
                status, 202,
                "case {case:?}: a formula over its declared variables must be accepted, \
                 got {status} {body}"
            ),
            Expected::Unknown(token, position) => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!(
                        "`{expression}`: Unknown function or variable '{token}' at pos \
                         {position} in expression '{expression}'"
                    ),
                );
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
            Expected::Unused(variable) => {
                assert_refused(
                    case,
                    status,
                    &body,
                    &format!(
                        "`{expression}`: Variable [{variable}] is unused in formula. \
                         Formulas must use all defined variables"
                    ),
                );
                assert!(
                    !holds_name(&router, &name).await,
                    "case {case:?}: a refused create must leave no asset model behind"
                );
            }
        }
    }
}

/// RULE: a transform's expression is parsed by the same rule as a metric's — the
/// window is the only member a metric adds — so a transform declaring a variable
/// its expression never names is refused too.
#[tokio::test]
async fn a_transforms_expression_is_parsed_by_the_same_rule() {
    let router = router();

    for (at, (case, expression, expected)) in [
        ("a transform over its declared variable", "power * 2", None),
        (
            "a transform naming an undeclared token",
            "power + nope",
            Some("`power + nope`: Unknown function or variable 'nope' at pos 8 in expression 'power + nope'"),
        ),
        (
            "a transform whose expression names no variable",
            "0",
            Some(
                "`0`: Variable [power] is unused in formula. Formulas must use all defined \
                 variables",
            ),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("Pump-{at}");
        let (status, body) = create(
            &router,
            json!({
                "assetModelName": &name,
                "assetModelProperties": [
                    measurement("Power"),
                    transform(
                        "Doubled",
                        expression,
                        json!([{"name": "power", "value": {"propertyId": "Power"}}]),
                    ),
                ],
            }),
        )
        .await;
        match expected {
            None => assert_eq!(
                status, 202,
                "case {case:?}: expected 202, got {status} {body}"
            ),
            Some(message) => assert_refused(case, status, &body, message),
        }
    }
}

/// RULE: a tumbling window's interval names a span between 1 minute and 1 week,
/// in either spelling the shape documents; outside that range — and in a spelling
/// that names no span at all — the write is refused, naming the metric's
/// expression and the interval as the request spelled it.
#[tokio::test]
async fn a_tumbling_interval_outside_the_published_limits_is_refused() {
    let router = router();

    // Rows: both published bounds, one step past each, the ISO 8601 spelling of
    // each, and text that names no span. Source for the spellings:
    // `smithy_query(query="shape", shape="TumblingWindow")` — "You can use `m`,
    // `h`, `d`, and `w` when you specify an interval or offset… The `interval` and
    // `offset` parameters support the ISO 8601 format."
    let rows: [(&str, &str, bool); 12] = [
        ("the published floor", "1m", true),
        ("the published ceiling", "1w", true),
        ("inside the range, in hours", "1h", true),
        ("inside the range, in days", "6d", true),
        ("the ceiling spelled in days", "7d", true),
        ("the floor in ISO 8601", "PT1M", true),
        ("the ceiling in ISO 8601 hours", "PT168H", true),
        ("below the floor", "30s", false),
        ("one second below the floor", "59s", false),
        ("past the ceiling", "8d", false),
        ("below the floor in ISO 8601", "PT30S", false),
        ("an interval naming no span", "later", false),
    ];

    for (at, (case, interval, accepted)) in rows.into_iter().enumerate() {
        let name = format!("Boiler-{at}");
        let (status, body) = create(
            &router,
            json!({
                "assetModelName": &name,
                "assetModelProperties": [
                    measurement("Power"),
                    metric(
                        "Average",
                        "avg(power)",
                        json!([{"name": "power", "value": {"propertyId": "Power"}}]),
                        interval,
                    ),
                ],
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 202,
                "case {case:?}: the interval {interval} is inside the limits, got {status} {body}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                &format!("`avg(power)`: Window time interval '{interval}' is not within limits"),
            );
            assert!(
                !holds_name(&router, &name).await,
                "case {case:?}: a refused create must leave no asset model behind"
            );
        }
    }
}

/// RULE: the formula reading is TOTAL over every string a caller can send — an
/// expression that is malformed, truncated, deeply nested or at the modelled
/// length bound is answered with a modelled status, never with a server failure
/// and never by hanging.
#[tokio::test]
async fn a_malformed_or_oversized_expression_is_answered_not_crashed() {
    let router = router();

    // The declared variable every row below reads, named by the model's own
    // property, so what each row varies is only the TEXT of the expression.
    let variables = json!([{"name": "power", "value": {"propertyId": "Power"}}]);

    // `Expression` is `@length 0..1024`
    // (`smithy_query(query="shape", shape="Metric")`), so a formula of exactly the
    // bound is the largest a caller can send and one past it is refused by the
    // modelled constraint rather than by these rules.
    let at_the_bound = {
        let mut expression = String::from("power");
        while expression.len() + 8 <= 1024 {
            expression.push_str(" + power");
        }
        expression
    };
    let past_the_bound = format!("{at_the_bound} + power + power");
    // 200 nested calls: the reading is a single pass, so a caller's nesting depth
    // is never a depth of this emulator's own.
    let deeply_nested = format!("{}power{}", "avg(".repeat(200), ")".repeat(200));

    let rows: Vec<(&str, String)> = vec![
        ("an unterminated quoted literal", "power + 'tail".to_string()),
        ("an unterminated double quote", "power + \"tail".to_string()),
        ("unbalanced parentheses", "avg(power".to_string()),
        ("a truncated formula", "power +".to_string()),
        ("nothing but operators", "+-*/".to_string()),
        ("an empty expression", String::new()),
        ("only whitespace", "   ".to_string()),
        ("a non-ASCII identifier", "power + Leistungß".to_string()),
        ("a formula at the modelled length bound", at_the_bound),
        ("a formula past the modelled length bound", past_the_bound),
        ("two hundred nested calls", deeply_nested),
    ];

    for (at, (case, expression)) in rows.into_iter().enumerate() {
        let name = format!("Press-{at}");
        let (status, body) = create(
            &router,
            json!({
                "assetModelName": &name,
                "assetModelProperties": [
                    measurement("Power"),
                    metric("Average", &expression, variables.clone(), "1h"),
                ],
            }),
        )
        .await;
        assert!(
            status == 202 || status == 400,
            "case {case:?}: expected a modelled answer, got {status} {body}"
        );
        if status == 400 {
            // Either of the two modelled bad requests: this module's own refusal,
            // or the framework's constraint walk for a value outside the shape.
            let code = error_code(&body);
            assert!(
                code == "InvalidRequestException" || code == "ValidationException",
                "case {case:?}: expected a modelled bad request, got {body}"
            );
        }
    }
}

/// RULE: a formula declares at most the published number of variables, whatever
/// the request sends — `Metric.expression` and `Transform.expression` both state
/// "You can specify up to 10 variables per expression".
#[tokio::test]
async fn a_formula_declares_at_most_the_published_number_of_variables() {
    let router = router();

    // The published quota, one below it and one above it: the bound and the step
    // past it, over a formula that uses every variable it declares.
    for (case, declared) in [("one short of the quota", 9), ("at the quota", 10), ("past the quota", 11)] {
        let name = format!("Fleet-{declared}");
        let properties: Vec<Value> = (0..declared)
            .map(|at| measurement(&format!("Power{at}")))
            .collect();
        let variables: Vec<Value> = (0..declared)
            .map(|at| json!({"name": format!("power{at}"), "value": {"propertyId": format!("Power{at}")}}))
            .collect();
        let expression = (0..declared)
            .map(|at| format!("power{at}"))
            .collect::<Vec<String>>()
            .join(" + ");
        let mut definitions = properties;
        definitions.push(metric("Total", &expression, json!(variables), "1h"));
        let (status, body) = create(
            &router,
            json!({"assetModelName": &name, "assetModelProperties": definitions}),
        )
        .await;
        if declared <= 10 {
            assert_eq!(
                status, 202,
                "case {case:?}: {declared} variables are inside the quota, got {status} {body}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                &format!(
                    "`{expression}`: {declared} variables are defined, but a formula may \
                     define at most 10"
                ),
            );
        }
    }
}

/// RULE (the resolution scope, from the composite-model side): what a
/// component-model-based composite model LENDS is part of the asset model's own
/// scope, so a write to that composite model — a rename that states no properties
/// of its own — leaves every formula that reads a lent property resolving, and is
/// accepted.
#[tokio::test]
async fn a_rename_of_a_component_based_composite_model_keeps_the_lent_scope() {
    let router = router();

    // The property the component model lends, under a caller-supplied id so the
    // asset model's own formula can name it before anything mints one.
    let lent = "11111111-2222-4333-8444-555555555555";
    let (status, component) = create(
        &router,
        json!({
            "assetModelName": "Pump",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [{
                "id": lent, "name": "Setpoint", "dataType": "DOUBLE",
                "type": {"measurement": {}},
            }],
        }),
    )
    .await;
    assert_eq!(status, 202, "create the component model: got {status} {component}");
    let component_id = component["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelId in {component}"))
        .to_string();

    let (status, created) = create(&router, json!({"assetModelName": "Boiler"})).await;
    assert_eq!(status, 202, "create the asset model: got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelId in {created}"))
        .to_string();

    let (status, composite) = crate::asset_model_composite_model::create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "ThePump",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &component_id,
        }),
    )
    .await;
    assert_eq!(status, 202, "create the composite model: got {status} {composite}");
    let composite_id = composite["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelId in {composite}"))
        .to_string();

    // The asset model's OWN metric reads the lent property, which it may because
    // the composite model above lends it into this model's scope.
    let (status, updated) = update(
        &router,
        &model_id,
        json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [metric(
                "ModelAverage",
                "avg(lent)",
                json!([{"name": "lent", "value": {"propertyId": lent}}]),
                "1h",
            )],
            "assetModelCompositeModels": [{
                "id": &composite_id, "name": "ThePump", "type": "CUSTOM",
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "a formula over a lent property must be accepted, got {status} {updated}"
    );

    // Renaming the composite model states no properties of its own — a
    // component-model-based one has none — and the component model goes on lending
    // them, so the model's own formula still resolves and the write is accepted.
    let (status, renamed) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model_id}/composite-models/{composite_id}"),
        Some(json!({"assetModelCompositeModelName": "TheOtherPump"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "a rename of a component-model-based composite model must be accepted, \
         got {status} {renamed}"
    );
    let (status, described) = crate::asset_model_composite_model::describe_composite(
        &router,
        &model_id,
        &composite_id,
    )
    .await;
    assert_eq!(status, 200, "describe the renamed composite: got {status} {described}");
    assert_eq!(
        described["assetModelCompositeModelName"].as_str(),
        Some("TheOtherPump"),
        "the rename must be stored, got {described}"
    );
    assert_eq!(
        described["assetModelCompositeModelProperties"][0]["id"].as_str(),
        Some(lent),
        "the composite model must go on lending the component model's property, got {described}"
    );
}

/// RULE: the bounds a definition is held to on the create are the ones an update
/// of the same model is held to, so nothing can be created outside what an update
/// would accept: the composite models one asset model carries and the properties
/// each of them states are both capped, and a request past either is the modelled
/// quota refusal (HTTP 410) that stores nothing.
#[tokio::test]
async fn the_create_holds_a_definition_to_the_same_bounds_as_the_update() {
    let router = router();

    /// A composite model definition carrying `properties`.
    fn composite(at: usize, properties: Vec<Value>) -> Value {
        json!({
            "name": format!("Part{at}"),
            "type": "CUSTOM",
            "properties": properties,
        })
    }

    // The two published per-model bounds, each at the cap and one past it:
    // `assetModelCompositeModelProperties` states "You can specify up to 200
    // properties per composite model", and the composite-model list carries the
    // same ceiling the update holds it to.
    for (case, composite_models, properties_each, accepted) in [
        ("at the composite-model cap", 100, 1, true),
        ("past the composite-model cap", 101, 1, false),
        ("at the per-composite property cap", 1, 200, true),
        ("past the per-composite property cap", 1, 201, false),
    ] {
        let name = format!("Assembly-{case}");
        let definitions: Vec<Value> = (0..composite_models)
            .map(|at| {
                composite(
                    at,
                    (0..properties_each)
                        .map(|which| measurement(&format!("Part{at}Power{which}")))
                        .collect(),
                )
            })
            .collect();
        let (status, body) = create(
            &router,
            json!({"assetModelName": &name, "assetModelCompositeModels": definitions}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 202,
                "case {case:?}: a definition inside the bounds must be accepted, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 410,
                "case {case:?}: expected the modelled quota refusal, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "LimitExceededException",
                "case {case:?}: expected LimitExceededException, got {body}"
            );
            assert!(
                !holds_name(&router, &name).await,
                "case {case:?}: a refused create must leave no asset model behind"
            );
        }
    }
}

/// RULE: `UpdateAssetModel` replaces the whole model, so a definition it carries
/// no stored id for CREATES a property — and a creation whose name an existing
/// property already holds is refused, naming the property it would have replaced;
/// the stored property set is unchanged. A definition that names the stored
/// property by its id (or `externalId:`) MODIFIES it and keeps that id.
#[tokio::test]
async fn an_id_less_definition_may_not_replace_a_property_of_the_same_name() {
    let router = router();

    /// How the update addresses the property it resubmits.
    #[derive(Debug, Clone, Copy)]
    enum By {
        /// No id at all — the refused replacement.
        Name,
        /// The stored id.
        Id,
        /// `externalId:` and the stored external id.
        ExternalId,
        /// A caller-supplied id of its own, which still re-creates the property.
        ForeignId,
        /// A name the model holds no property of: a plain creation.
        NewName,
    }

    for (at, (case, by)) in [
        ("resubmitted by name alone", By::Name),
        ("resubmitted by its stored id", By::Id),
        ("resubmitted by its externalId label", By::ExternalId),
        ("resubmitted under an id of the caller's own", By::ForeignId),
        ("a definition under a name the model does not hold", By::NewName),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("Kiln-{at}");
        let (status, created) = create(
            &router,
            json!({
                "assetModelName": &name,
                "assetModelProperties": [{
                    "externalId": "active_power", "name": "ActivePower",
                    "dataType": "DOUBLE", "type": {"measurement": {}},
                }],
            }),
        )
        .await;
        assert_eq!(status, 202, "case {case:?}: create got {status} {created}");
        let model_id = created["assetModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
            .to_string();
        let stored = property_id(
            &describe(&router, &model_id).await["assetModelProperties"],
            "ActivePower",
        );

        let mut property = json!({
            "name": "ActivePower", "dataType": "DOUBLE", "type": {"measurement": {}},
        });
        match by {
            By::Name => {}
            By::Id => property["id"] = json!(&stored),
            By::ExternalId => property["id"] = json!("externalId:active_power"),
            By::ForeignId => property["id"] = json!("99999999-8888-4777-8666-555555555555"),
            By::NewName => property["name"] = json!("ReactivePower"),
        }
        let (status, answered) = update(
            &router,
            &model_id,
            json!({"assetModelName": &name, "assetModelProperties": [property]}),
        )
        .await;

        match by {
            By::Name | By::ForeignId => {
                assert_refused(
                    case,
                    status,
                    &answered,
                    &format!(
                        "Cannot create property ActivePower. Creation will result in \
                         replacement of property {stored}"
                    ),
                );
                // The refused update changed nothing a later caller sees: the
                // property is still there, under the id it was minted with.
                let after = describe(&router, &model_id).await;
                assert_eq!(
                    property_id(&after["assetModelProperties"], "ActivePower"),
                    stored,
                    "case {case:?}: a refused update must leave the property alone, got {after}"
                );
            }
            By::Id | By::ExternalId => {
                assert_eq!(
                    status, 202,
                    "case {case:?}: naming the stored property must be accepted, \
                     got {status} {answered}"
                );
                let after = describe(&router, &model_id).await;
                assert_eq!(
                    property_id(&after["assetModelProperties"], "ActivePower"),
                    stored,
                    "case {case:?}: the modified property must keep its stored id, got {after}"
                );
            }
            By::NewName => {
                assert_eq!(
                    status, 202,
                    "case {case:?}: a new name is a creation, got {status} {answered}"
                );
                assert_eq!(
                    described_names(&router, &model_id).await,
                    vec!["ReactivePower".to_string()],
                    "case {case:?}: the update states the whole property set"
                );
            }
        }
    }
}

/// RULE (the same four, from the update side): an update is held to exactly the
/// rules the create is, so a definition the create would have refused cannot
/// reach the model through an update either, and the stored definition survives
/// the refusal.
#[tokio::test]
async fn an_update_is_held_to_the_same_rules_as_the_create() {
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "assetModelName": "Turbine",
            "assetModelProperties": [
                measurement("Power"),
                metric(
                    "Average",
                    "avg(power)",
                    json!([{"name": "power", "value": {"propertyId": "Power"}}]),
                    "1h",
                ),
            ],
        }),
    )
    .await;
    assert_eq!(status, 202, "create: got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelId in {created}"))
        .to_string();
    let before = describe(&router, &model_id).await;
    let power = property_id(&before["assetModelProperties"], "Power");
    let average = property_id(&before["assetModelProperties"], "Average");

    // One row per rule, each resubmitting the stored properties by their ids so the
    // only thing the row changes is the definition the rule refuses.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a repeated property name",
            json!([
                {"id": &power, "name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"id": &average, "name": "Power", "dataType": "DOUBLE", "type": {"metric": {
                    "expression": "avg(power)",
                    "variables": [{"name": "power", "value": {"propertyId": &power}}],
                    "window": {"tumbling": {"interval": "1h"}},
                }}},
            ]),
            "Property Power name is not unique in the Asset Model.".to_string(),
        ),
        (
            "a variable naming a property nothing holds",
            json!([
                {"id": &power, "name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"id": &average, "name": "Average", "dataType": "DOUBLE", "type": {"metric": {
                    "expression": "avg(power)",
                    "variables": [{"name": "power", "value": {"propertyId": "Gone"}}],
                    "window": {"tumbling": {"interval": "1h"}},
                }}},
            ]),
            "Property Gone does not exist.".to_string(),
        ),
        (
            "a hierarchyId the definition does not declare",
            json!([
                {"id": &power, "name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"id": &average, "name": "Average", "dataType": "DOUBLE", "type": {"metric": {
                    "expression": "avg(power)",
                    "variables": [{
                        "name": "power",
                        "value": {"propertyId": &power, "hierarchyId": "Nope"},
                    }],
                    "window": {"tumbling": {"interval": "1h"}},
                }}},
            ]),
            "AssetModelHierarchy Nope does not exist.".to_string(),
        ),
        (
            "an expression naming an undeclared token",
            json!([
                {"id": &power, "name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"id": &average, "name": "Average", "dataType": "DOUBLE", "type": {"metric": {
                    "expression": "avg(nope)",
                    "variables": [{"name": "power", "value": {"propertyId": &power}}],
                    "window": {"tumbling": {"interval": "1h"}},
                }}},
            ]),
            "`avg(nope)`: Unknown function or variable 'nope' at pos 4 in expression \
             'avg(nope)'"
                .to_string(),
        ),
        (
            "an interval below the published floor",
            json!([
                {"id": &power, "name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"id": &average, "name": "Average", "dataType": "DOUBLE", "type": {"metric": {
                    "expression": "avg(power)",
                    "variables": [{"name": "power", "value": {"propertyId": &power}}],
                    "window": {"tumbling": {"interval": "30s"}},
                }}},
            ]),
            "`avg(power)`: Window time interval '30s' is not within limits".to_string(),
        ),
    ];

    for (case, properties, message) in rows {
        let (status, answered) = update(
            &router,
            &model_id,
            json!({"assetModelName": "Turbine", "assetModelProperties": properties}),
        )
        .await;
        assert_refused(case, status, &answered, &message);
        // A refused update changes nothing a later caller sees.
        let after = describe(&router, &model_id).await;
        assert_eq!(
            after["assetModelProperties"], before["assetModelProperties"],
            "case {case:?}: a refused update must leave the definition alone, got {after}"
        );
    }

    // The same update with every rule satisfied is accepted, so the suite cannot
    // pass by refusing everything: the metric's window widens and its expression
    // takes the property the model holds.
    let (status, answered) = update(
        &router,
        &model_id,
        json!({
            "assetModelName": "Turbine",
            "assetModelProperties": [
                {"id": &power, "name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"id": &average, "name": "Average", "dataType": "DOUBLE", "type": {"metric": {
                    "expression": "sum(power)",
                    "variables": [{"name": "power", "value": {"propertyId": &power}}],
                    "window": {"tumbling": {"interval": "1d"}},
                }}},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "a definition satisfying every rule must be accepted, got {status} {answered}"
    );
}
