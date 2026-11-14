// SPDX-License-Identifier: CC0-1.0

//! Which expression a `@pattern` clause of this data plane QUOTES.
//!
//! One rule, asserted over a table rather than over one recorded call: the clause quotes
//! the expression declared for the MEMBER whose value it refuses, verbatim — anchors
//! included — and the family's own declaration at the three members quoted in the
//! SageMaker Runtime family's start-anchored form (the endpoint a request addresses, the
//! production variant and the container hostname it targets). The family's sibling model
//! declares the first two of those without an end anchor where this model's export appends
//! one; it binds no container-hostname member, so that third one is carried along with
//! them, which is also what this service answered for it before.
//!
//! The rows differ along the member, the route and the expression, and include members
//! this model exports with the SAME characters as the resource-name expression: the choice
//! is keyed to the member's declaration, never to the text of the exported expression.
//!
//! The second suite pins the other half: the CHECK still uses the expression this model
//! declares, so a value the start-anchored form would admit and the declared, fully
//! anchored one does not is still refused, and every value the declaration admits still
//! gets as far as resolving the endpoint it addressed.

use crate::harness::{
    async_inference_config, async_invocations, invocations, post, response_stream, Emulator,
    Headers, ACCOUNT, INPUT_LOCATION,
};

/// One quoting row: the case, the path it calls, the headers it sends, the member the
/// clause must name, the value it must quote and the expression it must quote.
type QuotingRow<'r> = (&'r str, String, Headers<'r>, &'r str, &'r str, &'r str);

/// The expression the SageMaker Runtime family declares for a resource NAME, anchored at
/// the start only: `smithy_query(query="shape",
/// shape="InvokeEndpointWithBidirectionalStreamInput",
/// model_path="services/service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2/smithy-model/com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2.json")`
/// declares it for `EndpointName` and `TargetVariant` — the two resource-name members that
/// sibling binds, whose other patterned members DO carry an end anchor — while this
/// service's export appends one.
const FAMILY_RESOURCE_NAME: &str = "^[a-zA-Z0-9](-*[a-zA-Z0-9])*";

/// The expressions this model declares, as `smithy_query(query="operation",
/// operation="InvokeEndpoint")`, `"InvokeEndpointAsync"` and
/// `"InvokeEndpointWithResponseStream"` report them.
const DECLARED_SESSION_ID: &str = "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$";
const DECLARED_SESSION_ID_OR_SENTINEL: &str = "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$";
const DECLARED_INFERENCE_COMPONENT: &str = r"^[a-zA-Z0-9]([\-a-zA-Z0-9]*[a-zA-Z0-9])?$";
const DECLARED_INPUT_LOCATION: &str = "^(https|s3)://([^/]+)/?(.*)$";

/// An endpoint name the declaration admits, for a row whose refused member is a header.
const WELL_FORMED_NAME: &str = "absent-endpoint";

/// `Filename`'s declaration, as `smithy_query(query="operation",
/// operation="InvokeEndpointAsync")` reports it. Its negative lookahead is the half the
/// framework's constraint walk cannot enforce, so the asynchronous route refuses that
/// member in a sentence of its own — which quotes the declaration the same way a clause of
/// the shared envelope does.
const DECLARED_FILENAME: &str = r"^(?!.*\..*\.)[a-zA-Z0-9][a-zA-Z0-9-_\.]*$";

/// Where the deployment in [`a_filename_refusal_quotes_the_declaration_too`] writes its
/// asynchronous results.
const OUTPUT_PATH: &str = "s3://aws-emulator-sagemaker/async/output";

/// RULE: a `@pattern` clause quotes the expression declared for the member it refuses —
/// this model's export verbatim, and the family's own declaration at the family's three
/// resource-name members.
#[tokio::test]
async fn a_pattern_clause_quotes_the_expression_declared_for_its_member() {
    let emulator = Emulator::new();
    let rows: Vec<QuotingRow<'_>> = vec![
        // The three resource-name members, across the three routes.
        (
            "endpointName, an underscored name",
            invocations("sim2real_probe_bad_name"),
            vec![],
            "endpointName",
            "sim2real_probe_bad_name",
            FAMILY_RESOURCE_NAME,
        ),
        (
            "endpointName on the streamed route, trailing junk",
            response_stream("good-name!"),
            vec![],
            "endpointName",
            "good-name!",
            FAMILY_RESOURCE_NAME,
        ),
        (
            "endpointName on the asynchronous route, a dotted name",
            async_invocations("bad.name"),
            vec![INPUT_LOCATION],
            "endpointName",
            "bad.name",
            FAMILY_RESOURCE_NAME,
        ),
        (
            "targetVariant, an underscored variant",
            invocations(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-target-variant", "not_a_variant")],
            "targetVariant",
            "not_a_variant",
            FAMILY_RESOURCE_NAME,
        ),
        (
            "targetVariant on the streamed route, trailing junk",
            response_stream(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-target-variant", "Variant!")],
            "targetVariant",
            "Variant!",
            FAMILY_RESOURCE_NAME,
        ),
        (
            "targetContainerHostname",
            invocations(WELL_FORMED_NAME),
            vec![(
                "x-amzn-sagemaker-target-container-hostname",
                "bad_host_name",
            )],
            "targetContainerHostname",
            "bad_host_name",
            FAMILY_RESOURCE_NAME,
        ),
        // `SessionIdHeader` on the streamed route: this model exports it with the SAME
        // characters as the resource-name members, and the family declares it WITH its end
        // anchor, so the clause quotes that anchor.
        (
            "sessionId on the streamed route, the sentinel the route does not admit",
            response_stream(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-session-id", "NEW_SESSION")],
            "sessionId",
            "NEW_SESSION",
            DECLARED_SESSION_ID,
        ),
        (
            "sessionId on the streamed route, an underscored id",
            response_stream(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-session-id", "not_a_session_id")],
            "sessionId",
            "not_a_session_id",
            DECLARED_SESSION_ID,
        ),
        (
            "sessionId on the streamed route, trailing junk",
            response_stream(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-session-id", "session-1!")],
            "sessionId",
            "session-1!",
            DECLARED_SESSION_ID,
        ),
        // `SessionIdOrNewSessionConstantHeader`: the same wire member name on the unary
        // route, a declaration of its own, quoted as this model declares it.
        (
            "sessionId on the unary route, an alternation",
            invocations(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-session-id", "not_a_session_id")],
            "sessionId",
            "not_a_session_id",
            DECLARED_SESSION_ID_OR_SENTINEL,
        ),
        // A member whose name reads like a resource name and whose declaration is its own.
        (
            "inferenceComponentName",
            invocations(WELL_FORMED_NAME),
            vec![(
                "x-amzn-sagemaker-inference-component",
                "bad_component_name",
            )],
            "inferenceComponentName",
            "bad_component_name",
            DECLARED_INFERENCE_COMPONENT,
        ),
        (
            "inferenceComponentName on the streamed route, trailing junk",
            response_stream(WELL_FORMED_NAME),
            vec![("x-amzn-sagemaker-inference-component", "trailing-")],
            "inferenceComponentName",
            "trailing-",
            DECLARED_INFERENCE_COMPONENT,
        ),
        (
            "inputLocation on the asynchronous route, an unsupported scheme",
            async_invocations(WELL_FORMED_NAME),
            vec![(
                "x-amzn-sagemaker-inputlocation",
                "ftp://bucket/input/payload.json",
            )],
            "inputLocation",
            "ftp://bucket/input/payload.json",
            DECLARED_INPUT_LOCATION,
        ),
    ];

    for (case, path, headers, member, value, expression) in rows {
        let call = post(&emulator, &path, &headers, b"payload").await;
        assert_eq!(
            call.status,
            400,
            "case {case}: expected HTTP 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationError",
            "case {case}: expected the modelled client-input shape, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.message(),
            format!(
                "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {expression}"
            ),
            "case {case}: expected the expression declared for {member} quoted as \
             {expression:?}, got {:?}",
            call.message()
        );
    }
}

/// RULE: the expression this model declares is still what the CHECK enforces, so how a
/// clause quotes it changes nothing about WHICH requests are refused.
///
/// The refused rows all begin with a run the start-anchored form matches and then break
/// the declaration, so only the fully anchored check refuses them; the admitted rows span
/// the shapes of name the declaration does admit.
#[tokio::test]
async fn the_declared_expression_is_still_what_the_check_enforces() {
    let emulator = Emulator::new();

    for value in ["good-name!", "trailing-", "9name*"] {
        let call = post(&emulator, &invocations(value), &[], b"payload").await;
        assert_eq!(
            call.status,
            400,
            "case {value:?}: a value only the start-anchored form admits must still be \
             refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert!(
            call.message()
                .contains("Member must satisfy regular expression pattern"),
            "case {value:?}: expected the pattern clause, got {:?}",
            call.message()
        );
    }

    for value in ["a", "9", "a-b-c", "AbsentEndpoint"] {
        let call = post(&emulator, &invocations(value), &[], b"payload").await;
        assert_eq!(
            call.message(),
            format!("Endpoint {value} of account {ACCOUNT} not found."),
            "case {value:?}: a value the declaration admits must reach the endpoint lookup, \
             got {:?}",
            call.message()
        );
    }
}

/// RULE: the same quoting holds for the one member this service refuses in a sentence of
/// its own — the asynchronous route's `Filename`, whose declaration carries a negative
/// lookahead the framework's constraint walk cannot enforce: that sentence quotes the
/// member's declaration verbatim, anchors included, like every clause above.
///
/// The rows differ along WHICH half of the declaration the value breaks (the lookahead
/// that admits at most one dot, the character class, the first-character class), so the
/// assertion is about the declaration and not about one offending value.
#[tokio::test]
async fn a_filename_refusal_quotes_the_declaration_too() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "filename-quoting",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    for (case, value) in [
        ("two dots", "a.b.c"),
        ("a path separator", "sub/answer.json"),
        ("a leading separator character", "-answer.json"),
    ] {
        let headers = vec![INPUT_LOCATION, ("x-amzn-sagemaker-filename", value)];
        let call = post(
            &emulator,
            &async_invocations("filename-quoting"),
            &headers,
            b"",
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case} ({value:?}): expected HTTP 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.message(),
            format!(
                "Filename {value:?} failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {DECLARED_FILENAME}"
            ),
            "case {case} ({value:?}): expected the declaration quoted verbatim, got {:?}",
            call.message()
        );
    }
}
