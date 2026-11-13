// SPDX-License-Identifier: CC0-1.0

//! The `$util` conversions a mapping template may apply: the
//! `application/x-www-form-urlencoded` ENCODE and DECODE, and the JavaScript string ESCAPE.
//!
//! RULE: a mapping template is rendered by resolving its references against the call, and a
//! `$util` method whose conversion is stated EXACTLY is one of those references —
//! `$util.urlEncode(String)` "converts a string into 'application/x-www-form-urlencoded'
//! format", `$util.urlDecode(String)` "decodes an 'application/x-www-form-urlencoded' string
//! back into its original form" and `$util.escapeJavaScript()` "escapes the characters in a
//! string using JavaScript string rules" — so the text it produces is that format's or that
//! language's own encoding of the value, whatever the value carries. A `$util` method whose
//! output the documentation does not state is a reference this emulator resolves nothing for,
//! so the template is not rendered and the stage answers the api's
//! `API_CONFIGURATION_ERROR` gateway response rather than a body built from a guess.
//!
//! This is the half of the rule that needs no other service: the template under test is an
//! integration REQUEST template of a `MOCK`, whose rendering IS what the integration answers
//! ("for testing without actually invoking the backend", `IntegrationType`), and the
//! integration response echoes that answer. The same rendering is what an `AWS` integration
//! to a QUERY api sends as its request, which the cross-service suite
//! `core/router/tests/all/apigateway_sqs_service_integration.rs` pins.
//!
//! The rows vary what the rule generalises over: WHICH conversion is applied, and the value
//! it is applied to — one needing no escaping, one carrying the characters that are
//! significant in that format (`&`, `=`, `+`, `%`, a space) or in a string literal (a quote,
//! a backslash), one carrying characters outside ASCII, the empty value, and a round trip of
//! the two form conversions.

use serde_json::json;

use crate::execute_api_data_plane::{
    deploy, integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};
use crate::stage_request_validation::{invoke, JSON, STAGE};

/// One row: the template under test and the body the stage must answer for the value the
/// request carries in its query string.
struct Row {
    case: &'static str,
    /// The integration REQUEST template, whose rendering a `MOCK` answers with.
    template: &'static str,
    /// The value the request carries as `?v=…`, already percent-encoded for the query
    /// string so the stage reads the value the row means.
    sent: &'static str,
    /// The text the stage must answer, or `None` for a template this emulator does not
    /// render — which the api answers as a configuration error.
    expected: Option<&'static str>,
}

/// The status API Gateway answers an integration it could not perform with.
const CONFIGURATION_ERROR_STATUS: u16 = 500;

/// The rows the table below drives.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an encode of a value needing no escaping",
            template: "$util.urlEncode($input.params('v'))",
            sent: "plain",
            expected: Some("plain"),
        },
        Row {
            case: "an encode of a value carrying a space",
            template: "$util.urlEncode($input.params('v'))",
            sent: "two%20words",
            expected: Some("two+words"),
        },
        Row {
            case: "an encode of the characters that are significant in the format",
            template: "$util.urlEncode($input.params('v'))",
            sent: "a%26b%3Dc%2Bd%25e",
            expected: Some("a%26b%3Dc%2Bd%25e"),
        },
        Row {
            case: "an encode of characters outside ASCII",
            template: "$util.urlEncode($input.params('v'))",
            sent: "caf%C3%A9",
            expected: Some("caf%C3%A9"),
        },
        Row {
            case: "an encode of a value the request carries nothing for",
            template: "$util.urlEncode($input.params('absent'))",
            sent: "plain",
            expected: Some(""),
        },
        Row {
            case: "a decode of escapes and of the format's own space",
            template: "$util.urlDecode($input.params('v'))",
            // The query string carries the escaped percent, so the value the template
            // decodes is the text `a%20b+c`.
            sent: "a%2520b%2Bc",
            expected: Some("a b c"),
        },
        Row {
            case: "a decode of the significant characters, escaped",
            template: "$util.urlDecode($input.params('v'))",
            sent: "a%2526b%253Dc",
            expected: Some("a&b=c"),
        },
        Row {
            case: "a round trip of the two conversions, which is the value itself",
            template: "$util.urlDecode($util.urlEncode($input.params('v')))",
            sent: "a%26b%3Dc%20d",
            expected: Some("a&b=c d"),
        },
        Row {
            case: "a literal beside a conversion, which renders as written",
            template: "MessageBody=$util.urlEncode($input.params('v'))",
            sent: "a%26b",
            expected: Some("MessageBody=a%26b"),
        },
        Row {
            case: "the JavaScript escape of a value a quoted string can carry as it stands",
            template: "$util.escapeJavaScript($input.params('v'))",
            sent: "plain",
            expected: Some("plain"),
        },
        Row {
            case: "the JavaScript escape of the characters a string literal cannot carry",
            template: "$util.escapeJavaScript($input.params('v'))",
            // The query string carries the escaped quote and backslash, so the value the
            // template escapes is the text `a"b\c`.
            sent: "a%22b%5Cc",
            expected: Some(r#"a\"b\\c"#),
        },
        Row {
            case: "a $util method whose output the documentation does not state",
            template: "$util.parseJson($input.params('v'))",
            sent: "plain",
            expected: None,
        },
        Row {
            case: "a $util method that does not exist at all",
            template: "$util.formEncode($input.params('v'))",
            sent: "plain",
            expected: None,
        },
    ]
}

/// The path part of the row at `index`: one resource per row, so each template is stood up
/// on a method of its own and one deployment serves them all.
fn route_of(index: usize) -> String {
    format!("u{index}")
}

#[tokio::test]
async fn a_util_conversion_renders_the_format_s_own_encoding_of_the_value() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "template-url-conversions").await;
    let rows = rows();
    for (index, row) in rows.iter().enumerate() {
        let resource = resource_under(&gateway, &api, &root, &route_of(index)).await;
        put_method(
            &gateway,
            &api,
            &resource,
            "POST",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(
            &gateway,
            &api,
            &resource,
            "POST",
            json!({ "type": "MOCK", "requestTemplates": { JSON: row.template } }),
        )
        .await;
        method_response(&gateway, &api, &resource, "POST", "200", json!({})).await;
        integration_response(
            &gateway,
            &api,
            &resource,
            "POST",
            "200",
            // The response template echoes what the integration answered, which for a
            // `MOCK` is the rendered request template above.
            json!({ "statusCode": "200", "responseTemplates": { JSON: "$input.body" } }),
        )
        .await;
    }
    deploy(&gateway, &api, STAGE).await;

    for (index, row) in rows.iter().enumerate() {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{}?v={}", route_of(index), row.sent),
            &[("content-type", JSON)],
            "{}",
        )
        .await;
        match row.expected {
            Some(expected) => {
                assert_eq!(
                    answered.status, 200,
                    "case {:?}: expected the integration to answer 200, got {} with {:?}",
                    row.case, answered.status, answered.body
                );
                assert_eq!(
                    answered.body, expected,
                    "case {:?}: expected the rendered body {expected:?}, got {:?}",
                    row.case, answered.body
                );
            }
            None => assert_eq!(
                answered.status, CONFIGURATION_ERROR_STATUS,
                "case {:?}: expected the api's own integration-failure response, got {} with \
                 {:?}",
                row.case, answered.status, answered.body
            ),
        }
    }
}
