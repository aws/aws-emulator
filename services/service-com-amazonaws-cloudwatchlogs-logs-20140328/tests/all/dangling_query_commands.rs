// SPDX-License-Identifier: CC0-1.0

//! A Logs Insights QL query that stops before a command's operand: what `StartQuery` does
//! with a pipeline whose last stage names a command and nothing for it to act on.
//!
//! RULE: a command of the query language is the command word AND the operand that command
//! acts on, so a query string that runs out of input before that operand does not compile —
//! `StartQuery` refuses it synchronously with 400 `MalformedQueryException`, the parser
//! diagnostic names the END OF INPUT as the symbol it stopped on at the place the string ran
//! out, no `queryId` is minted, and nothing of the refused query is left behind for
//! `DescribeQueries` to report. The diagnostic is stated inside the SDK envelope this
//! service's refusals carry, which reports the status and the code of the refusal and this
//! response's own request id.
//!
//! The rows span what the rule generalises over: WHICH command is left without its operand
//! (one row per command the language writes with one), whether the command is the whole query
//! or the last stage of a pipeline, whether blank space or a newline follows it, and where in
//! the string the incomplete command sits — which is what the reported position has to track.
//! The commands the language writes with NO operand, and the same commands given theirs, are
//! rows of the same tables, so the rule refuses what it must and nothing more.
//!
//! The counterpart suites: the accepted strings are in [`crate::query_compilation`], the
//! located character range in [`crate::scratch_queries`], and the diagnostic for the other
//! fault classes in [`crate::query_and_stream_refusal_text`].

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";

/// The modelled error code a query string that does not compile is refused under
/// (`smithy_query(query="operation",operation="StartQuery")` -> `MalformedQueryException`,
/// httpError 400).
const MALFORMED: &str = "MalformedQueryException";

/// What the diagnostic names as the symbol it stopped on when the query string ran out.
const END_OF_INPUT: &str = "<EOF>";

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

/// The `message` of a refusal body.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal carries a message, got {body}"))
        .to_string()
}

/// The diagnostic a refusal states: the SENTENCE of its message, with the SDK envelope this
/// service closes a refusal with stripped off, having asserted that envelope reports the
/// status, the code and the request id of this very response
/// ([`crate::harness::without_sdk_envelope`]).
fn diagnostic(case: &str, body: &Value) -> String {
    crate::harness::without_sdk_envelope(case, &message(case, body), 400, MALFORMED)
}

/// The place a parser states for the end of `query`: the line it ran out on, counted from
/// one, and the character position within that line — read off the query string here rather
/// than from the service, so a diagnostic that did not read the query cannot state it.
fn end_of_input_place(query: &str) -> (usize, usize) {
    let line = query.matches('\n').count() + 1;
    let position = query
        .rsplit_once('\n')
        .map_or(query, |(_, last)| last)
        .chars()
        .count();
    (line, position)
}

/// Every command the language writes with an operand is refused when the query ends before
/// that operand, and the refusal names the end of input at the place the string ran out.
#[tokio::test]
async fn a_command_the_query_ends_before_the_operand_of_is_refused_at_the_end_of_input() {
    let logs = Logs::new();
    const GROUP: &str = "/dangling/commands";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string) — one row per command the query syntax writes with an operand,
    // left without it at the end of the string: the fields `fields`, `display` and `dedup`
    // name, the expression `filter` selects by, the aggregation `stats` computes, the field
    // `sort` orders by and the one `pattern` clusters, the pattern `parse` extracts with, the
    // field `unmask` reveals, the count `limit` keeps, the table `lookup` joins, and the log
    // groups `SOURCE` selects.
    let cases: Vec<(&str, String)> = vec![
        ("a trailing sort", "fields @message | sort".to_string()),
        ("a trailing stats", "fields @message | stats".to_string()),
        ("a trailing fields", "fields @message | fields".to_string()),
        ("a trailing filter", "fields @message | filter".to_string()),
        ("a trailing display", "fields @message | display".to_string()),
        ("a trailing parse", "fields @message | parse".to_string()),
        ("a trailing pattern", "fields @message | pattern".to_string()),
        ("a trailing dedup", "fields @message | dedup".to_string()),
        ("a trailing unmask", "fields @message | unmask".to_string()),
        ("a trailing limit", "fields @message | limit".to_string()),
        ("a trailing lookup", "fields @message | lookup".to_string()),
        ("the whole query is one command", "fields".to_string()),
        ("the whole query is SOURCE", "SOURCE".to_string()),
        (
            "a trailing command spelled in upper case",
            "fields @message | SORT".to_string(),
        ),
        (
            "blank space after the trailing command",
            "fields @message | sort   ".to_string(),
        ),
        (
            "the trailing command on a line of its own",
            "fields @message\n| sort".to_string(),
        ),
        (
            "a longer pipeline, so the end of input moves",
            "fields @timestamp, @message | filter @message like /ERROR/ | limit 10 | sort"
                .to_string(),
        ),
        (
            // The query the issue's client assembled: the operand of the last stage was lost.
            "a filter over a quoted marker and then a bare sort",
            "fields @message | filter @message like 'mk-1234' | sort".to_string(),
        ),
    ];
    for (case, query_string) in &cases {
        let (status, body) = start(&logs, GROUP, window, query_string).await;
        assert_eq!(
            status, 400,
            "case {case}: an incomplete command is refused, got {status} with {body}"
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
        let (line, position) = end_of_input_place(query_string);
        assert_eq!(
            diagnostic(case, &body),
            format!("unexpected symbol found {END_OF_INPUT} at line {line} and position {position}"),
            "case {case}: expected the end-of-input diagnostic for {query_string:?}, got {body}"
        );
        // "Details about this error are displayed in a QueryCompileError object": the detail
        // locates the place the compiler stopped, which is the end of the string.
        let location = &body["queryCompileError"]["location"];
        let characters = i64::try_from(query_string.chars().count()).unwrap_or(i64::MAX);
        assert_eq!(
            (
                location["startCharOffset"].as_i64(),
                location["endCharOffset"].as_i64(),
            ),
            (Some(characters), Some(characters)),
            "case {case}: the compile error locates the end of the query, got {body}"
        );
        assert!(
            !body["queryCompileError"]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case}: the compile error says what is wrong, got {body}"
        );
    }

    // Nothing of a refused query is left behind: the store holds no query at all, so none of
    // the refused strings can be reported.
    let page = logs.ok(DESCRIBE, &json!({"logGroupName": GROUP})).await;
    let reported: Vec<String> = page["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueries reports a queries list, got {page}"))
        .iter()
        .map(|query| query["queryString"].as_str().unwrap_or_default().to_string())
        .collect();
    for (case, query_string) in &cases {
        assert!(
            !reported.contains(query_string),
            "case {case}: a refused query is not reported, got {reported:?}"
        );
    }
}

/// Where the operand was owed but the query did NOT end there, the diagnostic names the
/// symbol that stands in its place rather than the end of input: the stage separator opening
/// the next command of the pipeline.
#[tokio::test]
async fn a_command_with_no_operand_mid_pipeline_names_the_symbol_that_stands_there() {
    let logs = Logs::new();
    const GROUP: &str = "/dangling/mid-pipeline";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string, the command left without its operand) — the incomplete
    // command is followed by the `|` that opens the next stage, at a different place in each
    // row.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("the first stage", "fields | sort @timestamp", "fields"),
        (
            "the second stage",
            "fields @message | sort | limit 10",
            "sort",
        ),
        (
            "the second stage of a longer pipeline",
            "fields @timestamp, @message | stats | sort @timestamp | limit 5",
            "stats",
        ),
        (
            // The separator written against the next command: one character is one token, so
            // the diagnostic names the separator and not the word behind it.
            "a separator with no blank space after it",
            "fields |sort @timestamp",
            "fields",
        ),
    ];
    for (case, query_string, command) in cases {
        let (status, body) = start(&logs, GROUP, window, query_string).await;
        assert_eq!(
            status, 400,
            "case {case}: an incomplete command is refused, got {status} with {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, MALFORMED,
            "case {case}: expected {MALFORMED}, got {code} with {body}"
        );
        // The separator that follows the operand-less command, read off the query string here
        // rather than restated per row.
        let after = query_string
            .find(command)
            .map(|at| at + command.len())
            .unwrap_or_else(|| panic!("case {case}: the row carries the command {command:?}"));
        let at = after
            + query_string[after..]
                .find('|')
                .unwrap_or_else(|| panic!("case {case}: the row carries a stage separator"));
        assert_eq!(
            diagnostic(case, &body),
            format!("unexpected symbol found | at line 1 and position {at}"),
            "case {case}: expected the separator to be named as the symbol, got {body}"
        );
        // … and the located range covers exactly that one character.
        let location = &body["queryCompileError"]["location"];
        let at = i64::try_from(at).unwrap_or(i64::MAX);
        assert_eq!(
            (
                location["startCharOffset"].as_i64(),
                location["endCharOffset"].as_i64(),
            ),
            (Some(at), Some(at + 1)),
            "case {case}: the compile error locates the symbol it stopped on, got {body}"
        );
    }
}

/// The reported position TRACKS the token the compiler stopped on: two queries that differ
/// only by how far along the incomplete command sits are refused at positions that differ by
/// exactly that distance.
#[tokio::test]
async fn the_reported_position_moves_with_the_incomplete_command() {
    let logs = Logs::new();
    const GROUP: &str = "/dangling/positions";
    let window = group_with_window(&logs, GROUP).await;

    // One incomplete command, reached through pipelines of growing length: each prefix moves
    // the end of input by exactly its own number of characters.
    let prefixes = [
        "fields @message | ",
        "fields @timestamp, @message | ",
        "fields @message | filter @message like /ERROR/ | ",
        "fields @message | filter @message like /ERROR/ | limit 10 | ",
    ];
    let mut first_position: Option<usize> = None;
    let mut first_length = 0usize;
    for prefix in prefixes {
        let query_string = format!("{prefix}sort");
        let case = format!("the prefix {prefix:?}");
        let (status, body) = start(&logs, GROUP, window, &query_string).await;
        assert_eq!(
            status, 400,
            "case {case}: an incomplete command is refused, got {status} with {body}"
        );
        let said = diagnostic(&case, &body);
        let opening = format!("unexpected symbol found {END_OF_INPUT} at line 1 and position ");
        let position = said
            .strip_prefix(&opening)
            .and_then(|tail| tail.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("case {case}: expected {opening:?} and a position, got {said:?}"));
        let length = query_string.chars().count();
        match first_position {
            None => {
                first_position = Some(position);
                first_length = length;
            }
            Some(first) => assert_eq!(
                position - first,
                length - first_length,
                "case {case}: the position moves with the offending token, got {position} \
                 against {first}"
            ),
        }
    }
}

/// What the rule does NOT refuse: the same commands given their operand, and the one command
/// the query syntax writes on its own.
#[tokio::test]
async fn a_command_given_its_operand_still_compiles() {
    let logs = Logs::new();
    const GROUP: &str = "/dangling/accepted";
    let window = group_with_window(&logs, GROUP).await;
    logs.ok(
        "CreateLookupTable",
        &json!({
            "lookupTableName": "dangling_users",
            "tableBody": "user_id,name\nu1,Alice\n",
        }),
    )
    .await;

    // (case, the query string) — the operand present, in each form the language writes it,
    // plus `diff`, which the syntax writes with no operand at all.
    let cases: Vec<(&str, &str)> = vec![
        ("sort with a field", "fields @message | sort @timestamp desc"),
        ("stats with an aggregation", "fields @message | stats count(*)"),
        ("fields with a field list", "fields @timestamp, @message"),
        ("filter with an expression", "fields @message | filter @message like /ERROR/"),
        ("display with a field", r#"parse @message "* *" as a, b | display a"#),
        ("parse with a pattern", r"fields @message | parse @message /(?<level>[A-Z]+)/"),
        ("pattern with a field", "fields @message | pattern @message"),
        ("dedup with a field", "fields @message | dedup @message"),
        ("unmask with a field", "fields @message | unmask @message"),
        ("limit with a count", "fields @message | limit 10"),
        ("lookup with a table", "fields @message | lookup dangling_users user_id"),
        (
            "SOURCE with a selection",
            r#"SOURCE logGroups(namePrefix: ["/dangling/"]) | fields @message"#,
        ),
        // `diff` takes no operand: it compares this stage's records with the previous period.
        ("diff, which takes no operand", "fields @message | diff"),
    ];
    for (case, query_string) in cases {
        let (status, body) = start(&logs, GROUP, window, query_string).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the query to compile, got {status} with {body}"
        );
        let id = body["queryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: an accepted start answers a queryId, got {body}"));
        assert!(
            !id.is_empty(),
            "case {case}: the minted queryId is not empty, got {body}"
        );
    }
}
