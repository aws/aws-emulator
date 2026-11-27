// SPDX-License-Identifier: CC0-1.0

//! The `$util.dynamodb` MARSHALLING helpers of a mapping template, driven over the API's own
//! published endpoint.
//!
//! The rule these cases assert, one level above the recorded case (one PutItem template
//! inlining `$util.dynamodb.toDynamoDBJson($ctx.args.id)` into the request object it writes):
//!
//! > Each `$util.dynamodb` helper renders a value as the ATTRIBUTE VALUE DynamoDB holds it as
//! > — a one-member object whose name is the attribute type (`S`, `N`, `B`, `BOOL`, `NULL`,
//! > `L`, `M`, `SS`, `NS`, `BS`) and whose value is the marshalled content — with one typed
//! > helper per attribute type and a general converter that picks the type from the value it
//! > is handed, marshalling a list's elements and a map's members through itself; and every
//! > one of them has a `…Json` sibling answering the same representation already SERIALIZED,
//! > which is what lets a template inline it into the request object it writes.
//!
//! The rows span what the rule generalises over: the KIND of value the general converter is
//! handed (every JSON type, an empty and a nested collection among them), the attribute TYPE
//! each typed helper names, and the two forms every helper has. The two helpers that are not
//! conversions of one value — the S3 object reference, written and read back — have their own
//! case, as has the argument a helper cannot convert at all.
//!
//! Every case is driven as a client drives it: the API's schema declares one field per helper,
//! each resolved by a mapping template that marshals the field's own argument, and the
//! document is POSTed to the socket the API publishes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{
    emulator, install_schema, post_graphql, GraphqlApi,
};

/// Every helper of the family that CONVERTS one value, as the AppSync resolver
/// mapping-template utility reference lists its DynamoDB helpers. `$util` is no modelled
/// shape, so the published reference is the source; each of these has the serialized sibling
/// named by [`JSON_SIBLING`], and the two helpers that are not conversions of one value
/// ([`TO_S3_OBJECT`], [`FROM_S3_OBJECT`]) are named beside them.
const MARSHALLERS: &[&str] = &[
    "toDynamoDB",
    "toString",
    "toStringSet",
    "toNumber",
    "toNumberSet",
    "toBinary",
    "toBinarySet",
    "toBoolean",
    "toNull",
    "toList",
    "toMap",
    "toMapValues",
];

/// How the SERIALIZED sibling of every helper above is named.
const JSON_SIBLING: &str = "Json";

/// The two helpers that compose and read an S3 object reference rather than converting one
/// value. `fromS3ObjectJson` is the one name ending in [`JSON_SIBLING`] that is no sibling.
const TO_S3_OBJECT: &str = "toS3Object";
const FROM_S3_OBJECT: &str = "fromS3ObjectJson";

/// The `version` a local resolver's request object states, as the documented local-resolver
/// template does.
const REQUEST_VERSION: &str = "2017-02-28";

/// The name of the `NONE` data source every resolver of this suite resolves against: "There is
/// no data source", so the request half's own `payload` is the result and what a case observes
/// is exactly what the request template RENDERED.
const DATA_SOURCE: &str = "local";

/// One logical case: the helper it drives, the arguments the field is given and the attribute
/// value the helper has to render them as.
struct Case<'a> {
    case: &'a str,
    helper: &'a str,
    variables: Value,
    expected: Value,
}

/// The cases the general converter is asserted over: one per kind of value JSON carries, the
/// empty and nested collections among them.
fn general_cases() -> Vec<Case<'static>> {
    let general = |case: &'static str, value: &str, expected: Value| Case {
        case,
        helper: "toDynamoDB",
        variables: json!({"value": value}),
        expected,
    };
    vec![
        general("a string", "\"widget\"", json!({"S": "widget"})),
        general("an empty string", "\"\"", json!({"S": ""})),
        general("a positive integer", "7", json!({"N": 7})),
        general("a negative integer", "-12", json!({"N": -12})),
        general("a fraction", "1.5", json!({"N": 1.5})),
        general("a true boolean", "true", json!({"BOOL": true})),
        general("a false boolean", "false", json!({"BOOL": false})),
        general("a null", "null", json!({"NULL": Value::Null})),
        general(
            "a list of scalars",
            "[1, \"two\", false]",
            json!({"L": [{"N": 1}, {"S": "two"}, {"BOOL": false}]}),
        ),
        general("an empty list", "[]", json!({"L": []})),
        general(
            "a map of scalars",
            "{\"id\": \"a\", \"count\": 2}",
            json!({"M": {"id": {"S": "a"}, "count": {"N": 2}}}),
        ),
        general("an empty map", "{}", json!({"M": {}})),
        general(
            "a nested structure",
            "{\"items\": [{\"kind\": null}]}",
            json!({"M": {"items": {"L": [{"M": {"kind": {"NULL": Value::Null}}}]}}}),
        ),
    ]
}

/// The cases the TYPED helpers are asserted over: one per attribute type each names, with the
/// argument forms a template hands them.
fn typed_cases() -> Vec<Case<'static>> {
    let typed = |case: &'static str, helper: &'static str, value: &str, expected: Value| Case {
        case,
        helper,
        variables: json!({"value": value}),
        expected,
    };
    vec![
        typed("a string", "toString", "\"widget\"", json!({"S": "widget"})),
        // A value that is no string is rendered as the text the language writes it as.
        typed("a number as a string", "toString", "42", json!({"S": "42"})),
        typed(
            "a string set",
            "toStringSet",
            "[\"a\", \"b\"]",
            json!({"SS": ["a", "b"]}),
        ),
        typed("a number", "toNumber", "42", json!({"N": 42})),
        typed("a number read from text", "toNumber", "\"42\"", json!({"N": 42})),
        typed(
            "a number set",
            "toNumberSet",
            "[1, 2.5]",
            json!({"NS": [1, 2.5]}),
        ),
        typed(
            "binary data",
            "toBinary",
            "\"dGhlIGJ5dGVz\"",
            json!({"B": "dGhlIGJ5dGVz"}),
        ),
        typed(
            "a binary set",
            "toBinarySet",
            "[\"YQ==\", \"Yg==\"]",
            json!({"BS": ["YQ==", "Yg=="]}),
        ),
        typed("a true boolean", "toBoolean", "true", json!({"BOOL": true})),
        typed("a false boolean", "toBoolean", "false", json!({"BOOL": false})),
        Case {
            case: "a null, which the helper takes no argument for",
            helper: "toNull",
            variables: json!({}),
            expected: json!({"NULL": Value::Null}),
        },
        typed(
            "a list",
            "toList",
            "[1, \"a\"]",
            json!({"L": [{"N": 1}, {"S": "a"}]}),
        ),
        typed("a map", "toMap", "{\"a\": 1}", json!({"M": {"a": {"N": 1}}})),
        // `toMapValues` answers the map ITSELF with every value converted, which is what an
        // `attributeValues` member of a request object is.
        typed(
            "the values of a map",
            "toMapValues",
            "{\"a\": 1, \"b\": \"x\"}",
            json!({"a": {"N": 1}, "b": {"S": "x"}}),
        ),
    ]
}

#[tokio::test]
async fn every_helper_renders_the_attribute_value_dynamodb_holds_its_argument_as() {
    let router = emulator();
    let api = create_api(&router, "marshalling").await;

    for case in general_cases().into_iter().chain(typed_cases()) {
        // Both forms of the helper, over the same argument: the representation, and the
        // sibling that answers the same representation serialized.
        for serialized in [false, true] {
            let field = if serialized {
                format!("{}{JSON_SIBLING}", case.helper)
            } else {
                case.helper.to_string()
            };
            let label = format!("{} through {field}", case.case);
            let produced = marshalled(&api, &label, &field, &case.variables).await;
            if serialized {
                let inlined = produced.as_str().unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: the serialized sibling answers the representation as \
                         text, got {produced}"
                    )
                });
                let parsed: Value = serde_json::from_str(inlined).unwrap_or(Value::Null);
                assert_eq!(
                    parsed, case.expected,
                    "case {label:?}: expected {} serialized, got {inlined:?}",
                    case.expected
                );
            } else {
                assert_eq!(
                    produced, case.expected,
                    "case {label:?}: expected {}, got {produced}",
                    case.expected
                );
            }
        }
    }
}

#[tokio::test]
async fn an_s3_object_reference_is_composed_and_read_back_from_what_dynamodb_holds() {
    let router = emulator();
    let api = create_api(&router, "s3-objects").await;

    // The reference is held as TEXT under a string attribute, so each row's expectation is the
    // document the helper composes, serialized the one way this tree serializes a document.
    struct Row<'a> {
        case: &'a str,
        variables: Value,
        held: Value,
    }
    let rows: &[Row] = &[
        Row {
            case: "a reference naming an object version",
            variables: json!({
                "key": "items/one.json",
                "bucket": "the-bucket",
                "region": "us-west-2",
                "version": "3",
            }),
            held: json!({"s3": {
                "key": "items/one.json",
                "bucket": "the-bucket",
                "region": "us-west-2",
                "version": "3",
            }}),
        },
        Row {
            case: "a reference naming no version, which the composed document leaves out",
            variables: json!({
                "key": "items/two.json",
                "bucket": "the-other-bucket",
                "region": "eu-west-1",
            }),
            held: json!({"s3": {
                "key": "items/two.json",
                "bucket": "the-other-bucket",
                "region": "eu-west-1",
            }}),
        },
    ];
    for row in rows {
        let case = row.case;
        let composed = marshalled(&api, case, TO_S3_OBJECT, &row.variables).await;
        assert_eq!(
            composed,
            json!({"S": row.held.to_string()}),
            "case {case:?}: expected the reference held as text under a string attribute, got \
             {composed}"
        );

        // And the helper that READS one back answers the members the reference carries: the
        // round trip is the invariant, so what was composed is what is read.
        let text = composed["S"].as_str().unwrap_or_default().to_string();
        let read = marshalled(&api, case, FROM_S3_OBJECT, &json!({"value": text})).await;
        assert_eq!(
            read, row.held["s3"],
            "case {case:?}: expected the members the composed reference carries, got {read}"
        );
    }
}

#[tokio::test]
async fn an_argument_a_helper_cannot_convert_leaves_the_field_unresolved() {
    let router = emulator();
    let api = create_api(&router, "refusals").await;

    // Each row hands a typed helper a value of a kind it cannot marshal: the field is reported
    // as one that did not resolve rather than answered with something nothing converted.
    let rows: &[(&str, &str, &str)] = &[
        ("a list helper over a string", "toList", "\"not a list\""),
        ("a map helper over a list", "toMap", "[1, 2]"),
        ("a set helper over a map", "toStringSet", "{\"a\": 1}"),
        ("a number helper over a word", "toNumber", "\"widget\""),
    ];
    for (case, helper, value) in rows {
        let (status, answered) = post_graphql(
            case,
            &api,
            &document(helper, &json!({"value": value})),
            &[("x-api-key", api.api_key.as_str())],
        )
        .await;
        assert_eq!(
            (status, answered["data"][helper].as_str()),
            (200, None),
            "case {case:?}: a field whose template cannot marshal its argument resolves to \
             nothing, got {status} and {answered}"
        );
        let reported = answered["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the field to be reported as unresolved, got {answered}"
        );
        assert!(
            !reported[0]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: the reported error states what went wrong, got {answered}"
        );
    }
}

/// Drive one field of the suite's API and answer the JSON document its template rendered.
///
/// The field answers the rendered representation as TEXT, so what a case asserts on is the
/// document that text parses to.
async fn marshalled(api: &GraphqlApi, case: &str, field: &str, variables: &Value) -> Value {
    let (status, answered) = post_graphql(
        case,
        api,
        &document(field, variables),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        (status, answered["errors"].as_array()),
        (200, None),
        "case {case:?}: a marshalling template resolves the field it is written for, got \
         {status} and {answered}"
    );
    let text = answered["data"][field]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the rendered text, got {answered}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON document, got {text:?}"))
}

/// The document a client POSTs to select `field`, with one variable per argument the case
/// states.
fn document(field: &str, variables: &Value) -> Value {
    let names: Vec<String> = variables
        .as_object()
        .map(|given| given.keys().cloned().collect())
        .unwrap_or_default();
    // A field the case gives no argument is selected bare: a variable list and an argument
    // list are both written only where there is one.
    if names.is_empty() {
        return json!({"query": format!("query Marshal {{ {field} }}")});
    }
    let declared: Vec<String> = names.iter().map(|name| format!("${name}: String")).collect();
    let passed: Vec<String> = names
        .iter()
        .map(|name| format!("{name}: ${name}"))
        .collect();
    json!({
        "query": format!(
            "query Marshal({}) {{ {field}({}) }}",
            declared.join(", "),
            passed.join(", "),
        ),
        "variables": variables.clone(),
    })
}

/// Every field this suite's schema declares: one per helper form, named after the helper it
/// drives.
fn fields() -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    for helper in MARSHALLERS {
        fields.push((*helper).to_string());
        fields.push(format!("{helper}{JSON_SIBLING}"));
    }
    fields.push(TO_S3_OBJECT.to_string());
    fields.push(format!("{TO_S3_OBJECT}{JSON_SIBLING}"));
    fields.push(FROM_S3_OBJECT.to_string());
    fields
}

/// The arguments a field takes: the reference a helper composes is given its parts, and every
/// other field the one value it marshals.
fn arguments(field: &str) -> &'static [&'static str] {
    if field.starts_with(TO_S3_OBJECT) {
        &["key", "bucket", "region", "version"]
    } else {
        &["value"]
    }
}

/// The schema this suite's API is created with: one field per helper form, each taking the
/// arguments that helper is given and answering the rendered representation as text.
fn schema() -> String {
    let declared: String = fields()
        .iter()
        .map(|field| {
            let arguments: Vec<String> = arguments(field)
                .iter()
                .map(|name| format!("{name}: String"))
                .collect();
            format!("  {field}({}): String\n", arguments.join(", "))
        })
        .collect();
    format!("schema {{ query: Query }}\n\ntype Query {{\n{declared}}}\n")
}

/// The request template of the resolver on `field`: it marshals the field's own argument with
/// the helper the field is named after and renders what that came to as text, so the value a
/// case reads back is exactly what the helper produced.
fn request_template(field: &str) -> String {
    let given = if field.starts_with(TO_S3_OBJECT) {
        "$ctx.args.key, $ctx.args.bucket, $ctx.args.region, $ctx.args.version".to_string()
    } else if field == FROM_S3_OBJECT {
        "$ctx.args.value".to_string()
    } else if field.starts_with("toNull") {
        String::new()
    } else {
        "$parsed".to_string()
    };
    // Only a helper that CONVERTS a value reads one: the argument arrives as the JSON text of
    // the value, so the case can hand one field a value of any kind.
    let read = if given == "$parsed" {
        "#set($parsed = $util.parseJson($ctx.args.value))\n"
    } else {
        ""
    };
    format!(
        "{read}#set($marshalled = $util.dynamodb.{field}({given}))\n\
         #set($text = $util.toJson($marshalled))\n\
         {{\"version\": \"{REQUEST_VERSION}\", \"payload\": {{\"out\": $util.toJson($text)}}}}"
    )
}

/// Create one GraphQL API with an API key, [`schema`], a `NONE` data source and one resolver
/// per field, and read the address its GraphQL endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("dynamodb-marshalling-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, &schema()).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    for field in fields() {
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/Query/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": request_template(&field),
                "responseMappingTemplate": "$util.toJson($ctx.result.out)",
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
