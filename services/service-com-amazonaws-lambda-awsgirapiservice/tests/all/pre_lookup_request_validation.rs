// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the request validation Lambda performs BEFORE it looks anything up.
//!
//! The RULE asserted here, over a table of inputs for each operation it holds for: **an
//! operation whose subject is the FUNCTION ITSELF — the aliases it holds, the versions it
//! holds, its reserved concurrency, its asynchronous-invocation configurations — takes no
//! qualifier at all, and a paginated listing takes only a `Marker` it issued itself; either
//! value is judged as a REQUEST, so it is refused 400 `InvalidParameterValueException` with
//! that operation's fixed sentence whether or not the resource it names exists, and nothing
//! is mutated.** The same reading applies to the parameters of a create: a `CreateFunction`
//! AWS refuses leaves NO function behind, so the follow-up read answers 404 rather than
//! reporting a configuration that was never accepted.
//!
//! The rows differ along what the rule generalises over: the operation, the documented form
//! the identifier is written in (bare name, the `function:` prefix, a partial ARN, a full
//! ARN), the kind of qualifier (a published version, an alias, the unpublished version),
//! whether the function exists at all, the shape of the marker, and — for the create — where
//! the refused value sits relative to the published ceiling.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about the
//! wire behaviour (status, error code, message) rather than any handler's internals. The
//! harness (`Answer`, `json_call`, `create_function`) is the one `function_lifecycle.rs`
//! publishes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, zip_file, Answer,
    ACCOUNT, REGION, ROLE,
};

/// The error code every refusal in this suite carries, and its status.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";
const REFUSAL_STATUS: u16 = 400;

/// The function these suites hold, so a refusal cannot be mistaken for the answer about a
/// function that is not there.
const HELD: &str = "pre-lookup-probe";

/// A name no `CreateFunction` is made for.
const ABSENT: &str = "pre-lookup-ghost";

/// The sentence `ListAliases` refuses a qualified identifier with.
const LIST_ALIASES_SENTENCE: &str = "Should not specify a qualifier in list alias request.";

/// The sentence `ListVersionsByFunction` refuses a qualified identifier with.
const LIST_VERSIONS_SENTENCE: &str =
    "Should not specify a version number in list version request.";

/// The sentence `ListFunctionEventInvokeConfigs` refuses a qualified identifier with.
const EVENT_INVOKE_CONFIGS_SENTENCE: &str =
    "The function name must not include a version number or alias.";

/// The sentence every operation on the function's reserved concurrency refuses a qualified
/// identifier with.
const FUNCTION_ONLY_SENTENCE: &str = "This operation is permitted on Lambda functions only. \
                                      Aliases and versions do not support this operation. \
                                      Please specify either a function name or an unqualified \
                                      function ARN.";

/// The sentence every paginated listing refuses a `Marker` it did not issue with.
const INVALID_MARKER_SENTENCE: &str = "Invalid marker value from user.";

/// The `message` of a wire error, under either casing the JSON envelopes use.
fn message(answer: &Answer) -> Option<String> {
    answer.string("message").or_else(|| answer.string("Message"))
}

/// The body of an answer, for a failing assertion to report.
fn body_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// One request this suite drives: a method, a URI built from the identifier, and a body.
struct Call {
    method: &'static str,
    uri: String,
    body: Value,
}

/// The call each operation under test makes for one `FunctionName` reference.
fn call_for(operation: &str, reference: &str) -> Call {
    match operation {
        "ListAliases" => Call {
            method: "GET",
            uri: format!("/2015-03-31/functions/{reference}/aliases"),
            body: Value::Null,
        },
        "ListVersionsByFunction" => Call {
            method: "GET",
            uri: format!("/2015-03-31/functions/{reference}/versions"),
            body: Value::Null,
        },
        "ListFunctionEventInvokeConfigs" => Call {
            method: "GET",
            uri: format!("/2019-09-25/functions/{reference}/event-invoke-config/list"),
            body: Value::Null,
        },
        "ListFunctionUrlConfigs" => Call {
            method: "GET",
            uri: format!("/2021-10-31/functions/{reference}/urls"),
            body: Value::Null,
        },
        "GetFunctionConcurrency" => Call {
            method: "GET",
            uri: format!("/2019-09-30/functions/{reference}/concurrency"),
            body: Value::Null,
        },
        "PutFunctionConcurrency" => Call {
            method: "PUT",
            uri: format!("/2017-10-31/functions/{reference}/concurrency"),
            body: json!({ "ReservedConcurrentExecutions": 1 }),
        },
        "DeleteFunctionConcurrency" => Call {
            method: "DELETE",
            uri: format!("/2017-10-31/functions/{reference}/concurrency"),
            body: Value::Null,
        },
        other => panic!("the suite drives no operation named {other}"),
    }
}

/// Dispatch the call `operation` makes for `reference`.
async fn drive(router: &MultiServiceRouter, operation: &str, reference: &str) -> Answer {
    let call = call_for(operation, reference);
    json_call(router, call.method, &call.uri, &call.body).await
}

/// A router holding [`HELD`], created and with its activation already due.
async fn emulator_holding_a_function() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let created = create_function(
        &router,
        &json!({
            "FunctionName": HELD,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the function under test must be created, got {} {}",
        created.status,
        body_of(&created)
    );
    make_activation_due(&engine);
    (engine, router)
}

/// Every way one qualifier can be written into a `FunctionName`, for a function of `name`.
///
/// The forms are the ones the member documents ("Function name - my-function. Function ARN -
/// arn:aws:lambda:us-west-2:123456789012:function:my-function. Partial ARN -
/// 123456789012:function:my-function"), plus the bare `function:` prefix its pattern admits.
fn qualified_forms(name: &str, qualifier: &str) -> Vec<(String, String)> {
    vec![
        ("a bare name".to_string(), format!("{name}:{qualifier}")),
        (
            "the resource-type prefix form".to_string(),
            format!("function:{name}:{qualifier}"),
        ),
        (
            "a partial ARN".to_string(),
            format!("{ACCOUNT}:function:{name}:{qualifier}"),
        ),
        (
            "a function ARN".to_string(),
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}:{qualifier}"),
        ),
    ]
}

/// RULE: an operation whose `FunctionName` names the function itself refuses an identifier
/// carrying a qualifier with 400 `InvalidParameterValueException` and that operation's fixed
/// sentence — before any lookup, so a function this store does not hold is refused for the
/// qualifier rather than for being absent.
#[tokio::test]
async fn a_qualified_identifier_is_refused_before_any_lookup() {
    let (_engine, router) = emulator_holding_a_function().await;

    // (the operation, the sentence it words, the qualifiers it is driven with)
    let operations: &[(&str, &str, &[&str])] = &[
        ("ListAliases", LIST_ALIASES_SENTENCE, &["1", "live"]),
        (
            "ListVersionsByFunction",
            LIST_VERSIONS_SENTENCE,
            &["1", "live", "$LATEST.PUBLISHED"],
        ),
        (
            "ListFunctionEventInvokeConfigs",
            EVENT_INVOKE_CONFIGS_SENTENCE,
            &["1", "live"],
        ),
        (
            "GetFunctionConcurrency",
            FUNCTION_ONLY_SENTENCE,
            // The concurrency operations ask for "either a function name or an unqualified
            // function ARN", so the unpublished version is a qualifier there too.
            &["1", "live", "$LATEST"],
        ),
        (
            "PutFunctionConcurrency",
            FUNCTION_ONLY_SENTENCE,
            &["1", "live", "$LATEST"],
        ),
        (
            "DeleteFunctionConcurrency",
            FUNCTION_ONLY_SENTENCE,
            &["1", "live", "$LATEST"],
        ),
    ];

    for (operation, sentence, qualifiers) in operations {
        for qualifier in *qualifiers {
            // The same qualifier against a function this store HOLDS and against one it
            // does not: the answer is about the request, so both get the same refusal.
            for (existence, name) in [
                ("a function that exists", HELD),
                ("a function that does not", ABSENT),
            ] {
                for (form, reference) in qualified_forms(name, qualifier) {
                    let case =
                        format!("{operation} on {existence} as {form} qualified by {qualifier:?}");
                    let answer = drive(&router, operation, &reference).await;
                    assert_eq!(
                        answer.status,
                        REFUSAL_STATUS,
                        "case {case:?}: expected {REFUSAL_STATUS}, got {} {}",
                        answer.status,
                        body_of(&answer)
                    );
                    assert!(
                        answer.error_code().contains(INVALID_PARAMETER),
                        "case {case:?}: expected {INVALID_PARAMETER}, got {:?}",
                        answer.error_code()
                    );
                    let worded = message(&answer);
                    assert_eq!(
                        worded.as_deref(),
                        Some(*sentence),
                        "case {case:?}: expected {sentence:?}, got {worded:?}"
                    );
                }
            }
        }
    }
}

/// RULE: with no qualifier, the same operations reach the lookup — served for a function
/// this store holds, and answered with the modelled 404 for one it does not. The control
/// that shows the refusals above are about the qualifier rather than about the operation.
#[tokio::test]
async fn an_unqualified_identifier_reaches_the_lookup() {
    let (_engine, router) = emulator_holding_a_function().await;

    for operation in [
        "ListAliases",
        "ListVersionsByFunction",
        "ListFunctionEventInvokeConfigs",
        "ListFunctionUrlConfigs",
        "GetFunctionConcurrency",
    ] {
        for (case, reference, expected) in [
            (
                format!("{operation} on a function that exists"),
                HELD.to_string(),
                200,
            ),
            (
                format!("{operation} on a function ARN that exists"),
                format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{HELD}"),
                200,
            ),
            (
                format!("{operation} on a function that does not exist"),
                ABSENT.to_string(),
                404,
            ),
        ] {
            let answer = drive(&router, operation, &reference).await;
            assert_eq!(
                answer.status,
                expected,
                "case {case:?}: expected {expected}, got {} {}",
                answer.status,
                body_of(&answer)
            );
        }
    }
}

/// RULE: a paginated listing continues only from a `Marker` it issued itself, and refuses
/// any other value with 400 `InvalidParameterValueException` and the one fixed sentence this
/// service words for it.
///
/// One row per kind of order this service pages in — alias names, version labels, minted
/// page tokens and layer version numbers — crossed with the ways a value can name no place
/// in that order: the wrong alphabet, the wrong shape, the right shape under another
/// listing's reading, and the empty string. The last row of each listing is a value that
/// DOES name a place in its order, which must not be refused.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_is_refused() {
    let (_engine, router) = emulator_holding_a_function().await;

    // (the listing, its URI, the marker, whether it names a place in that listing's order)
    let rows: Vec<(&str, String, &str, bool)> = vec![
        (
            "ListAliases",
            format!("/2015-03-31/functions/{HELD}/aliases"),
            "not a marker",
            false,
        ),
        (
            "ListAliases",
            format!("/2015-03-31/functions/{HELD}/aliases"),
            "$LATEST",
            false,
        ),
        (
            "ListAliases",
            format!("/2015-03-31/functions/{HELD}/aliases"),
            "",
            false,
        ),
        (
            "ListAliases",
            format!("/2015-03-31/functions/{HELD}/aliases"),
            "live",
            true,
        ),
        (
            "ListVersionsByFunction",
            format!("/2015-03-31/functions/{HELD}/versions"),
            "not a version",
            false,
        ),
        (
            "ListVersionsByFunction",
            format!("/2015-03-31/functions/{HELD}/versions"),
            "live",
            false,
        ),
        (
            "ListVersionsByFunction",
            format!("/2015-03-31/functions/{HELD}/versions"),
            "",
            false,
        ),
        (
            "ListVersionsByFunction",
            format!("/2015-03-31/functions/{HELD}/versions"),
            "9999",
            true,
        ),
        (
            "ListFunctionUrlConfigs",
            format!("/2021-10-31/functions/{HELD}/urls"),
            "not a marker",
            false,
        ),
        (
            "ListFunctionUrlConfigs",
            format!("/2021-10-31/functions/{HELD}/urls"),
            // The right alphabet for a minted token, but no token this store issued.
            "deadbeef",
            false,
        ),
        (
            "ListFunctionUrlConfigs",
            format!("/2021-10-31/functions/{HELD}/urls"),
            "",
            false,
        ),
        (
            "ListLayerVersions",
            "/2018-10-31/layers/pre-lookup-layer/versions".to_string(),
            "not a version number",
            false,
        ),
        (
            "ListLayerVersions",
            "/2018-10-31/layers/pre-lookup-layer/versions".to_string(),
            "",
            false,
        ),
        (
            "ListLayerVersions",
            "/2018-10-31/layers/pre-lookup-layer/versions".to_string(),
            "7",
            true,
        ),
    ];

    for (operation, uri, marker, names_a_position) in rows {
        let case = format!("{operation} with the marker {marker:?}");
        let answer = json_call(
            &router,
            "GET",
            &format!("{uri}?Marker={}", query_encoded(marker)),
            &Value::Null,
        )
        .await;
        if names_a_position {
            assert_ne!(
                answer.status,
                REFUSAL_STATUS,
                "case {case:?}: a marker that names a place in this listing's order must not \
                 be refused, got {}",
                body_of(&answer)
            );
            continue;
        }
        assert_eq!(
            answer.status,
            REFUSAL_STATUS,
            "case {case:?}: expected {REFUSAL_STATUS}, got {} {}",
            answer.status,
            body_of(&answer)
        );
        assert!(
            answer.error_code().contains(INVALID_PARAMETER),
            "case {case:?}: expected {INVALID_PARAMETER}, got {:?}",
            answer.error_code()
        );
        let worded = message(&answer);
        assert_eq!(
            worded.as_deref(),
            Some(INVALID_MARKER_SENTENCE),
            "case {case:?}: expected {INVALID_MARKER_SENTENCE:?}, got {worded:?}"
        );
    }
}

/// Percent-encode the characters this suite's markers carry that a query string does not.
fn query_encoded(value: &str) -> String {
    value.replace('%', "%25").replace(' ', "%20").replace('$', "%24")
}

/// RULE: `CreateFunction` judges `EphemeralStorage.Size` against the ceiling the member
/// documents ("can be any whole number between 512 and 10,240 MB") before anything is
/// stored, so a request past it is refused and NO function is left behind for a later read
/// to report.
///
/// The rows are the documented bounds themselves, one past the maximum, and values well past
/// it — up to the wider maximum the model's own `EphemeralStorageSize` range admits, which
/// is why the request walk lets such a value through to the handler at all.
#[tokio::test]
async fn a_create_past_the_ephemeral_storage_ceiling_leaves_no_function_behind() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The ceiling the member documents, and the maximum the model's range admits.
    let documented_maximum = 10_240;
    let modelled_maximum = 32_768;
    let rows: &[(&str, i64, bool)] = &[
        ("at the documented minimum", 512, true),
        ("inside the ceiling", 1_024, true),
        ("at the documented maximum", documented_maximum, true),
        (
            "one past the documented maximum",
            documented_maximum + 1,
            false,
        ),
        ("well past the documented maximum", 20_480, false),
        ("at the maximum the model admits", modelled_maximum, false),
    ];

    for (case, size, served) in rows {
        let name = format!("ephemeral-{size}");
        let created = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&package_bytes(32)) },
                "EphemeralStorage": { "Size": size },
            }),
        )
        .await;
        let read = json_call(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}"),
            &Value::Null,
        )
        .await;

        if *served {
            assert_eq!(
                created.status,
                201,
                "case {case:?}: expected 201, got {} {}",
                created.status,
                body_of(&created)
            );
            // Round trip: the size the create stored is the one a read reports.
            let reported = read
                .json()
                .pointer("/Configuration/EphemeralStorage/Size")
                .and_then(Value::as_i64);
            assert_eq!(
                reported,
                Some(*size),
                "case {case:?}: expected the stored size {size} to be read back, got {}",
                body_of(&read)
            );
            continue;
        }

        assert_eq!(
            created.status,
            REFUSAL_STATUS,
            "case {case:?}: expected {REFUSAL_STATUS}, got {} {}",
            created.status,
            body_of(&created)
        );
        let expected = format!(
            "'EphemeralStorage.Size' value failed to satisfy constraint: Member must have \
             value less than or equal to {documented_maximum}"
        );
        let worded = message(&created);
        assert_eq!(
            worded.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {worded:?}"
        );
        // Nothing was created, so the follow-up read reports no function at all rather
        // than a configuration this service never accepted.
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected no function to have been created, got {} {}",
            read.status,
            body_of(&read)
        );
        let absent =
            format!("Function not found: arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}");
        assert_eq!(
            message(&read).as_deref(),
            Some(absent.as_str()),
            "case {case:?}: expected {absent:?}, got {}",
            body_of(&read)
        );
    }
}

/// RULE: a member that belongs to the container-image package type is refused on a function
/// deployed as a .zip archive, before anything is stored — so a create AWS refuses leaves no
/// function behind.
///
/// The rows are each of the entry-point overrides an `ImageConfig` can carry, plus the
/// control that the same create without the member is served and readable.
#[tokio::test]
async fn an_image_config_on_a_zip_package_leaves_no_function_behind() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let sentence = "The ImageConfig parameter is not supported for functions created with \
                    deployment packages.";
    let rows: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "an ImageConfig entry point",
            "zip-with-image-entrypoint",
            json!({ "ImageConfig": { "EntryPoint": ["/usr/bin/python3"] } }),
            Some(sentence),
        ),
        (
            "an ImageConfig command",
            "zip-with-image-command",
            json!({ "ImageConfig": { "Command": ["app.handler"] } }),
            Some(sentence),
        ),
        (
            "an ImageConfig working directory",
            "zip-with-image-workdir",
            json!({ "ImageConfig": { "WorkingDirectory": "/var/task" } }),
            Some(sentence),
        ),
        (
            "no image-only member at all",
            "zip-without-image-config",
            json!({}),
            None,
        ),
    ];

    for (case, name, extra, expected) in rows {
        let mut request = json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(32)) },
        });
        if let (Some(members), Some(extra)) = (request.as_object_mut(), extra.as_object()) {
            for (member, value) in extra {
                members.insert(member.clone(), value.clone());
            }
        }
        let created = create_function(&router, &request).await;
        let read = json_call(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}"),
            &Value::Null,
        )
        .await;

        let Some(expected) = expected else {
            assert_eq!(
                created.status,
                201,
                "case {case:?}: expected 201, got {} {}",
                created.status,
                body_of(&created)
            );
            assert_eq!(
                read.status,
                200,
                "case {case:?}: expected the created function to be readable, got {} {}",
                read.status,
                body_of(&read)
            );
            continue;
        };
        assert_eq!(
            created.status,
            REFUSAL_STATUS,
            "case {case:?}: expected {REFUSAL_STATUS}, got {} {}",
            created.status,
            body_of(&created)
        );
        assert!(
            created.error_code().contains(INVALID_PARAMETER),
            "case {case:?}: expected {INVALID_PARAMETER}, got {:?}",
            created.error_code()
        );
        let worded = message(&created);
        assert_eq!(
            worded.as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {worded:?}"
        );
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected no function to have been created, got {} {}",
            read.status,
            body_of(&read)
        );
    }
}
