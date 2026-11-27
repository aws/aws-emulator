// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudWatch Logs query refusals that state a RULE rather than the
//! symbol the compiler stopped on, and for the one symbol-shaped refusal that names a stage
//! separator.
//!
//! Three rules, each pinned over a table of inputs rather than one recorded request:
//!
//! * a `/…/` literal of a `parse` or `filter` command is a REGULAR EXPRESSION: one that is not
//!   is refused with the COMPILE FAILURE of that expression — the expression as the compiler
//!   reads it (every named capture group renamed to its positional name and opening with the
//!   inline flag that lets `.` match a newline), what is wrong, and the index into that
//!   expression the compiler stopped at — and never as an unexpected symbol, because the
//!   literal is a literal where it stands and the fault is inside it;
//! * a pipeline may carry one ephemeral field of a given name: a command that DEFINES a name
//!   an earlier command of the same pipeline already created — with an `as` clause, with a
//!   named capture group, or as an item of a `fields` projection, which declares the fields
//!   its stage produces — is refused with the duplicate-field rule, the name, and the marker
//!   this service prints where a span of the text at fault would stand, which this class
//!   carries none of. A command that only READS such a field declares nothing and is admitted,
//!   and the modelled detail still locates the name inside the query;
//! * a query whose OPENING word is no command of the language has no command read when it
//!   reaches a stage separator that separates nothing, so that separator is the symbol the
//!   diagnostic names; where the pipeline did open with a command, the diagnostic names the
//!   WORD that is no command.
//!
//! Every message is the COMPILER's own text and nothing else: the clause an AWS SDK writes
//! when it renders an exception is the client's, not this service's, and is absent from the
//! serialized body.
//!
//! Every case drives the service the way a client does — `POST /` with `X-Amz-Target:
//! Logs_20140328.<Operation>` through the router that mounts it — so a response a handler does
//! not compute cannot pass. The counterpart suites: the accepted strings are in
//! [`crate::query_language_admission`], the end-of-input diagnostic in
//! [`crate::dangling_query_commands`] and the other refusal texts in
//! [`crate::query_and_stream_refusal_text`].

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";

/// The modelled error code a query string that does not compile is refused under
/// (`smithy_query(query="operation",operation="StartQuery")` -> `MalformedQueryException`,
/// httpError 400).
const MALFORMED: &str = "MalformedQueryException";

/// The status such a refusal is answered with.
const REFUSAL_STATUS: u16 = 400;

/// The fixed text a refusal for a query the compiler could not build opens with.
const COMPILE_ERROR: &str = "Compile Error - ";

/// A log group and the window a query of it is started with, in the epoch SECONDS
/// `StartQuery`'s range is specified in.
async fn group_with_window(logs: &Logs, group: &str) -> (i64, i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let now_secs = now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// Start `query_string` over `group`, answering the status and the body.
async fn start(logs: &Logs, group: &str, window: (i64, i64), query_string: &str) -> (u16, Value) {
    logs.call(
        START,
        &json!({
            "logGroupName": group,
            "startTime": window.0,
            "endTime": window.1,
            "queryString": query_string,
        }),
    )
    .await
}

/// The refusal's message with the SDK envelope stripped off, having asserted the refusal's
/// status, its code and that no `queryId` was minted.
async fn refused_with(logs: &Logs, case: &str, group: &str, window: (i64, i64), query: &str) -> (String, Value) {
    let (status, body) = start(logs, group, window, query).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case}: expected the query to be refused, got {status} with {body}"
    );
    let code = crate::harness::error_code(&body);
    assert_eq!(
        code, MALFORMED,
        "case {case}: expected {MALFORMED}, got {code} with {body}"
    );
    assert!(
        body.get("queryId").is_none(),
        "case {case}: a refused query mints no queryId, got {body}"
    );
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal carries a message, got {body}"))
        .to_string();
    (
        // The compile failure's text is the SENTENCE of the message, which the envelope this
        // service closes every refusal with follows.
        crate::harness::without_sdk_envelope(case, &message, REFUSAL_STATUS, MALFORMED),
        body,
    )
}

/// The place a diagnostic states for the character at `offset` of `query`: the line, counted
/// from one, and the character position within that line, counted from zero — read off the
/// query here rather than from the service, so a diagnostic that did not read the query cannot
/// state it.
fn place(query: &str, offset: usize) -> (usize, usize) {
    let before = &query[..offset];
    let line = before.matches('\n').count() + 1;
    let position = before
        .rsplit_once('\n')
        .map_or(before, |(_, last)| last)
        .chars()
        .count();
    (line, position)
}

/// A regular-expression literal that is not a regular expression is refused with the compile
/// failure of the expression the compiler READS — the rewritten one — naming the fault and the
/// index it stopped at.
#[tokio::test]
async fn a_regex_literal_that_does_not_compile_is_refused_with_the_compile_failure() {
    let logs = Logs::new();
    const GROUP: &str = "/compile-rules/regex";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string, the literal with its delimiters, the expression the compiler
    // reads, what is wrong, the index it stopped at). The rows span what the rule generalises
    // over: which command carries the literal, how many named groups it has (so the positional
    // renaming is visible), which construct is broken, and where in the query the literal
    // sits.
    let cases: Vec<(&str, &str, &str, &str, &str, usize)> = vec![
        (
            "a named group the expression never closes",
            r"fields @message | parse @message /lat=(?<lat>\d+/",
            r"/lat=(?<lat>\d+/",
            r"lat=(?<capture000>(?s)\d+",
            "Unclosed group",
            25,
        ),
        (
            // The literal moves, the expression does not: the compile failure is about the
            // expression and says the same thing wherever the query carries it.
            "the same literal further along the pipeline",
            r"fields @message | sort @timestamp | parse @message /lat=(?<lat>\d+/",
            r"/lat=(?<lat>\d+/",
            r"lat=(?<capture000>(?s)\d+",
            "Unclosed group",
            25,
        ),
        (
            "two named groups, the second one unclosed",
            r"fields @message | parse @message /(?<a>\w+) (?<b>\w+/",
            r"/(?<a>\w+) (?<b>\w+/",
            r"(?<capture000>(?s)\w+) (?<capture001>(?s)\w+",
            "Unclosed group",
            44,
        ),
        (
            "an unnamed group, which is renamed to nothing",
            r"fields @message | parse @message /lat=(\d+/",
            r"/lat=(\d+/",
            r"lat=(\d+",
            "Unclosed group",
            8,
        ),
        (
            "a quantifier with nothing to repeat",
            r"fields @message | parse @message /+/",
            "/+/",
            "+",
            "Dangling meta character '+'",
            0,
        ),
        (
            "a character class the expression never closes",
            r"fields @message | parse @message /lat=[0-9/",
            "/lat=[0-9/",
            "lat=[0-9",
            "Unclosed character class",
            7,
        ),
        (
            "a closing parenthesis nothing opened",
            r"fields @message | parse @message /lat=\d+)/",
            r"/lat=\d+)/",
            r"lat=\d+)",
            "Unmatched closing ')'",
            7,
        ),
        (
            // The other command that matches against a pattern: the same rule, read where
            // that command writes its literal.
            "a filter matching against an expression that does not compile",
            r"fields @message | filter @message like /ERROR(/",
            "/ERROR(/",
            "ERROR(",
            "Unclosed group",
            6,
        ),
    ];

    for (case, query, literal, read, description, index) in &cases {
        let (said, body) = refused_with(&logs, case, GROUP, window, query).await;
        let mut expected = format!(
            "{COMPILE_ERROR}Invalid regular expression in query: '{read}' — {description} near \
             index {index}\n{read}"
        );
        // The caret marks the index only where the index is INSIDE the expression: a fault the
        // compiler reaches at the end of one has no character there to point at.
        if *index < read.chars().count() {
            expected.push('\n');
            for _ in 0..*index {
                expected.push(' ');
            }
            expected.push('^');
        }
        assert_eq!(
            said, expected,
            "case {case}: expected the compile failure of {read:?}, got {said:?}"
        );
        // "Details about this error are displayed in a QueryCompileError object": the detail
        // locates the literal inside the submitted query.
        let at = query
            .find(literal)
            .unwrap_or_else(|| panic!("case {case}: the row carries the literal {literal:?}"));
        let location = &body["queryCompileError"]["location"];
        assert_eq!(
            (
                location["startCharOffset"].as_i64(),
                location["endCharOffset"].as_i64(),
            ),
            (
                Some(i64::try_from(at).unwrap_or(i64::MAX)),
                Some(i64::try_from(at + literal.chars().count()).unwrap_or(i64::MAX)),
            ),
            "case {case}: the compile error locates the literal, got {body}"
        );
        assert!(
            !body["queryCompileError"]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case}: the compile error says what is wrong, got {body}"
        );
    }

    // Nothing of a refused query is left behind.
    let page = logs.ok(DESCRIBE, &json!({"logGroupName": GROUP})).await;
    let reported: Vec<String> = page["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueries reports a queries list, got {page}"))
        .iter()
        .map(|query| query["queryString"].as_str().unwrap_or_default().to_string())
        .collect();
    for (case, query, ..) in &cases {
        assert!(
            !reported.contains(&(*query).to_string()),
            "case {case}: a refused query is not reported, got {reported:?}"
        );
    }
}

/// What the regex rule does NOT refuse: an expression that compiles, including the named
/// groups the rewriting renames and the constructs the expression engine reads.
#[tokio::test]
async fn an_expression_that_compiles_is_admitted_however_its_groups_are_named() {
    let logs = Logs::new();
    const GROUP: &str = "/compile-rules/regex-accepted";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string)
    let cases: Vec<(&str, &str)> = vec![
        (
            "one named group",
            r"fields @message | parse @message /lat=(?<lat>\d+)/",
        ),
        (
            "the Python spelling of a named group",
            r"fields @message | parse @message /lat=(?P<lat>\d+)/",
        ),
        (
            "two named groups and an unnamed one",
            r"fields @message | parse @message /(?<a>\w+) (\d+) (?<b>\w+)/",
        ),
        (
            "an alternation carrying the stage separator",
            r"fields @message | filter @message like /(WARN|ERROR)/",
        ),
        (
            // A look-around is a construct the service's expression engine reads, so an
            // expression using one is an expression.
            "a look-ahead",
            r"fields @message | parse @message /lat=(?=\d)(?<lat>\d+)/",
        ),
        (
            "a counted repetition",
            r"fields @message | parse @message /lat=(?<lat>\d{2,4})/",
        ),
        (
            "a character class",
            r"fields @message | filter @message like /[A-Z]+-[0-9]+/",
        ),
    ];

    for (case, query) in cases {
        let (status, body) = start(&logs, GROUP, window, query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the query to compile, got {status} with {body}"
        );
        assert!(
            body["queryId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case}: an accepted query is minted an id, got {body}"
        );
    }
}

/// A pipeline that names one ephemeral field twice over is refused with the duplicate-field
/// rule, the field's name and the span of the CLAUSE that declares it again, and the modelled
/// detail locates that clause too.
#[tokio::test]
async fn an_ephemeral_field_the_pipeline_already_carries_is_refused_by_the_rule() {
    let logs = Logs::new();
    const GROUP: &str = "/compile-rules/ephemeral";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string, the duplicated name, the clause that declares it a second time)
    // — the span is asserted from that clause's own place in the query, so a detail built from
    // a fixed string cannot pass. The rows span how the second definition is written: a glob
    // `as` clause, a named capture group, an aggregation's alias, a renaming item, and an item
    // of a `fields` projection, which declares the fields its stage produces.
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "a glob parse redefining a field another glob parse named",
            r#"fields @message | parse @message "lat=*" as lat | parse @message "lon=*" as lat"#
                .to_string(),
            "lat",
            r#"parse @message "lon=*" as lat"#.to_string(),
        ),
        (
            "a named capture group redefining a parsed field",
            r"parse @message /lat=(?<lat>\d+)/ | parse @message /lon=(?<lat>\d+)/".to_string(),
            "lat",
            r"parse @message /lon=(?<lat>\d+)/".to_string(),
        ),
        (
            "an aggregation naming a parsed field",
            r"parse @message /(?<num>\d+)/ | stats count(*) as num".to_string(),
            "num",
            "stats count(*) as num".to_string(),
        ),
        (
            "a fields item renaming an expression to a parsed field",
            r#"parse @message "svc=*" as svc | fields @message as svc"#.to_string(),
            "svc",
            "fields @message as svc".to_string(),
        ),
        (
            "a fields projection naming a parsed field",
            r#"filter @message like /mk/ | parse @message "svc=* lat=* status=*" as svc, lat, st | sort @timestamp asc | fields @message, lat"#
                .to_string(),
            "lat",
            "fields @message, lat".to_string(),
        ),
        (
            "a fields projection naming a field a capture group produced",
            r"parse @message /lat=(?<lat>\d+)/ | fields @timestamp, lat".to_string(),
            "lat",
            "fields @timestamp, lat".to_string(),
        ),
        (
            // The projection is read item by item, so the name does not have to be the first
            // item of the list for the stage to declare it.
            "a fields projection naming it last of several items",
            r"parse @message /lat=(?<lat>\d+)/ | fields @timestamp, @message, lat".to_string(),
            "lat",
            "fields @timestamp, @message, lat".to_string(),
        ),
        (
            "a lookup output naming a parsed field",
            format!(
                "{} | lookup {} user_id OUTPUT name AS tag",
                r#"parse @message "tag=*" as tag"#, "compile_rule_users"
            ),
            "tag",
            "lookup compile_rule_users user_id OUTPUT name AS tag".to_string(),
        ),
    ];

    logs.ok(
        "CreateLookupTable",
        &json!({
            "lookupTableName": "compile_rule_users",
            "tableBody": "user_id,name\nu1,Alice\n",
        }),
    )
    .await;

    for (case, query, field, clause) in &cases {
        let (said, body) = refused_with(&logs, case, GROUP, window, query).await;
        let at = query
            .rfind(clause.as_str())
            .unwrap_or_else(|| panic!("case {case}: the row carries the clause {clause:?}"));
        let last = at + clause.chars().count() - 1;
        assert_eq!(
            said,
            format!("Ephemeral field is already defined: {field} ([{at},{last}])"),
            "case {case}: expected the duplicate-field rule for {field:?} and the clause's own \
             span, got {said:?}"
        );
        // The detail travels in the structure the model declares for it, locating the same
        // clause inside the query.
        let location = &body["queryCompileError"]["location"];
        assert_eq!(
            (
                location["startCharOffset"].as_i64(),
                location["endCharOffset"].as_i64(),
            ),
            (
                Some(i64::try_from(at).unwrap_or(i64::MAX)),
                Some(i64::try_from(last + 1).unwrap_or(i64::MAX)),
            ),
            "case {case}: the compile error locates the clause at fault, got {body}"
        );
    }
}

/// What the ephemeral-field rule does NOT refuse: a stage that READS a field the pipeline
/// carries, a projection of the log event's own fields, and the same name in another query.
#[tokio::test]
async fn a_stage_that_only_reads_a_parsed_field_is_admitted() {
    let logs = Logs::new();
    const GROUP: &str = "/compile-rules/ephemeral-accepted";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string)
    let cases: Vec<(&str, &str)> = vec![
        (
            "a display of two parsed fields",
            r#"fields @message | parse @message "svc=* lat=*" as svc, lat | display svc, lat"#,
        ),
        (
            "a sort, a filter and a dedup over a parsed field",
            r"parse @message /lat=(?<lat>\d+)/ | filter lat > 1 | sort lat desc | dedup lat",
        ),
        (
            "an aggregation grouped by a parsed field",
            r#"parse @message "svc=* lat=*" as svc, lat | stats avg(lat) as mean by svc"#,
        ),
        (
            "a fields item computing a new name from a parsed field",
            r#"parse @message "lat=*" as lat | fields lat / 1000 as seconds | sort seconds desc"#,
        ),
        (
            // The log event's own fields are not declared by naming them, so a projection may
            // name one as often as the query likes.
            "a projection naming the same log fields twice over",
            "fields @timestamp, @message | sort @timestamp asc | fields @timestamp, @message",
        ),
        (
            "a parse defining a field another stage of the SAME name never defined",
            r#"parse @message "svc=*" as svc | parse @message "lat=*" as lat"#,
        ),
        (
            // Two queries, one name: a pipeline's ephemeral fields are its own.
            "the name another query already used",
            r#"parse @message "svc=*" as svc | display svc"#,
        ),
    ];

    for (case, query) in cases {
        let (status, body) = start(&logs, GROUP, window, query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the query to compile, got {status} with {body}"
        );
        assert!(
            body["queryId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case}: an accepted query is minted an id, got {body}"
        );
    }
}

/// A query whose OPENING word is no command of the language is refused at the stage separator
/// that separates nothing, and a query that opened with a command is refused at the word that
/// is no command.
#[tokio::test]
async fn a_query_that_opens_with_no_command_is_refused_at_its_stage_separator() {
    let logs = Logs::new();
    const GROUP: &str = "/compile-rules/opening";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string, the symbol the diagnostic must name, the text that symbol opens
    // in the query) — the place is asserted from that text's own offset, so a message built
    // from a fixed string cannot pass.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an opening stage that is no command, and a separator that separates nothing",
            "this is not | a valid query",
            "|",
            "| a valid query",
        ),
        (
            "the separator further along the opening stage",
            "nonsense words of no language here | nor here",
            "|",
            "| nor here",
        ),
        (
            // `||` is the OR operator of the language's expressions, not a stage separator, so
            // the symbol is the single `|` after it.
            "an OR operator before the separator",
            "nonsense || other | more nonsense",
            "|",
            "| more nonsense",
        ),
        (
            // A query string carries newlines as readily as a console does, and the place is
            // the LINE the symbol is on and its position within that line.
            "the separator on a line of its own",
            "nonsense words\n| nor these",
            "|",
            "| nor these",
        ),
        (
            // No separator at all: there is nothing but the opening word to stop on.
            "an opening word that is no command, and no separator",
            "shuffle @message",
            "shuffle",
            "shuffle @message",
        ),
        (
            // The pipeline DID open with a command, so the compiler read it and stops on the
            // word that is no command of the language.
            "a later stage that is no command",
            "fields @message | shuffle @message",
            "shuffle",
            "shuffle @message",
        ),
        (
            "a separator that separates nothing after a command",
            "fields @message | sort @timestamp | shuffle @message",
            "shuffle",
            "shuffle @message",
        ),
    ];

    for (case, query, symbol, opened) in &cases {
        let (said, _) = refused_with(&logs, case, GROUP, window, query).await;
        let at = query
            .find(opened)
            .unwrap_or_else(|| panic!("case {case}: the row carries the text {opened:?}"));
        let (line, position) = place(query, at);
        assert_eq!(
            said,
            format!("unexpected symbol found {symbol} at line {line} and position {position}"),
            "case {case}: expected the diagnostic to name {symbol:?} where it stands, got \
             {said:?}"
        );
    }
}
