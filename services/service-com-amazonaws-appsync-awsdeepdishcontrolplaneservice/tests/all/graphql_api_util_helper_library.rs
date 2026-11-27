// SPDX-License-Identifier: CC0-1.0

//! The `$util` HELPER LIBRARY a VTL mapping template reaches, driven over a GraphQL API's own
//! published endpoint.
//!
//! The rules these cases assert, one level above the recorded case (one `NONE` data source and
//! one unit resolver whose response template calls the helpers in a single pass):
//!
//! > Every helper the AppSync resolver-utility reference publishes has a DEFINED result a
//! > mapping template may call for, so a template calling one is answered `200` with the value
//! > that helper computes under `data` and no `errors` member — the string, math, list, map,
//! > DynamoDB-marshalling and clock families alike — and only a name the reference publishes
//! > none of is reported as a helper this endpoint does not carry.
//!
//! > `$util.transform.toDynamoDBFilterExpression` answers, as JSON TEXT, the DynamoDB
//! > expression document the stated filter comes to — the `expression` text over the
//! > `expressionNames` its attribute names travel under and the `expressionValues` its
//! > operands travel under — and an input it
//! > cannot build one from fails only that field: `200`, a `null` value, and exactly one entry
//! > in `errors` for that path carrying `errorType` `DynamoDBFilterError` and a message whose
//! > fixed text is followed by the stated filter as the resolver's collections write
//! > themselves. A sibling field of the same request still resolves.
//!
//! The rows span what the rules generalise over: the helper FAMILY and the helper within it,
//! the kind of value a helper is handed and answers (text, a number, a list, a map, a
//! marshalled attribute), the ORDERING a sort is asked in, the OPERATOR a filter compares
//! under (containment, equality, a range, an existence question, an `or` group), the
//! arguments no helper can compute with, and the two inputs a filter transformation refuses —
//! with every token-derived part of an answer driven from the request's own arguments rather
//! than written into the template.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    emulator, install_schema, post_graphql, GraphqlApi,
};

/// The name of the local data source every resolver of this suite resolves against: `NONE` is
/// "There is no data source", so the request half's own payload is the result and the response
/// half is where the helpers are called.
const DATA_SOURCE: &str = "library";

/// The request template every resolver of this suite carries: the `payload` a `NONE` data
/// source passes through, which is the documented form for a local resolver.
const REQUEST_TEMPLATE: &str = "{\"version\": \"2017-02-28\", \"payload\": {}}";

/// The argument every field of this suite takes, so a helper's variable part can come from the
/// REQUEST rather than from the template text.
const TOKEN_ARGUMENT: &str = "token: String!";

/// What one helper row expects of the field it is attached to.
enum Expect {
    /// The exact value the field must answer.
    Exactly(Value),
    /// A whole number of seconds since the epoch: `$util.time.nowEpochSeconds` answers "the
    /// current epoch second", so the value is an integer rather than a fractional reading of
    /// a finer clock.
    WholeSeconds,
    /// A number the helper DRAWS, which has no one value: it must lie in `[low, high)`, which
    /// is the range the reference states the draw is made within.
    Drawn(f64, f64),
}

/// One row of the helper table: the helper it calls, the GraphQL type the field answers under,
/// and what the field must answer.
struct Helper {
    case: &'static str,
    field: &'static str,
    graphql_type: &'static str,
    response: &'static str,
    expect: Expect,
}

#[tokio::test]
async fn every_documented_util_helper_answers_the_result_the_reference_defines() {
    // Source of the helper names and of what each answers: the AppSync resolver mapping-template
    // utility reference — `$util.str` (`toUpper`, `toLower`, `toReplace`, `normalize`),
    // `$util.math` (`roundNum`, `minVal`, `maxVal`), `$util.list` (`copyAndRetainAll`,
    // `copyAndRemoveAll`, `sortList`), `$util.map` (`copyAndRetainAllKeys`,
    // `copyAndRemoveAllKeys`), `$util.dynamodb` (`toDynamoDB`) and `$util.time`
    // (`nowEpochSeconds`). The rows include helpers the issue cited and helpers it did not.
    let rows: &[Helper] = &[
        Helper {
            case: "a replacement of one substring",
            field: "replaced",
            graphql_type: "String",
            response: "$util.toJson($util.str.toReplace(\"hello world\", \"hello\", \"mellow\"))",
            expect: Expect::Exactly(json!("mellow world")),
        },
        Helper {
            case: "a replacement whose replacement comes from the request",
            field: "replacedFromRequest",
            graphql_type: "String",
            response: "$util.toJson($util.str.toReplace(\"hello world\", \"world\", \
                       $context.arguments.token))",
            expect: Expect::Exactly(json!("hello settled")),
        },
        Helper {
            case: "text raised to upper case",
            field: "raised",
            graphql_type: "String",
            response: "$util.toJson($util.str.toUpper(\"MiXeD case\"))",
            expect: Expect::Exactly(json!("MIXED CASE")),
        },
        Helper {
            case: "text lowered",
            field: "lowered",
            graphql_type: "String",
            response: "$util.toJson($util.str.toLower(\"MiXeD case\"))",
            expect: Expect::Exactly(json!("mixed case")),
        },
        Helper {
            case: "a composing normalization",
            field: "composed",
            graphql_type: "String",
            response: "$util.toJson($util.str.normalize(\"e\u{301}\", \"nfc\"))",
            expect: Expect::Exactly(json!("\u{e9}")),
        },
        Helper {
            case: "the decomposing normalization of the same text",
            field: "decomposed",
            graphql_type: "String",
            response: "$util.toJson($util.str.normalize(\"\u{e9}\", \"NFD\"))",
            expect: Expect::Exactly(json!("e\u{301}")),
        },
        Helper {
            case: "the larger of two numbers",
            field: "largest",
            graphql_type: "Float",
            response: "$util.toJson($util.math.maxVal(3.5, 7.25))",
            expect: Expect::Exactly(json!(7.25)),
        },
        Helper {
            case: "the smaller of the same two",
            field: "smallest",
            graphql_type: "Float",
            response: "$util.toJson($util.math.minVal(3.5, 7.25))",
            expect: Expect::Exactly(json!(3.5)),
        },
        Helper {
            case: "a number that rounds up",
            field: "roundedUp",
            graphql_type: "Int",
            response: "$util.toJson($util.math.roundNum(2.5))",
            expect: Expect::Exactly(json!(3)),
        },
        Helper {
            case: "a number that rounds down",
            field: "roundedDown",
            graphql_type: "Int",
            response: "$util.toJson($util.math.roundNum(2.4))",
            expect: Expect::Exactly(json!(2)),
        },
        Helper {
            case: "a list narrowed to the elements named",
            field: "retained",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.copyAndRetainAll([\"red\", \"green\", \"blue\"], \
                       [\"green\", \"blue\"]))",
            expect: Expect::Exactly(json!("[\"green\",\"blue\"]")),
        },
        Helper {
            case: "the same list with those elements removed",
            field: "removed",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.copyAndRemoveAll([\"red\", \"green\", \"blue\"], \
                       [\"green\", \"blue\"]))",
            expect: Expect::Exactly(json!("[\"red\"]")),
        },
        Helper {
            case: "numbers retained against the same values written another way",
            field: "retainedNumbers",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.copyAndRetainAll([1, 2, 3], [1.0, 3]))",
            expect: Expect::Exactly(json!("[1,3]")),
        },
        Helper {
            case: "a list sorted ascending",
            field: "sortedUp",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.sortList([3, 1, 2], false, \"\"))",
            expect: Expect::Exactly(json!("[1,2,3]")),
        },
        Helper {
            case: "the same list sorted descending",
            field: "sortedDown",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.sortList([3, 1, 2], true, \"\"))",
            expect: Expect::Exactly(json!("[3,2,1]")),
        },
        Helper {
            case: "a map narrowed to the keys named",
            field: "keptKeys",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.map.copyAndRetainAllKeys({\"a\": 1, \"b\": 2, \
                       \"c\": 3}, [\"b\", \"c\"]))",
            expect: Expect::Exactly(json!("{\"b\":2,\"c\":3}")),
        },
        Helper {
            case: "the same map with those keys removed",
            field: "droppedKeys",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.map.copyAndRemoveAllKeys({\"a\": 1, \"b\": 2, \
                       \"c\": 3}, [\"b\", \"c\"]))",
            expect: Expect::Exactly(json!("{\"a\":1}")),
        },
        Helper {
            case: "an argument marshalled as the attribute value DynamoDB holds it as",
            field: "marshalled",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.dynamodb.toDynamoDB($context.arguments.token))",
            expect: Expect::Exactly(json!("{\"S\":\"settled\"}")),
        },
        Helper {
            case: "a flag marshalled the same way",
            field: "marshalledFlag",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.dynamodb.toDynamoDB(true))",
            expect: Expect::Exactly(json!("{\"BOOL\":true}")),
        },
        Helper {
            case: "the current epoch second",
            field: "epoch",
            graphql_type: "Int",
            response: "$util.toJson($util.time.nowEpochSeconds())",
            expect: Expect::WholeSeconds,
        },
        Helper {
            case: "a list of mixed kinds sorted, which orders by kind and then by value",
            field: "sortedMixed",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.sortList([2, \"b\", 1, \"a\", true, null], false, \
                       \"\"))",
            expect: Expect::Exactly(json!("[null,true,1,2,\"a\",\"b\"]")),
        },
        Helper {
            case: "the same mixed list sorted the other way",
            field: "sortedMixedDown",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.sortList([2, \"b\", 1, \"a\", true, null], true, \
                       \"\"))",
            expect: Expect::Exactly(json!("[\"b\",\"a\",2,1,true,null]")),
        },
        Helper {
            case: "maps sorted by the property the call names",
            field: "sortedByProperty",
            graphql_type: "AWSJSON",
            response: "$util.toJson($util.list.sortList([{\"rank\": 2}, {\"rank\": 1}], false, \
                       \"rank\"))",
            expect: Expect::Exactly(json!("[{\"rank\":1},{\"rank\":2}]")),
        },
        Helper {
            case: "a drawn double",
            field: "drawn",
            graphql_type: "Float",
            response: "$util.toJson($util.math.randomDouble())",
            expect: Expect::Drawn(0.0, 1.0),
        },
        Helper {
            case: "a drawn integer within a range of one value",
            field: "drawnWithinRange",
            graphql_type: "Int",
            response: "$util.toJson($util.math.randomWithinRange(1, 2))",
            expect: Expect::Exactly(json!(1)),
        },
    ];

    let router = emulator();
    let fields: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| (row.field, row.graphql_type))
        .collect();
    let definitions: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| (row.field, row.response))
        .collect();
    let api = create_api(&router, "helpers", &fields, &definitions).await;

    // ONE request selecting every field, so the whole library is exercised in a single pass —
    // which is what a template written against the reference does.
    let case = "every documented helper in one request";
    let (status, answered) = post_graphql(
        case,
        &api,
        &json!({
            "query": document(&fields),
            "variables": {"token": TOKEN},
        }),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
    );
    assert!(
        answered.get("errors").is_none(),
        "case {case:?}: every documented helper has a result, got {answered}"
    );
    for row in rows {
        let value = &answered["data"][row.field];
        match &row.expect {
            Expect::Exactly(expected) => assert_eq!(
                value, expected,
                "case {:?}: expected {expected}, got {value}",
                row.case
            ),
            Expect::WholeSeconds => assert!(
                value.is_i64() && value.as_i64().is_some_and(|seconds| seconds > 0),
                "case {:?}: expected a whole number of seconds, got {value}",
                row.case
            ),
            Expect::Drawn(low, high) => assert!(
                value
                    .as_f64()
                    .is_some_and(|drawn| drawn >= *low && drawn < *high),
                "case {:?}: expected a number drawn within [{low}, {high}), got {value}",
                row.case
            ),
        }
    }
}

#[tokio::test]
async fn a_helper_handed_a_value_it_cannot_compute_with_reports_a_fault_for_its_field() {
    // The rows span the kinds of argument a helper cannot compute with: a replacement naming
    // nothing to replace, a number this language cannot hold (no JSON value is `NaN` or an
    // infinity, so a reading that is neither arrives only as text), a range that runs
    // backwards, a normalization form that does not exist, and a collection helper handed a
    // value of the wrong kind. Each one must be reported as a FIELD that did not resolve,
    // beside a sibling that still does.
    let rows: &[(&str, &str, &str)] = &[
        (
            "a replacement naming nothing to replace",
            "emptyPattern",
            "$util.toJson($util.str.toReplace(\"hello\", \"\", \"-\"))",
        ),
        (
            "a draw whose bound is not a number at all",
            "notANumber",
            "$util.toJson($util.math.randomWithinRange(\"NaN\", 5))",
        ),
        (
            "a draw whose bound has no finite value",
            "notFinite",
            "$util.toJson($util.math.randomWithinRange(1, \"inf\"))",
        ),
        (
            "a draw over a range that runs backwards",
            "backwards",
            "$util.toJson($util.math.randomWithinRange(9, 2))",
        ),
        (
            "a draw over two finite bounds whose width is more than a number holds",
            "tooWide",
            "$util.toJson($util.math.randomWithinRange(\"-1.7e308\", \"1.7e308\"))",
        ),
        (
            "a normalization form that does not exist",
            "noSuchForm",
            "$util.toJson($util.str.normalize(\"text\", \"nfq\"))",
        ),
        (
            "a list helper handed something that is no list",
            "notAList",
            "$util.toJson($util.list.copyAndRetainAll(\"red\", [\"red\"]))",
        ),
        (
            "a map helper handed something that is no map",
            "notAMap",
            "$util.toJson($util.map.copyAndRetainAllKeys([1], [\"a\"]))",
        ),
    ];

    let router = emulator();
    let mut fields: Vec<(&str, &str)> = rows
        .iter()
        .map(|(_, field, _)| (*field, "AWSJSON"))
        .collect();
    let mut definitions: Vec<(&str, &str)> = rows
        .iter()
        .map(|(_, field, response)| (*field, *response))
        .collect();
    // The sibling every row is answered beside: one field whose helper computes, so a fault
    // is shown to cost the field that asked for it and nothing else.
    fields.push(("resolved", "String"));
    definitions.push(("resolved", "$util.toJson($util.str.toUpper(\"still here\"))"));
    let api = create_api(&router, "faults", &fields, &definitions).await;

    let case = "every argument no helper can compute with, in one request";
    let (status, answered) = post_graphql(
        case,
        &api,
        &json!({"query": document(&fields), "variables": {"token": TOKEN}}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
    );
    assert_eq!(
        answered["data"]["resolved"].as_str(),
        Some("STILL HERE"),
        "case {case:?}: a sibling field still resolves, got {answered}"
    );
    let reported = answered["errors"].as_array().cloned().unwrap_or_default();
    for (case, field, _) in rows {
        assert_eq!(
            &answered["data"][*field],
            &Value::Null,
            "case {case:?}: a field whose helper could not compute resolves no value, got \
             {answered}"
        );
        let entries: Vec<&Value> = reported
            .iter()
            .filter(|entry| entry["path"][0].as_str() == Some(*field))
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly one error for the field, got {entries:?}"
        );
        assert!(
            entries[0]["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the error states what the helper could not compute with, got {:?}",
            entries[0]
        );
    }
}

/// What one filter row expects of the field it is attached to.
enum Expected {
    /// The expression DOCUMENT the field answers, once the `AWSJSON` string it is written
    /// out as is parsed: the form a template that reads a member of the answer sees.
    Document(Value),
    /// The document as JSON TEXT, which is what the helper itself answers — so a template
    /// that passes the answer through `$util.toJson` carries the ENCODED document, not the
    /// document.
    Encoded(Value),
    /// The message the classified failure states.
    Refused(String),
}

/// One row of the filter table: the response template it attaches to its field, and what the
/// field must answer.
struct Filter {
    case: &'static str,
    field: &'static str,
    response: String,
    expected: Expected,
}

#[tokio::test]
async fn a_filter_transformation_answers_its_expression_document_or_a_classified_failure() {
    let router = emulator();
    // The two tokens drive the same templates, so what changes between the runs is the
    // REQUEST's own argument — the variable part of an answer and of a failure's message.
    for token in [TOKEN, OTHER_TOKEN] {
        let rows = filters(token);
        let fields: Vec<(&str, &str)> = rows.iter().map(|row| (row.field, "AWSJSON")).collect();
        let definitions: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.field, row.response.as_str()))
            .collect();
        let api = create_api(&router, token, &fields, &definitions).await;

        let case = "every filter of the table in one request";
        let (status, answered) = post_graphql(
            case,
            &api,
            &json!({"query": document(&fields), "variables": {"token": token}}),
            &[("x-api-key", api.api_key.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} for {token:?}: a GraphQL request is answered 200, got {status} and \
             {answered}"
        );
        let reported = answered["errors"].as_array().cloned().unwrap_or_default();
        for row in &rows {
            let case = row.case;
            let value = &answered["data"][row.field];
            let entries: Vec<&Value> = reported
                .iter()
                .filter(|entry| entry["path"][0].as_str() == Some(row.field))
                .collect();
            match &row.expected {
                Expected::Document(document) => {
                    assert!(
                        entries.is_empty(),
                        "case {case:?} for {token:?}: a filter this helper builds reports no \
                         error, got {entries:?}"
                    );
                    let built: Value = value
                        .as_str()
                        .and_then(|text| serde_json::from_str(text).ok())
                        .unwrap_or(Value::Null);
                    assert_eq!(
                        &built, document,
                        "case {case:?} for {token:?}: expected {document}, got {value}"
                    );
                }
                Expected::Encoded(document) => {
                    assert!(
                        entries.is_empty(),
                        "case {case:?} for {token:?}: a filter this helper builds reports no \
                         error, got {entries:?}"
                    );
                    // The field carries the ENCODED document: the `AWSJSON` string holds the
                    // JSON text of a JSON STRING, whose content is the document.
                    let expected = Value::String(Value::String(document.to_string()).to_string());
                    assert_eq!(
                        value, &expected,
                        "case {case:?} for {token:?}: expected {expected}, got {value}"
                    );
                }
                Expected::Refused(message) => {
                    assert_eq!(
                        value,
                        &Value::Null,
                        "case {case:?} for {token:?}: a refused transformation resolves no \
                         value, got {value}"
                    );
                    assert_eq!(
                        entries.len(),
                        1,
                        "case {case:?} for {token:?}: expected exactly one error for the \
                         field, got {entries:?}"
                    );
                    let entry = entries[0];
                    assert_eq!(
                        (
                            entry["errorType"].as_str(),
                            entry["message"].as_str().unwrap_or_default()
                        ),
                        (Some(FILTER_ERROR_TYPE), message.as_str()),
                        "case {case:?} for {token:?}: expected the classified failure, got \
                         {entry}"
                    );
                }
            }
        }
    }
}

/// The filter table, over the argument `token` the request carries.
///
/// Source of the operators and of the document's shape: the transformation-helper reference's
/// own example, which states `{"title":{"contains":"Hello World"}}` as
/// `(contains(#title,:title_contains))` over `{"#title":"title"}` and
/// `{":title_contains":{"S":"Hello World"}}` — the attribute under `#<attribute>` and the
/// operand under `:<attribute>_<operator>`, with a second operand of the same comparison
/// taking the next free name.
///
/// A row's response template renders the helper's answer as it stands, which is the JSON text
/// of the document; the one `Encoded` row passes it through `$util.toJson` instead, so the
/// field carries the answer ENCODED — the difference that tells a helper answering TEXT apart
/// from one answering the document.
fn filters(token: &str) -> Vec<Filter> {
    let built = |filter: &str| format!("$util.transform.toDynamoDBFilterExpression({filter})");
    vec![
        Filter {
            case: "the documented containment over one attribute",
            field: "contained",
            response: built("{\"status\": {\"contains\": $context.arguments.token}}"),
            expected: Expected::Document(json!({
                "expression": "(contains(#status,:status_contains))",
                "expressionNames": {"#status": "status"},
                "expressionValues": {":status_contains": {"S": token}},
            })),
        },
        Filter {
            case: "an equality over the same attribute",
            field: "equal",
            response: built("{\"status\": {\"eq\": $context.arguments.token}}"),
            expected: Expected::Document(json!({
                "expression": "(#status = :status_eq)",
                "expressionNames": {"#status": "status"},
                "expressionValues": {":status_eq": {"S": token}},
            })),
        },
        Filter {
            case: "the same equality, carried through util.toJson",
            field: "encoded",
            response: format!(
                "$util.toJson({})",
                built("{\"status\": {\"eq\": $context.arguments.token}}")
            ),
            expected: Expected::Encoded(json!({
                "expression": "(#status = :status_eq)",
                "expressionNames": {"#status": "status"},
                "expressionValues": {":status_eq": {"S": token}},
            })),
        },
        Filter {
            case: "a range, whose two bounds each travel under a placeholder",
            field: "ranged",
            response: built("{\"votes\": {\"between\": [1, 9]}}"),
            expected: Expected::Document(json!({
                "expression": "(#votes BETWEEN :votes_between AND :votes_between_1)",
                "expressionNames": {"#votes": "votes"},
                "expressionValues": {
                    ":votes_between": {"N": 1},
                    ":votes_between_1": {"N": 9},
                },
            })),
        },
        Filter {
            case: "the existence question, which is a flag rather than an operand",
            field: "existing",
            response: built("{\"author\": {\"attributeExists\": true}}"),
            // A comparison carrying no operand at all parameterises no value, so the document
            // states no `expressionValues` member.
            expected: Expected::Document(json!({
                "expression": "(attribute_exists(#author))",
                "expressionNames": {"#author": "author"},
            })),
        },
        Filter {
            case: "an or group over two filters",
            field: "grouped",
            response: built(
                "{\"or\": [{\"status\": {\"eq\": $context.arguments.token}}, \
                 {\"author\": {\"beginsWith\": \"a\"}}]}",
            ),
            expected: Expected::Document(json!({
                "expression": "((#status = :status_eq) OR (begins_with(#author,\
                               :author_beginsWith)))",
                "expressionNames": {"#status": "status", "#author": "author"},
                "expressionValues": {
                    ":status_eq": {"S": token},
                    ":author_beginsWith": {"S": "a"},
                },
            })),
        },
        Filter {
            case: "an operator the helper defines none of",
            field: "bogus",
            response: built("{\"status\": {\"bogusOperator\": $context.arguments.token}}"),
            expected: Expected::Refused(format!(
                "{FILTER_FAILED}{{status={{bogusOperator={token}}}}}"
            )),
        },
        Filter {
            case: "a filter comparing nothing at all",
            field: "empty",
            response: built("{}"),
            expected: Expected::Refused(format!("{FILTER_FAILED}{{}}")),
        },
        Filter {
            case: "a value that is no filter",
            field: "unmapped",
            response: built("\"every record\""),
            expected: Expected::Refused(format!("{FILTER_FAILED}every record")),
        },
    ]
}

/// The document a case POSTs: every field of the table, each selected with the request's own
/// token.
fn document(fields: &[(&str, &str)]) -> String {
    let selections: String = fields
        .iter()
        .map(|(field, _)| format!("  {field}(token: $token)\n"))
        .collect();
    format!("query Probe($token: String!) {{\n{selections}}}")
}

/// Create one GraphQL API with an API key, a schema declaring `fields`, a `NONE` data source
/// and one unit resolver per field, and read the address its GraphQL endpoint is served on.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    fields: &[(&str, &str)],
    definitions: &[(&str, &str)],
) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("util-library-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(api, "GRAPHQL");
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");

    let mut schema = String::from("schema { query: Query }\n\ntype Query {\n");
    for (field, graphql_type) in fields {
        schema.push_str(&format!("  {field}({TOKEN_ARGUMENT}): {graphql_type}\n"));
    }
    schema.push_str("}\n");
    install_schema(router, &api_id, &schema).await;

    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");

    for (field, response) in definitions {
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": REQUEST_TEMPLATE,
                "responseMappingTemplate": response,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {field} must answer 200, got {resolver}"
        );
    }

    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// The two arguments the cases drive their templates with, so what varies between two runs of
/// the same template is the request rather than the template.
const TOKEN: &str = "settled";
const OTHER_TOKEN: &str = "paused";

/// The fixed text a refused filter transformation states, and the class AppSync classifies it
/// under — the service's own wording, which the emulator's copies live beside the helper
/// (`src/service/graphql_api/mapping_template/transform.rs`).
const FILTER_FAILED: &str = "Error generating DynamoDB filter expression from input - ";
const FILTER_ERROR_TYPE: &str = "DynamoDBFilterError";
