// SPDX-License-Identifier: CC0-1.0

//! The DIAGNOSTIC `EvaluateMappingTemplate` answers a template that does not parse with.
//!
//! "Mapping templates are written in the Apache Velocity Template Language (VTL)"
//! (`smithy_query(query="operation", operation="EvaluateMappingTemplate")`), and `error`
//! (`ErrorDetail`) is an OUTPUT member of this operation's 200 response — so text that is not
//! a template is a SERVED call whose body carries the parse failure, and that failure is the
//! only machine-readable account of WHERE and WHY the text stopped being a template.
//!
//! The rule asserted here, over the CLASS of text that does not parse rather than over the one
//! template that exposed the defect: the message is the Velocity parser's own
//! `ParseErrorException` diagnostic in full —
//!
//! 1. the exception class, then `Encountered "<token>" at velocity[line L, column C]`, where
//!    the token is the one the parse stopped at as the language's reader takes it off the text
//!    and `L`/`C` are THAT token's own 1-based position — the end-of-input token being stated
//!    at the last character the parser read, not at the place after it;
//! 2. then, on the following lines, the tokens the grammar would have accepted there: the
//!    header the parser states them under (`Was expecting one of:` over a set,
//!    `Was expecting:` over a single token) and one four-space-indented `<TOKEN> ...` line per
//!    token, the last of them followed by that same indent;
//! 3. and nothing of the accepted-token section where the parse stopped against one of this
//!    endpoint's own bounds rather than against the grammar, which states no accepted set.
//!
//! The rows differ along what the rule generalises over: which construct the text stopped in
//! (and so which token set the grammar accepts there), whether the parse stopped at a token of
//! the text or at the end of it, how long the text is, and which line it stopped on.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The router and the request helper are the crate's shared test harness, reused rather than
// copied.
use super::error_contract::{call, router, Answer};

/// The operation's own path: `POST /v1/dataplane-evaluatetemplate`
/// (`smithy_query(query="operation", operation="EvaluateMappingTemplate")`).
const PATH: &str = "/v1/dataplane-evaluatetemplate";

/// The class a Velocity parse failure is raised under, which the diagnostic opens with.
const PARSE_ERROR: &str = "org.apache.velocity.exception.ParseErrorException";

/// The headers the accepted tokens are listed under, and how each of them is written: four
/// spaces of indent, the token image, and ` ...` for the text that could have followed it.
const ONE_OF: &str = "Was expecting one of:";
/// See [`ONE_OF`].
const EXACTLY: &str = "Was expecting:";
/// See [`ONE_OF`].
const INDENT: &str = "\n    ";
/// See [`ONE_OF`].
const MORE: &str = " ...";

/// The arithmetic operators the grammar accepts just past a complete operand, as its own
/// diagnostic images them. Source: the operator spellings of the one reader of this language
/// (`service_common::velocity_template`), imaged as the Apache Velocity parser grammar spells
/// them — inline, and so quoted.
const ARITHMETIC: [&str; 5] = ["\"-\"", "\"+\"", "\"*\"", "\"/\"", "\"%\""];

/// The logical and relational operators it accepts there, each a NAMED token of that grammar
/// (`LOGICAL_AND` … `LOGICAL_NOT_EQUALS`), and so imaged `<NAME>`.
const LOGICAL: [&str; 8] = [
    "<LOGICAL_AND>",
    "<LOGICAL_OR>",
    "<LOGICAL_LT>",
    "<LOGICAL_LE>",
    "<LOGICAL_GT>",
    "<LOGICAL_GE>",
    "<LOGICAL_EQUALS>",
    "<LOGICAL_NOT_EQUALS>",
];

/// What the grammar accepts just past a COMPLETE operand: the expression may be indexed, the
/// construct around it may close with one of `closers`, space may separate it from either, or
/// an operator may carry the expression on.
fn after_operand(closers: &[&str]) -> Vec<String> {
    let mut tokens = vec!["\"[\"".to_string()];
    tokens.extend(closers.iter().map(|closer| (*closer).to_string()));
    tokens.push("<WHITESPACE>".to_string());
    tokens.extend(
        ARITHMETIC
            .iter()
            .chain(LOGICAL.iter())
            .map(|token| (*token).to_string()),
    );
    tokens
}

/// What the grammar accepts where an operand must BEGIN: a reference or a word, a literal of
/// each kind the language has, a prefix operator, a grouped expression, a list or a map.
fn operand() -> Vec<String> {
    [
        "<WHITESPACE>",
        "<LPAREN>",
        "<IDENTIFIER>",
        "<STRING_LITERAL>",
        "<INTEGER_LITERAL>",
        "<FLOATING_POINT_LITERAL>",
        "<TRUE>",
        "<FALSE>",
        "<LOGICAL_NOT>",
        "\"-\"",
        "\"[\"",
        "\"{\"",
    ]
    .iter()
    .map(|token| (*token).to_string())
    .collect()
}

/// A few token images written out as such, for the positions that accept only those.
fn tokens(images: &[&str]) -> Vec<String> {
    images.iter().map(|image| (*image).to_string()).collect()
}

/// The whole diagnostic of a parse that stopped at `token`, at line `line` and column `column`,
/// where the grammar would have accepted `accepted`.
fn diagnostic(token: &str, line: usize, column: usize, accepted: &[String]) -> String {
    let mut message =
        format!("{PARSE_ERROR}: Encountered \"{token}\" at velocity[line {line}, column {column}]");
    if accepted.is_empty() {
        return message;
    }
    message.push('\n');
    message.push_str(if accepted.len() == 1 { EXACTLY } else { ONE_OF });
    for image in accepted {
        message.push_str(INDENT);
        message.push_str(image);
        message.push_str(MORE);
    }
    message.push_str(INDENT);
    message
}

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

/// The `error.message` of an answer, or the empty string where it carries none.
fn message(answer: &Answer) -> String {
    answer.body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Rules 1 and 2 over the recorded CLASS, written out token for token: a template whose
/// expression simply stops is the full diagnostic, the position wrapped in the name the parser
/// reads the text under and the accepted tokens listed one per line.
#[tokio::test]
async fn an_expression_that_simply_stops_is_the_whole_velocity_diagnostic() {
    let router = router();

    // 28 characters, so the end-of-input token sits at the 28th column: the last character the
    // parser read, not the place after it.
    let template = "#set($id = $ctx.arguments.id";
    let expected = "org.apache.velocity.exception.ParseErrorException: Encountered \"<EOF>\" at \
                    velocity[line 1, column 28]\nWas expecting one of:\n    \"[\" ...\n    \
                    <RPAREN> ...\n    <WHITESPACE> ...\n    \"-\" ...\n    \"+\" ...\n    \
                    \"*\" ...\n    \"/\" ...\n    \"%\" ...\n    <LOGICAL_AND> ...\n    \
                    <LOGICAL_OR> ...\n    <LOGICAL_LT> ...\n    <LOGICAL_LE> ...\n    \
                    <LOGICAL_GT> ...\n    <LOGICAL_GE> ...\n    <LOGICAL_EQUALS> ...\n    \
                    <LOGICAL_NOT_EQUALS> ...\n    ";
    let answer = evaluate(&router, template, r#"{"arguments":{"id":"7"}}"#).await;

    assert_eq!(
        answer.status, 200,
        "case an expression that simply stops: a template that does not parse is a served \
         call, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        message(&answer),
        expected,
        "case an expression that simply stops: expected the whole Velocity diagnostic, got {:?}",
        message(&answer)
    );
    assert_eq!(
        answer.body["evaluationResult"].as_str(),
        None,
        "case an expression that simply stops: a template that did not parse rendered nothing, \
         got {:?}",
        answer.body
    );
}

/// Rules 1, 2 and 3 over the class: every way text stops being a template is that same
/// diagnostic, stating THAT position and THAT accepted-token set.
#[tokio::test]
async fn every_parse_failure_is_stated_as_the_velocity_diagnostic() {
    let router = router();

    let cases: Vec<(&str, &str, String)> = vec![
        (
            "an unterminated #set",
            "#set($id = $ctx.arguments.id",
            diagnostic("<EOF>", 1, 28, &after_operand(&["<RPAREN>"])),
        ),
        (
            // The same construct, one line further down: the position is the one THAT line
            // gives it.
            "an unterminated #set on the second line",
            "#if($ok)\n#set($x = 1",
            diagnostic("<EOF>", 2, 11, &after_operand(&["<RPAREN>"])),
        ),
        (
            "a list that is never closed, on the second line",
            "#set($x = [1,\n2",
            diagnostic("<EOF>", 2, 1, &after_operand(&["\"]\"", "\",\""])),
        ),
        (
            // A parse that stopped at a token OF THE TEXT quotes that one token and points at
            // where it begins.
            "a #set whose value is missing",
            "#set($x = )rendered",
            diagnostic(")", 1, 11, &operand()),
        ),
        (
            "a condition whose right operand is missing",
            "#if($x ==)yes#end",
            diagnostic(")", 1, 10, &operand()),
        ),
        (
            "a #set that is no assignment",
            "#set($ctx.arguments.id)",
            diagnostic(")", 1, 23, &tokens(&["\"=\"", "<WHITESPACE>"])),
        ),
        (
            "a directive written without its parentheses",
            "#if $ctx.arguments.id",
            diagnostic(
                "$ctx.arguments.id",
                1,
                5,
                &tokens(&["<LPAREN>", "<WHITESPACE>"]),
            ),
        ),
        (
            "a #foreach written without its in",
            "#foreach($n [1])",
            diagnostic("[", 1, 13, &tokens(&["<IN>", "<WHITESPACE>"])),
        ),
        (
            "a map whose key is not followed by its value",
            "#set($x = {\"a\" 1})",
            diagnostic("1", 1, 16, &tokens(&["\":\"", "<WHITESPACE>"])),
        ),
        (
            // A position that accepts exactly ONE token is headed in the singular.
            "a loop that is never closed",
            "#foreach($n in [1, 2])$n",
            diagnostic("<EOF>", 1, 24, &tokens(&["<END>"])),
        ),
        (
            "a conditional that is never closed",
            "#if($ok)open",
            diagnostic(
                "<EOF>",
                1,
                12,
                &tokens(&["<END>", "<ELSE>", "<ELSEIF>"]),
            ),
        ),
        (
            "an #end closing no block",
            "rendered#end",
            diagnostic("#end", 1, 9, &tokens(&["<EOF>"])),
        ),
        (
            // Rule 3: text the LEXER refuses rather than the parser has no accepted-token set,
            // so the diagnostic is the position alone.
            "a quoted string that is never closed",
            "#set($x = \"unclosed)",
            diagnostic("<EOF>", 1, 20, &[]),
        ),
    ];

    for (case, template, expected) in cases {
        let answer = evaluate(&router, template, "{}").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a template that does not parse is a served call, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {case:?}: expected the Velocity diagnostic {expected:?}, got {:?}",
            message(&answer)
        );
        assert_eq!(
            answer.body["evaluationResult"].as_str(),
            None,
            "case {case:?}: a template that did not parse rendered nothing, got {:?}",
            answer.body
        );
    }
}

/// Rule 1's POSITION, as an invariant over the text rather than over one template: wherever a
/// template simply runs out mid-construct, the end-of-input token is stated at the LAST
/// CHARACTER the parser read — its line is the last line of the text and its column is that
/// line's own length.
#[tokio::test]
async fn the_end_of_input_token_is_stated_at_the_last_character_read() {
    let router = router();

    // The rows differ in how long the text is and on which line it stops, which is what the
    // position generalises over; each one ends mid-`#set`, so each stops at end of input.
    let templates = [
        "#set($v = $ctx.a",
        "#set($v = $ctx.arguments.id",
        "#set($v = $ctx.arguments.identifier",
        "line\n#set($v = $ctx.a",
        "line\nmore\n#set($v = $ctx.arguments.id",
        "#if($ok)\n#foreach($n in [1])\n#set($v = $ctx.a",
    ];
    for template in templates {
        let answer = evaluate(&router, template, "{}").await;
        let characters: Vec<char> = template.chars().collect();
        let lines = characters.iter().filter(|character| **character == '\n').count() + 1;
        let last_line = template.split('\n').next_back().unwrap_or_default();
        let column = last_line.chars().count();
        let expected = format!(
            "{PARSE_ERROR}: Encountered \"<EOF>\" at velocity[line {lines}, column {column}]"
        );
        let message = message(&answer);
        assert_eq!(
            answer.status, 200,
            "case {template:?}: a template that does not parse is a served call, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            message.starts_with(&expected),
            "case {template:?}: expected the position {expected:?}, got {message:?}"
        );
    }
}

/// The control: a template that DOES parse is unaffected — it renders, and reports no error at
/// all, so only the diagnostic of text that does not parse is what the rules above state.
#[tokio::test]
async fn a_template_that_parses_still_renders_with_no_error() {
    let router = router();

    let answer = evaluate(
        &router,
        "#set($id = $ctx.arguments.id)id is $id",
        r#"{"arguments":{"id":"7"}}"#,
    )
    .await;
    assert_eq!(
        (
            answer.status,
            answer.body["evaluationResult"].as_str(),
            answer.body["error"].is_null()
        ),
        (200, Some("id is 7"), true),
        "case a well-formed template: expected the text it rendered and no error, got {} {:?}",
        answer.status,
        answer.body
    );
}
