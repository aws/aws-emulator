// SPDX-License-Identifier: CC0-1.0

//! `EvaluateMappingTemplate`, end to end over HTTP: the Velocity template a call carries is
//! RENDERED over the context it carries, and the response is what that render produced.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one request
//! that exposed the defect:
//!
//! 1. "Evaluates a given template and returns the response": the `evaluationResult` is the
//!    text the caller's own template rendered over the caller's own context, over a table
//!    spanning the language — literal text, a reference walking into the context by member
//!    and by index, both names the context is read under, a method of the language, the
//!    `$util` helpers, `#set`, `#if`/`#else`, `#foreach`, the comments and the unparsed
//!    block;
//! 2. the `stash` "lives through a single resolver run", so the response reports it as the
//!    render left it, and `outErrors` reports the errors the render appended without ending
//!    it;
//! 3. a template that RAISES (`$util.error`, `$util.unauthorized`) is a SERVED call whose
//!    `error` carries that error's own message, and no `evaluationResult`;
//! 4. a template that cannot be EVALUATED at all — it does not parse, or it names a helper
//!    this reader does not publish — is a SERVED 200 reporting that diagnostic through the
//!    output `error` member, while a `context` that is not a map is the 400
//!    `BadRequestException` the modelled constraint takes;
//! 5. the evaluation is a READ of nothing: the identical call answers identically, and
//!    nothing one render left behind is reachable from the next.
//!
//! Every assertion goes through the router the production emulator mounts, so these exercise
//! routing, restJson1 (de)serialization, the handler and the template reader together.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router and the request helper are the crate's shared test harness, reused rather than
// copied.
use super::error_contract::{call, router, Answer};

/// The operation's own path: `POST /v1/dataplane-evaluatetemplate`
/// (`smithy_query(query="operation", operation="EvaluateMappingTemplate")`).
const PATH: &str = "/v1/dataplane-evaluatetemplate";

/// The class a Velocity parse failure names, which the `error` of an unparseable template
/// carries.
const PARSE_ERROR: &str = "org.apache.velocity.exception.ParseErrorException";

/// Render `template` over `context` and return what the emulator answered.
async fn evaluate(router: &MultiServiceRouter, template: &str, context: &str) -> Answer {
    call(
        router,
        "POST",
        PATH,
        Some(json!({"template": template, "context": context})),
    )
    .await
}

/// The `evaluationResult` of an answer, or `None` where the response carries none.
fn result(answer: &Answer) -> Option<&str> {
    answer.body["evaluationResult"].as_str()
}

/// Rule 1: the `evaluationResult` is the text the template rendered over the context the call
/// supplied.
///
/// The rows differ along what the rule generalises over: which construct of the language the
/// template is written with, which member of the context it reads, and what kind of value
/// that member holds.
#[tokio::test]
async fn the_evaluation_result_is_what_the_template_rendered() {
    let router = router();

    let items = r#"{"result":{"items":[10,20,30]}}"#;
    let one_argument = r#"{"arguments":{"id":"7"}}"#;
    let branch = "#set($id = $ctx.arguments.id)#if($id == \"7\")matched#else missed#end";
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "text carrying no reference at all",
            "a line of text",
            "{}",
            "a line of text",
        ),
        (
            // The context a template is evaluated over is a resolver context, which always
            // has the stash a run writes through — so the whole of it carries the stash
            // whether the call stated one or not.
            "the whole context, rendered as JSON",
            "$util.toJson($context)",
            one_argument,
            r#"{"arguments":{"id":"7"},"stash":{}}"#,
        ),
        (
            "a reference walking into the context by member",
            "$context.arguments.id",
            one_argument,
            "7",
        ),
        (
            "the $ctx alias of that same value",
            "$ctx.arguments.id",
            one_argument,
            "7",
        ),
        (
            "a formal reference inside surrounding text",
            "id=${ctx.arguments.id}.",
            one_argument,
            "id=7.",
        ),
        (
            "a quiet reference that names nothing",
            "[$!{ctx.arguments.missing}]",
            one_argument,
            "[]",
        ),
        (
            "a reference that names nothing, written out as Velocity renders one",
            "[$ctx.arguments.missing]",
            one_argument,
            "[$ctx.arguments.missing]",
        ),
        (
            "an index into a list of the context",
            "$ctx.result.items[1]",
            items,
            "20",
        ),
        (
            "a method of the language on that list",
            "$ctx.result.items.size()",
            items,
            "3",
        ),
        (
            "a $util helper over a value of the context",
            "$util.toJson($ctx.result.items)",
            items,
            "[10,20,30]",
        ),
        ("a #set and the #if branch it decides", branch, one_argument, "matched"),
        (
            "the #else branch of that same template",
            branch,
            r#"{"arguments":{"id":"8"}}"#,
            " missed",
        ),
        (
            "a #foreach over a list of the context",
            "#foreach($n in $ctx.result.items)$n#if($foreach.hasNext),#end#end",
            items,
            "10,20,30",
        ),
        ("a comment, which renders nothing", "kept#* dropped *#", "{}", "kept"),
        (
            "an unparsed block, written out as it stands",
            "#[[$ctx.arguments.id]]#",
            one_argument,
            "$ctx.arguments.id",
        ),
        (
            "a request template building a data source operation",
            "{\"operation\": \"GetItem\", \"key\": {\"id\": $util.toJson($ctx.arguments.id)}}",
            one_argument,
            r#"{"operation": "GetItem", "key": {"id": "7"}}"#,
        ),
    ];

    for (case, template, context, expected) in cases {
        let answer = evaluate(&router, template, context).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            result(&answer),
            Some(*expected),
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            answer.body["error"].is_null(),
            "case {case:?}: a template that rendered reports no error, got {:?}",
            answer.body
        );
        // "A list of logs that were generated by calls to util.log.info and util.log.error":
        // these renders write no line, so the list they report is empty.
        assert_eq!(
            answer.body["logs"],
            json!([]),
            "case {case:?}: expected the lines the render wrote, got {:?}",
            answer.body
        );
    }
}

/// Rule 2: the response reports the stash as the render left it, and the errors it appended.
#[tokio::test]
async fn the_stash_and_the_appended_errors_are_what_the_render_left() {
    let router = router();

    // The stash the REQUEST carried travels back unchanged when the template leaves it alone.
    let carried = evaluate(&router, "kept", r#"{"stash":{"from":"request"}}"#).await;
    assert_eq!(
        (result(&carried), carried.body["stash"].as_str()),
        (Some("kept"), Some(r#"{"from":"request"}"#)),
        "case a stash the request carried: expected it reported back as it stands, got {:?}",
        carried.body
    );

    // A template that WRITES the stash is reported with what it wrote.
    let written = evaluate(
        &router,
        "#set($ctx.stash.carried = \"yes\")rendered",
        r#"{"stash":{}}"#,
    )
    .await;
    assert_eq!(
        (result(&written), written.body["stash"].as_str()),
        (Some("rendered"), Some(r#"{"carried":"yes"}"#)),
        "case a template writing the stash: expected the stash it left, got {:?}",
        written.body
    );

    // A context carrying no stash, and a template writing none, are still reported: both
    // members carry what the render left, which is the empty object and the empty list.
    let none = evaluate(&router, "rendered", "{}").await;
    assert_eq!(
        (
            none.body["stash"].as_str(),
            none.body["outErrors"].as_str()
        ),
        (Some("{}"), Some("[]")),
        "case a run that left neither: expected the empty object and the empty list, got {:?}",
        none.body
    );

    // `$util.appendError` "does not interrupt the evaluation": the render finishes and the
    // errors it appended travel beside its text, in the order it appended them.
    let appended = evaluate(
        &router,
        "$util.appendError(\"noted\")$util.appendError(\"flagged\", \"Flagged\")kept",
        "{}",
    )
    .await;
    assert_eq!(
        result(&appended),
        Some("kept"),
        "case a template appending errors: expected the text it rendered, got {:?}",
        appended.body
    );
    let out_errors: Value = appended.body["outErrors"]
        .as_str()
        .map(|text| serde_json::from_str(text).expect("outErrors is a JSON document"))
        .unwrap_or(Value::Null);
    assert_eq!(
        out_errors,
        json!([
            {"message": "noted", "type": null, "data": null, "info": null},
            {"message": "flagged", "type": "Flagged", "data": null, "info": null},
        ]),
        "case a template appending errors: expected both errors in order, got {:?}",
        appended.body
    );
}

/// Rule 3: a template that RAISES is a served call reporting that error.
#[tokio::test]
async fn a_template_that_raises_is_a_served_call_reporting_the_error() {
    let router = router();

    let field = r#"{"info":{"fieldName":"field","parentTypeName":"Query"}}"#;
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "the sentence $util.error was given",
            "$util.error(\"boom\")",
            "{}",
            "boom",
        ),
        (
            "the same with an errorType beside it",
            "$util.error(\"boom\", \"Refused\")",
            "{}",
            "boom",
        ),
        (
            "a raise reached through a branch, after text was rendered",
            "#if($ctx.info.fieldName)$util.error(\"refused in a branch\")#end",
            field,
            "refused in a branch",
        ),
        (
            // A template evaluated in the SANDBOX is resolving no field, so the refusal
            // $util.unauthorized raises is the word on its own — the same answer the
            // APPSYNC_JS half of this sandbox gives for `util.unauthorized()`. The field-level
            // sentence belongs to a resolution that names a field and a type, which this
            // call does not (`graphql_api_field_authorization`).
            "the refusal $util.unauthorized raises",
            "$util.unauthorized()",
            field,
            "Unauthorized",
        ),
    ];

    for (case, template, context, expected) in cases {
        let answer = evaluate(&router, template, context).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a raise is a served call, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["error"]["message"].as_str(),
            Some(*expected),
            "case {case:?}: expected the error {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            result(&answer).is_none(),
            "case {case:?}: a template that raised rendered no result, got {:?}",
            answer.body
        );
    }
}

/// Rule 4: a template this cannot EVALUATE is a served 200 reporting that diagnostic through
/// the output `error` member, and a `context` that is not a map — a MODELLED constraint of the
/// request — is the one 400 `BadRequestException` left.
///
/// `error` is an output member of this operation's 200 response, so an evaluation that failed
/// is reported in the body: the rows span the ways a template can fail to parse (a block never
/// closed, a directive without its parentheses, a `#set` that is no assignment, an `#end`
/// closing nothing, an unterminated string, an expression with an operand missing) and the
/// ways a parsed one can fail to run (a helper this reader does not publish), beside the
/// request shapes that are refused.
#[tokio::test]
async fn a_template_this_cannot_evaluate_is_a_served_error_and_a_bad_context_is_refused() {
    let router = router();

    // A template that does not PARSE is reported with the Velocity parse diagnostic: the
    // exception class, the token the parse stopped at and where that token sits.
    let unparseable: &[(&str, &str, &str)] = &[
        ("a block that is never closed", "#if($ctx.arguments.id)open", r#"{"arguments":{"id":"7"}}"#),
        ("a loop that is never closed", "#foreach($n in [1, 2])$n", "{}"),
        ("a directive written without its parentheses", "#if $ctx.arguments.id", "{}"),
        ("a #set that is no assignment", "#set($ctx.arguments.id)", "{}"),
        ("an #end closing no block", "rendered#end", "{}"),
        ("an unterminated string", "#set($x = \"unclosed)", "{}"),
        ("an expression with an operand missing", "#if($x ==)yes#end", "{}"),
    ];
    for (case, template, context) in unparseable {
        let answer = evaluate(&router, template, context).await;
        let message = answer.body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a template that does not parse is a served call, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            message.contains(PARSE_ERROR) && message.contains("at velocity[line "),
            "case {case:?}: expected the parse diagnostic, got {message:?}"
        );
        assert!(
            result(&answer).is_none(),
            "case {case:?}: a template that did not parse rendered nothing, got {:?}",
            answer.body
        );
        // Nothing ran, so the run's own members are the ones it would have started from.
        assert_eq!(
            (
                answer.body["stash"].as_str(),
                answer.body["outErrors"].as_str(),
                answer.body["logs"].as_str()
            ),
            (Some("{}"), Some("[]"), None),
            "case {case:?}: expected the members of a run that never happened, got {:?}",
            answer.body
        );
    }

    // A template that parses but cannot be RUN is reported the same way, with the diagnostic
    // naming what it asked for.
    let unrunnable = evaluate(&router, "$util.noSuchHelper()", "{}").await;
    assert_eq!(
        (
            unrunnable.status,
            unrunnable.body["error"]["message"].as_str(),
            result(&unrunnable)
        ),
        (200, Some("No such helper: $util.noSuchHelper"), None),
        "case a helper this reader does not publish: got {} {:?}",
        unrunnable.status,
        unrunnable.body
    );

    // A `context` that is not the map the model requires is the refusal: the REQUEST is what
    // is malformed, not the template.
    let refused: &[(&str, &str)] = &[
        ("a context that is a list rather than a map", "[1]"),
        ("a context that is a number", "12"),
        ("a context that is a quoted string", "\"text\""),
        ("a context that is not JSON at all", "not json"),
    ];
    for (case, context) in refused {
        let answer = evaluate(&router, "rendered", context).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str(), answer.message.as_str()),
            (400, "BadRequestException", "Invalid context given."),
            "case {case:?}: expected the refusal, got {} {} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        assert!(
            result(&answer).is_none(),
            "case {case:?}: a refused request renders nothing, got {:?}",
            answer.body
        );
    }
}

/// Rule 5: the evaluation is a read of nothing — the identical call answers identically, and
/// what one render left behind is not reachable from the next.
#[tokio::test]
async fn one_render_leaves_nothing_behind_for_the_next() {
    let router = router();

    let template = "#set($ctx.stash.seen = \"once\")$util.toJson($ctx.stash)";
    let first = evaluate(&router, template, "{}").await;
    let second = evaluate(&router, template, "{}").await;
    assert_eq!(
        (first.status, result(&first)),
        (200, Some(r#"{"seen":"once"}"#)),
        "case the first render: expected the stash it wrote, got {} {:?}",
        first.status,
        first.body
    );
    assert_eq!(
        (second.status, result(&second)),
        (first.status, result(&first)),
        "case the identical render again: expected the same answer, got {} {:?}",
        second.status,
        second.body
    );

    // A later call that reads the stash without writing it sees the context IT supplied, so
    // nothing of the renders above reached it.
    let read = evaluate(&router, "$util.toJson($ctx.stash)", r#"{"stash":{}}"#).await;
    assert_eq!(
        (read.status, result(&read)),
        (200, Some("{}")),
        "case a later render reading the stash: expected the caller's own stash, got {} {:?}",
        read.status,
        read.body
    );
}
