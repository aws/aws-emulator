// SPDX-License-Identifier: CC0-1.0

//! `TestFunction`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for a test:
//!
//! * "CloudFront runs the function, passing it the event object that you provided, and
//!   returns the function's result (the modified event object) in the response. The response
//!   also contains function logs and error messages, if any exist." So the result is
//!   whatever the STORED code did with the submitted event object: the event object it
//!   returned (reported under `response` when it carries `statusCode`, under `request`
//!   otherwise), the lines it logged in the order it logged them, and — when it failed —
//!   the error message instead of an output;
//! * the code that runs is the one the STAGE the request names holds: a request naming no
//!   stage tests `DEVELOPMENT`, `LIVE` runs what a publish copied there, and a stage no
//!   stored function has is "The function does not exist" (`NoSuchFunctionExists`, HTTP 404);
//! * `IfMatch` is "The current version (ETag value) of the function that you are testing",
//!   checked against the version of the stage being tested, and this operation models no
//!   failed-precondition error: a header that is no version token at all and one carrying a
//!   token that is not that stage's are both `InvalidIfMatchVersion` (HTTP 400);
//! * an event object that is no JSON object describes no event a function can be passed, so
//!   it is refused with `InvalidArgument` (HTTP 400) and nothing runs;
//! * a test REPORTS, so it persists nothing: the function, its version tokens and its code
//!   stand exactly as they did before it.
//!
//! The table rows differ along everything the first rule generalises over — what the code
//! DOES (rewrites the request it was handed, generates a response, logs, computes from the
//! event, returns a promise, raises, returns something that is no event object, declares no
//! handler, does not compile, never returns) and which event object it is handed (a
//! viewer-request event and a viewer-response one) — because the result is a property of the
//! run rather than of any one script.

use ::serde_json::Value as Json;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_LOG_ENTRIES, MAX_LOG_ENTRY_BYTES, MAX_OUTPUT_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::function_lifecycle::{base64, create_function, describe_function, get_function, Create};
use crate::function_publish::publish_function;
use crate::function_update::{update_function, UPDATE_ETAG_HEADER};

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no function in a fresh emulator holds, so a precondition carrying it is stale
/// rather than malformed.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

/// A viewer-request event object, in the shape CloudFront documents for one: the version, the
/// context, the viewer and the `request` the function may modify.
pub(crate) const VIEWER_REQUEST_EVENT: &str = r#"{"version":"1.0",
     "context":{"distributionDomainName":"d111111abcdef8.cloudfront.net",
     "distributionId":"EDFDVBD6EXAMPLE","eventType":"viewer-request","requestId":"r-1"},
     "viewer":{"ip":"198.51.100.11"},
     "request":{"method":"GET","uri":"/old","querystring":{"page":{"value":"2"}},
     "headers":{"host":{"value":"example.com"}},"cookies":{}}}"#;

/// A viewer-response event object, the other half of the pair: it carries a `response` rather
/// than a request, so a function handed one works on the response.
pub(crate) const VIEWER_RESPONSE_EVENT: &str = r#"{"version":"1.0",
     "context":{"distributionDomainName":"d111111abcdef8.cloudfront.net",
     "distributionId":"EDFDVBD6EXAMPLE","eventType":"viewer-response","requestId":"r-2"},
     "viewer":{"ip":"198.51.100.11"},
     "request":{"method":"GET","uri":"/index.html","querystring":{},
     "headers":{"host":{"value":"example.com"}},"cookies":{}},
     "response":{"statusCode":200,"statusDescription":"OK",
     "headers":{"content-type":{"value":"text/html"}},"cookies":{}}}"#;

// ---------------------------------------------------------------------------
// Calling the test
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/function/{Name}/test`, carrying the event object as the base64 blob
/// member a restXml request puts it in, the stage when the request names one, and `if_match`
/// as the `If-Match` header the operation binds its required `IfMatch` member to.
pub(crate) async fn test_function(
    router: &MultiServiceRouter,
    name: &str,
    stage: Option<&str>,
    event: &[u8],
    if_match: &str,
) -> Answer {
    let stage_element = match stage {
        Some(stage) => format!("<Stage>{stage}</Stage>"),
        None => String::new(),
    };
    let body = format!(
        "<TestFunctionRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         {stage_element}<EventObject>{}</EventObject></TestFunctionRequest>",
        base64(event)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/function/{name}/test"),
        Some(&body),
        Some(if_match),
    )
    .await
}

/// Create the function `name` with `code` and answer the version token of its `DEVELOPMENT`
/// stage, which is the version a test of that stage is checked against.
async fn create_one(router: &MultiServiceRouter, name: &str, code: &str) -> String {
    let create = function(name, code);
    let answer = create_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "the create of {name} must answer HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    answer.header("etag").to_string()
}

/// One `CreateFunction` request for the function `name` carrying `code`.
fn function(name: &str, code: &str) -> Create {
    Create {
        case: "a function under test",
        name: name.to_string(),
        comment: "runs under test".to_string(),
        runtime: "cloudfront-js-2.0",
        code: code.as_bytes().to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    }
}

/// The text of one member of the `TestResult` document, or `None` when it carries none.
fn result_text<'a>(result: &'a Node, name: &str) -> Option<&'a str> {
    result.text_at(name)
}

/// The log lines the `TestResult` document reports, in the order they appear in it.
fn result_logs(result: &Node) -> Vec<String> {
    result
        .at("FunctionExecutionLogs")
        .map(|logs| {
            logs.children
                .iter()
                .map(|line| line.text().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `text` as JSON, or a panic naming the case: the output member carries the JSON of the
/// event object the function returned, so it is compared as a document rather than as text.
fn json(text: &str, case: &str) -> Json {
    ::serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case}: expected JSON, got {text:?} ({error})"))
}

// ---------------------------------------------------------------------------
// The runs under test
// ---------------------------------------------------------------------------

/// One run: the code a function was stored with, the event object it is handed, and what the
/// result must report.
struct Run {
    case: &'static str,
    code: &'static str,
    /// The event object the request carries, as the JSON text a client submits.
    event: &'static str,
    /// The JSON the `FunctionOutput` member must carry, when the run returns one.
    output: Option<&'static str>,
    /// The log lines the result must report, in order.
    logs: Vec<&'static str>,
    /// A fragment the reported error message must contain, or `Some("")` for a run that must
    /// report SOME error message whose wording is the engine's own. `None` where the run
    /// must report no error at all.
    error: Option<&'static str>,
}

/// Every rule below is asserted over these runs.
fn run_table() -> Vec<Run> {
    vec![
        Run {
            case: "the request it was handed, with the URI and a header rewritten",
            code: "function handler(event) {\n\
                   \x20   var request = event.request;\n\
                   \x20   if (request.uri === '/old' && request.method === 'GET') {\n\
                   \x20       request.uri = '/new';\n\
                   \x20   }\n\
                   \x20   request.headers['x-rewritten'] = { value: '1' };\n\
                   \x20   return request;\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: Some(
                r#"{"request":{"method":"GET","uri":"/new","querystring":{"page":{"value":"2"}},
                 "headers":{"host":{"value":"example.com"},"x-rewritten":{"value":"1"}},
                 "cookies":{}}}"#,
            ),
            logs: Vec::new(),
            error: None,
        },
        Run {
            case: "a generated response, which carries a status code",
            code: "function handler(event) {\n\
                   \x20   return {\n\
                   \x20       statusCode: 302,\n\
                   \x20       statusDescription: 'Found',\n\
                   \x20       headers: { location: { value: '/index.html' } }\n\
                   \x20   };\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: Some(
                r#"{"response":{"statusCode":302,"statusDescription":"Found",
                 "headers":{"location":{"value":"/index.html"}}}}"#,
            ),
            logs: Vec::new(),
            error: None,
        },
        Run {
            case: "the response of a viewer-response event, with a header added",
            code: "function handler(event) {\n\
                   \x20   var response = event.response;\n\
                   \x20   response.headers['x-cache'] = { value: 'hit' };\n\
                   \x20   return response;\n\
                   }\n",
            event: VIEWER_RESPONSE_EVENT,
            output: Some(
                r#"{"response":{"statusCode":200,"statusDescription":"OK",
                 "headers":{"content-type":{"value":"text/html"},"x-cache":{"value":"hit"}},
                 "cookies":{}}}"#,
            ),
            logs: Vec::new(),
            error: None,
        },
        Run {
            case: "a value computed from the event object",
            code: "function handler(event) {\n\
                   \x20   var request = event.request;\n\
                   \x20   var names = Object.keys(request.querystring).sort();\n\
                   \x20   request.uri = '/q/' + names.length + '/' + names.join('-');\n\
                   \x20   return request;\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: Some(
                r#"{"request":{"method":"GET","uri":"/q/1/page","querystring":{"page":{"value":"2"}},
                 "headers":{"host":{"value":"example.com"}},"cookies":{}}}"#,
            ),
            logs: Vec::new(),
            error: None,
        },
        Run {
            case: "a value computed over a loop, which the step budget leaves room for",
            code: "function handler(event) {\n\
                   \x20   var request = event.request;\n\
                   \x20   var total = 0;\n\
                   \x20   for (var index = 0; index < 10000; index++) { total += index; }\n\
                   \x20   request.uri = '/sum/' + total;\n\
                   \x20   return request;\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: Some(
                r#"{"request":{"method":"GET","uri":"/sum/49995000","querystring":{"page":{"value":"2"}},
                 "headers":{"host":{"value":"example.com"}},"cookies":{}}}"#,
            ),
            logs: Vec::new(),
            error: None,
        },
        Run {
            case: "lines written through the console, in the order they were written",
            code: "function handler(event) {\n\
                   \x20   console.log('uri', event.request.uri);\n\
                   \x20   console.log(event.request.querystring);\n\
                   \x20   console.error('done');\n\
                   \x20   return event.request;\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: Some(
                r#"{"request":{"method":"GET","uri":"/old","querystring":{"page":{"value":"2"}},
                 "headers":{"host":{"value":"example.com"}},"cookies":{}}}"#,
            ),
            logs: vec!["uri /old", "{\"page\":{\"value\":\"2\"}}", "done"],
            error: None,
        },
        Run {
            case: "an async handler, whose promise settles with the request",
            code: "async function handler(event) {\n\
                   \x20   var request = event.request;\n\
                   \x20   request.uri = await Promise.resolve('/awaited');\n\
                   \x20   return request;\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: Some(
                r#"{"request":{"method":"GET","uri":"/awaited","querystring":{"page":{"value":"2"}},
                 "headers":{"host":{"value":"example.com"}},"cookies":{}}}"#,
            ),
            logs: Vec::new(),
            error: None,
        },
        Run {
            case: "a handler that raises, after logging",
            code: "function handler(event) {\n\
                   \x20   console.log('about to fail');\n\
                   \x20   throw new Error('no catalog for ' + event.request.uri);\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: None,
            logs: vec!["about to fail"],
            error: Some("no catalog for /old"),
        },
        Run {
            case: "a handler that returns something that is no event object",
            code: "function handler(event) { return 'rewritten'; }\n",
            event: VIEWER_REQUEST_EVENT,
            output: None,
            logs: Vec::new(),
            error: Some("event object"),
        },
        Run {
            case: "code that declares no handler",
            code: "var rewrite = function (event) { return event.request; };\n",
            event: VIEWER_REQUEST_EVENT,
            output: None,
            logs: Vec::new(),
            error: Some("handler"),
        },
        Run {
            case: "code that does not compile",
            code: "function handler(event) { return event.request; \n",
            event: VIEWER_REQUEST_EVENT,
            output: None,
            logs: Vec::new(),
            error: Some(""),
        },
        Run {
            case: "a handler that never returns, which a bound stops",
            code: "function handler(event) { while (true) { } }\n",
            event: VIEWER_REQUEST_EVENT,
            output: None,
            logs: Vec::new(),
            error: Some("stopped"),
        },
        Run {
            case: "a handler that recurses without end, which the stack ceiling stops",
            code: "function handler(event) {\n\
                   \x20   var deeper = function (depth) { return deeper(depth + 1); };\n\
                   \x20   return deeper(0);\n\
                   }\n",
            event: VIEWER_REQUEST_EVENT,
            output: None,
            logs: Vec::new(),
            error: Some(""),
        },
    ]
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The result is what the stored code DID with the submitted event object: the event object
/// it returned, under the member its shape names; the lines it logged, in order; and the
/// error message of a run that failed, with no output beside it. Every result reports a
/// compute utilization that is a whole-number percentage of the allowed execution time, and
/// the summary of the function that was tested.
#[tokio::test]
async fn a_test_reports_what_the_stored_code_did_with_the_event_object() {
    let router = emulator();

    for (index, run) in run_table().into_iter().enumerate() {
        let case = run.case;
        let name = format!("run_{index}");
        let etag = create_one(&router, &name, run.code).await;

        let answer = test_function(&router, &name, None, run.event.as_bytes(), &etag).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from the test, got {} with {}",
            answer.status, answer.raw
        );
        let result = answer.root();
        assert_eq!(
            result.name(),
            "TestResult",
            "case {case}: expected the TestResult payload, got {}",
            answer.raw
        );

        // "Contains configuration information and metadata about the CloudFront function
        // that was tested": the stage that ran, projected from the stored record.
        assert_eq!(
            result.text_at("FunctionSummary/Name"),
            Some(name.as_str()),
            "case {case}: expected the tested function's name in the summary, got {}",
            answer.raw
        );
        assert_eq!(
            result.text_at("FunctionSummary/FunctionMetadata/Stage"),
            Some(STAGE_DEVELOPMENT),
            "case {case}: expected the summary of the stage that ran, got {}",
            answer.raw
        );

        // "The amount of time that the function took to run as a percentage of the maximum
        // allowed time" — a percentage, so a whole number no greater than 100.
        let utilization = result_text(result, "ComputeUtilization")
            .unwrap_or_else(|| panic!("case {case}: the result carries no ComputeUtilization: {}", answer.raw));
        let utilization: u32 = utilization.parse().unwrap_or_else(|error| {
            panic!("case {case}: expected a whole-number percentage, got {utilization:?} ({error})")
        });
        assert!(
            utilization <= 100,
            "case {case}: expected a percentage of the allowed time, got {utilization}"
        );

        match run.output {
            Some(expected) => {
                let output = result_text(result, "FunctionOutput").unwrap_or_else(|| {
                    panic!("case {case}: the result carries no FunctionOutput: {}", answer.raw)
                });
                assert_eq!(
                    json(output, case),
                    json(expected, case),
                    "case {case}: expected the event object the function returned, got {output}"
                );
            }
            None => assert_eq!(
                result_text(result, "FunctionOutput").unwrap_or_default(),
                "",
                "case {case}: a failed run reports no event object, got {}",
                answer.raw
            ),
        }

        match run.error {
            Some(fragment) => {
                let message = result_text(result, "FunctionErrorMessage").unwrap_or_else(|| {
                    panic!("case {case}: the result carries no FunctionErrorMessage: {}", answer.raw)
                });
                assert!(
                    !message.is_empty() && message.contains(fragment),
                    "case {case}: expected an error message carrying {fragment:?}, got {message:?}"
                );
            }
            None => assert_eq!(
                result_text(result, "FunctionErrorMessage").unwrap_or_default(),
                "",
                "case {case}: a run that returned reports no error message, got {}",
                answer.raw
            ),
        }

        let logs = result_logs(result);
        assert_eq!(
            logs, run.logs,
            "case {case}: expected the lines the function logged, got {logs:?}"
        );
    }
}

/// The code that runs is the one the stage the request names holds. A request naming no stage
/// tests `DEVELOPMENT`; `LIVE` runs what the publish copied there, so an update of
/// `DEVELOPMENT` after a publish leaves the two stages running DIFFERENT code; and a stage no
/// stored function has — the `LIVE` stage of a function nothing published, or a name no
/// function holds — is "The function does not exist".
#[tokio::test]
async fn the_stage_the_request_names_decides_which_code_runs() {
    let router = emulator();
    let name = "staged_rewrite";
    let development = "function handler(event) {\n\
                       \x20   var request = event.request;\n\
                       \x20   request.uri = '/development';\n\
                       \x20   return request;\n\
                       }\n";
    let published = "function handler(event) {\n\
                     \x20   var request = event.request;\n\
                     \x20   request.uri = '/published';\n\
                     \x20   return request;\n\
                     }\n";
    let created_etag = create_one(&router, name, published).await;

    // The LIVE stage of a function nothing has published does not exist, so there is no code
    // to run there and no version to check against.
    let unpublished = test_function(
        &router,
        name,
        Some(STAGE_LIVE),
        VIEWER_REQUEST_EVENT.as_bytes(),
        &created_etag,
    )
    .await;
    assert_eq!(
        unpublished.status, 404,
        "the LIVE stage of an unpublished function must be not found, got {} with {}",
        unpublished.status, unpublished.raw
    );
    assert_eq!(
        unpublished.error_code(),
        "NoSuchFunctionExists",
        "expected NoSuchFunctionExists for an unpublished stage, got {}",
        unpublished.raw
    );

    let publish = publish_function(&router, name, &created_etag).await;
    assert_eq!(
        publish.status, 200,
        "the publish must answer HTTP 200, got {} with {}",
        publish.status, publish.raw
    );
    let live_etag = describe_function(&router, name, Some(STAGE_LIVE))
        .await
        .header("etag")
        .to_string();

    // The DEVELOPMENT stage is updated to other code, which the publish has not copied.
    let revised = function(name, development);
    let updated = update_function(&router, name, &revised, Some(&created_etag)).await;
    assert_eq!(
        updated.status, 200,
        "the update must answer HTTP 200, got {} with {}",
        updated.status, updated.raw
    );
    // `UpdateFunction` binds its version token to a header of its own, where the rest of the
    // function surface binds `ETag`.
    let development_etag = updated.header(UPDATE_ETAG_HEADER).to_string();

    for (case, stage, if_match, expected) in [
        (
            "no stage named",
            None,
            development_etag.as_str(),
            "/development",
        ),
        (
            "DEVELOPMENT named",
            Some(STAGE_DEVELOPMENT),
            development_etag.as_str(),
            "/development",
        ),
        (
            "LIVE named",
            Some(STAGE_LIVE),
            live_etag.as_str(),
            "/published",
        ),
    ] {
        let answer =
            test_function(&router, name, stage, VIEWER_REQUEST_EVENT.as_bytes(), if_match).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from the test, got {} with {}",
            answer.status, answer.raw
        );
        let output = result_text(answer.root(), "FunctionOutput")
            .unwrap_or_else(|| panic!("case {case}: the result carries no FunctionOutput: {}", answer.raw));
        let returned = json(output, case);
        let uri = returned
            .pointer("/request/uri")
            .and_then(Json::as_str)
            .unwrap_or_default();
        assert_eq!(
            uri, expected,
            "case {case}: expected the code of that stage to run, got {output}"
        );
        let reported_stage = answer
            .root()
            .text_at("FunctionSummary/FunctionMetadata/Stage")
            .unwrap_or_default();
        assert_eq!(
            reported_stage,
            stage.unwrap_or(STAGE_DEVELOPMENT),
            "case {case}: expected the summary of the stage that ran, got {}",
            answer.raw
        );
    }

    // A name no function holds has no stage to test either.
    let absent = test_function(
        &router,
        "no_such_function",
        None,
        VIEWER_REQUEST_EVENT.as_bytes(),
        &development_etag,
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "a name no function holds must be not found, got {} with {}",
        absent.status, absent.raw
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchFunctionExists",
        "expected NoSuchFunctionExists for a name no function holds, got {}",
        absent.raw
    );
}

/// The test only runs the version its caller read: the precondition is checked against the
/// version of the STAGE being tested, and this operation models no failed-precondition error,
/// so a header that is no version token at all and one carrying a version no stage of the
/// function stands at are both "missing or not valid". The current version of the stage
/// addressed runs, bare or quoted as an HTTP entity tag.
#[tokio::test]
async fn a_test_needs_the_current_version_of_the_stage_it_runs() {
    let router = emulator();
    let name = "precondition_rewrite";
    let code = "function handler(event) { return event.request; }\n";
    let development_etag = create_one(&router, name, code).await;
    publish_function(&router, name, &development_etag).await;
    // The version of the stage the publish wrote, read back from the describe of it rather
    // than assumed: each stage's own token is what a test of that stage is checked against.
    let live_etag = describe_function(&router, name, Some(STAGE_LIVE))
        .await
        .header("etag")
        .to_string();

    for (case, stage, if_match, refusal) in [
        ("the current version, bare", None, development_etag.clone(), None),
        (
            "the current version, quoted as an entity tag",
            None,
            format!("\"{development_etag}\""),
            None,
        ),
        ("the LIVE stage's own version", Some(STAGE_LIVE), live_etag.clone(), None),
        (
            "a header that is no version token at all",
            None,
            "not-a-version".to_string(),
            Some("InvalidIfMatchVersion"),
        ),
        (
            "an empty header",
            None,
            String::new(),
            Some("InvalidIfMatchVersion"),
        ),
        (
            "a well-formed token no function holds",
            None,
            UNHELD_VERSION_TOKEN.to_string(),
            Some("InvalidIfMatchVersion"),
        ),
        (
            "a well-formed token no function holds, against the LIVE stage",
            Some(STAGE_LIVE),
            UNHELD_VERSION_TOKEN.to_string(),
            Some("InvalidIfMatchVersion"),
        ),
    ] {
        let answer =
            test_function(&router, name, stage, VIEWER_REQUEST_EVENT.as_bytes(), &if_match).await;
        match refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the test to run, got {} with {}",
                    answer.status, answer.raw
                );
                assert!(
                    result_text(answer.root(), "FunctionOutput").is_some(),
                    "case {case}: a test that ran reports the event object, got {}",
                    answer.raw
                );
            }
            Some(code) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected HTTP 400, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case}: expected {code}, got {}",
                    answer.raw
                );
            }
        }
    }
}

/// "You provide an event object that represents an HTTP request or response": a payload that
/// is no JSON object describes no such event, so it is refused as an invalid argument and
/// nothing runs.
#[tokio::test]
async fn an_event_object_that_is_no_json_object_is_refused() {
    let router = emulator();
    let name = "event_object_shape";
    let etag = create_one(
        &router,
        name,
        "function handler(event) { return event.request; }\n",
    )
    .await;

    for (case, event) in [
        ("no bytes at all", &b""[..]),
        ("text that is not JSON", &b"not an event"[..]),
        ("a truncated document", &b"{\"request\":"[..]),
        ("a JSON array", &b"[{\"request\":{}}]"[..]),
        ("a JSON string", &b"\"request\""[..]),
        ("a JSON number", &b"42"[..]),
        ("a JSON null", &b"null"[..]),
        ("bytes that are not UTF-8", &[0x00, 0xff, 0xfe][..]),
    ] {
        let answer = test_function(&router, name, None, event, &etag).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }
}

/// A test REPORTS: it runs the code and persists nothing. The function's configuration, its
/// version tokens and the code each stage serves stand exactly as they did before, so a
/// second test of the same version runs and answers the same way.
#[tokio::test]
async fn a_test_persists_nothing() {
    let router = emulator();
    let name = "reported_only";
    let code = "function handler(event) {\n\
                \x20   var request = event.request;\n\
                \x20   request.uri = '/stable';\n\
                \x20   return request;\n\
                }\n";
    let etag = create_one(&router, name, code).await;

    let before = describe_function(&router, name, None).await;
    let before_code = get_function(&router, name, None).await;

    let first = test_function(&router, name, None, VIEWER_REQUEST_EVENT.as_bytes(), &etag).await;
    assert_eq!(
        first.status, 200,
        "the first test must answer HTTP 200, got {} with {}",
        first.status, first.raw
    );
    let first_output = result_text(first.root(), "FunctionOutput")
        .expect("the first test reports the event object")
        .to_string();

    let after = describe_function(&router, name, None).await;
    let after_code = get_function(&router, name, None).await;
    assert_eq!(
        after.header("etag"),
        before.header("etag"),
        "a test must not mint a version: {}",
        after.raw
    );
    assert_eq!(
        after.raw, before.raw,
        "a test must leave the stored function exactly as it was"
    );
    assert_eq!(
        after_code.body, before_code.body,
        "a test must leave the stored code exactly as it was"
    );

    // The version the first test used is therefore still the current one, and the second run
    // of the same code over the same event object answers with the same event object.
    let second = test_function(&router, name, None, VIEWER_REQUEST_EVENT.as_bytes(), &etag).await;
    assert_eq!(
        second.status, 200,
        "the second test must answer HTTP 200, got {} with {}",
        second.status, second.raw
    );
    let second_output = result_text(second.root(), "FunctionOutput")
        .expect("the second test reports the event object")
        .to_string();
    assert_eq!(
        json(&second_output, "a repeated test"),
        json(&first_output, "a repeated test"),
        "the same code over the same event object must answer the same event object"
    );
}

/// One run cannot make an unbounded answer out of a small request: the lines it logs are
/// held to the published count and length ceilings, and an event object past the published
/// output ceiling is reported as a run that failed rather than served.
#[tokio::test]
async fn one_run_cannot_report_more_than_the_ceilings_allow() {
    let router = emulator();

    // Twice the line ceiling, each line longer than the length ceiling.
    let chatty = format!(
        "function handler(event) {{\n\
         \x20   for (var index = 0; index < {}; index++) {{\n\
         \x20       console.log('line ' + index + ' ' + 'y'.repeat({}));\n\
         \x20   }}\n\
         \x20   return event.request;\n\
         }}\n",
        MAX_LOG_ENTRIES * 2,
        MAX_LOG_ENTRY_BYTES + 1_000
    );
    let etag = create_one(&router, "log_ceilings", &chatty).await;
    let answer = test_function(
        &router,
        "log_ceilings",
        None,
        VIEWER_REQUEST_EVENT.as_bytes(),
        &etag,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a run that logs more than the ceilings allow is still a run, got {} with {}",
        answer.status, answer.raw
    );
    let logs = result_logs(answer.root());
    assert_eq!(
        logs.len(),
        MAX_LOG_ENTRIES,
        "expected the reported lines to stop at {MAX_LOG_ENTRIES}, got {}",
        logs.len()
    );
    for (index, line) in logs.iter().enumerate() {
        assert!(
            line.len() <= MAX_LOG_ENTRY_BYTES,
            "line {index}: expected at most {MAX_LOG_ENTRY_BYTES} bytes, got {}",
            line.len()
        );
    }

    // An event object past the output ceiling: the run happened, so it is the run that is
    // reported as failed, with the ceiling named and no output beside it.
    let verbose = format!(
        "function handler(event) {{\n\
         \x20   var request = event.request;\n\
         \x20   request.headers['x-big'] = {{ value: 'x'.repeat({}) }};\n\
         \x20   return request;\n\
         }}\n",
        MAX_OUTPUT_BYTES + 1_000
    );
    let etag = create_one(&router, "output_ceiling", &verbose).await;
    let answer = test_function(
        &router,
        "output_ceiling",
        None,
        VIEWER_REQUEST_EVENT.as_bytes(),
        &etag,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a run that returned too large an event object is still a run, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        result_text(answer.root(), "FunctionOutput").unwrap_or_default(),
        "",
        "an event object past the ceiling must not be served: {}",
        answer.raw
    );
    let message = result_text(answer.root(), "FunctionErrorMessage").unwrap_or_default();
    assert!(
        message.contains(&MAX_OUTPUT_BYTES.to_string()),
        "expected the reported error to name the {MAX_OUTPUT_BYTES} byte ceiling, got \
         {message:?}"
    );
}
