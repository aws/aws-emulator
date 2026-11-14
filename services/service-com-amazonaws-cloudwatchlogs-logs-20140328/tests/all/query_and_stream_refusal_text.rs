// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT CloudWatch Logs refuses a query, a query definition, a
//! log-stream read and a pagination token with.
//!
//! The rule under test: a refusal's message is AWS's own wording for that error class —
//! its fixed text exactly, with its variable parts taken from the request and from the
//! store — and never a sentence of this emulator's own. Each test below pins one class:
//!
//! * a `queryId` this account holds no query under is INVALID, and the refusal names the
//!   id the request sent;
//! * a query definition id nothing is stored under is refused with the class's fixed
//!   sentence inside the SDK error envelope, whose only per-run field is the request id;
//! * ordering log streams by event time together with a name prefix is refused with the
//!   service's fixed sentence for the combination;
//! * a `nextToken` a read cannot resume from is refused with one fixed sentence, whichever
//!   read and whichever way the token fails to place;
//! * a query string that does not compile is refused with the COMPILER'S diagnostic, so a
//!   caller can tell where the query is wrong;
//! * a SQL statement filtering its groups on an aggregate function expression is refused with
//!   the compiler's rule, the offending predicate as the pipeline the statement translates to
//!   states it, and the RANGE that clause occupies in the statement the compiler read;
//! * a `logGroupNamePattern` outside the accepted grammar is refused with the constraint
//!   template stating the expression the service publishes.
//!
//! Every table spans what its rule generalises over rather than the single recorded
//! request: the id and token forms, the operations that resolve each kind of value, and
//! the ways a query string and a pattern can be malformed. Every case drives the service
//! the way a client does — `POST /` with `X-Amz-Target: Logs_20140328.<Operation>`
//! through the router that mounts it — so a response a handler does not compute cannot
//! pass.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    INVALID_NEXT_TOKEN_MESSAGE, LOG_GROUP_NAME_REGEX_PATTERN,
};

use crate::harness::{
    base_ms, batch, envelope_request_id, group_with_stream, now_ms, without_sdk_envelope, Logs,
};

/// The modelled codes these refusals are answered under.
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_PARAMETER: &str = "InvalidParameterException";
const MALFORMED_QUERY: &str = "MalformedQueryException";

/// A query string that compiles, so a start below is never refused over its syntax.
const VALID_QUERY: &str = "fields @message";

/// The message a refusal reports, whichever casing the envelope carries it under.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal carries a message, got {body}"))
        .to_string()
}

/// One call that must be refused with HTTP 400, returning its code and message.
async fn refusal(logs: &Logs, case: &str, operation: &str, body: &Value) -> (String, String) {
    let (status, parsed) = logs.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: {operation} {body} must be refused with HTTP 400, got {status} {parsed}"
    );
    (
        crate::harness::error_code(&parsed),
        message(case, &parsed),
    )
}

/// A log group plus a query over it, answering the `queryId` the start minted.
async fn started_query(logs: &Logs, group: &str) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let now_secs = now_ms() / 1_000;
    let answer = logs
        .ok(
            "StartQuery",
            &json!({
                "logGroupName": group,
                "startTime": now_secs - 3_600,
                // Rounded UP to the next whole second, so the range reaches the instant the
                // group was just created: one closing before that covers no instant of the
                // group's lifetime.
                "endTime": now_secs + 1,
                "queryString": VALID_QUERY,
            }),
        )
        .await;
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("a start answers a queryId, got {answer}"))
        .to_string()
}

/// A `queryId` this account holds no query under is refused as the missing resource it is,
/// in a sentence that names the id the request sent — for every form the member admits.
#[tokio::test]
async fn a_queryid_no_query_is_stored_under_is_refused_naming_the_id() {
    let logs = Logs::new();
    let stored = started_query(&logs, "/stop-query/text").await;

    // (case, the id) — `StopQuery$queryId` is modelled `len 1..256`, so both published
    // bounds reach the handler, as do a canonical UUID this store never minted and ids
    // that differ from the stored one by a single character.
    let cases: Vec<(&str, String)> = vec![
        ("an id at the published minimum length", "q".to_string()),
        ("an id at the published maximum length", "q".repeat(256)),
        (
            "the all-zero UUID a client sends for an unknown query",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a canonical UUID this store never minted",
            "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
        ),
        (
            "the stored id with one character appended",
            format!("{stored}0"),
        ),
        (
            "a prefix of the stored id",
            stored[..stored.len() - 1].to_string(),
        ),
        (
            "an id carrying characters no minted id uses",
            "Query Id/With Spaces".to_string(),
        ),
    ];

    for (case, id) in cases {
        let (code, said) = refusal(&logs, case, "StopQuery", &json!({"queryId": id})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: an id naming no query is a not-found, got {code} {said:?}"
        );
        assert_eq!(
            said,
            format!("The provided queryId={id} is invalid."),
            "case {case}: the refusal names the id it rejected, got {said:?}"
        );
    }

    // The positive control: the id this account DOES hold a query under is stopped, and
    // the refusal above is therefore about the id and not about the operation.
    let answer = logs.ok("StopQuery", &json!({"queryId": &stored})).await;
    assert_eq!(
        answer["success"].as_bool(),
        Some(true),
        "case the stored query: the id this account holds is stopped, got {answer}"
    );
}

/// A query definition id nothing is stored under is refused with the class's fixed
/// sentence plus the SDK error envelope, whose status and code are the ones the response
/// itself answers with and whose request id is the one per-run field — the same for every
/// operation that addresses a definition by id.
#[tokio::test]
async fn a_query_definition_id_that_is_not_stored_is_refused_in_the_services_wording() {
    let logs = Logs::new();
    // One definition that DOES exist, so a refusal below is about the id it names rather
    // than about an empty store.
    let saved = logs
        .ok(
            "PutQueryDefinition",
            &json!({"name": "kept", "queryString": VALID_QUERY}),
        )
        .await;
    let stored = saved["queryDefinitionId"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers a queryDefinitionId, got {saved}"))
        .to_string();

    let absent: Vec<(&str, String)> = vec![
        (
            "a canonical UUID this store never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "the stored id with one character appended",
            format!("{stored}0"),
        ),
        (
            "a prefix of the stored id",
            stored[..stored.len() - 1].to_string(),
        ),
        ("an id that is not a UUID at all", "not-a-definition".to_string()),
    ];

    let expected_text = "Query definition id is invalid.";
    let mut seen_request_ids: Vec<String> = Vec::new();
    for (case, id) in absent {
        // Both operations that resolve a `queryDefinitionId`: the delete, and the put
        // that updates an existing definition.
        for (operation, body) in [
            ("DeleteQueryDefinition", json!({"queryDefinitionId": id})),
            (
                "PutQueryDefinition",
                json!({
                    "queryDefinitionId": id,
                    "name": "updated",
                    "queryString": VALID_QUERY,
                }),
            ),
        ] {
            let case = format!("{operation}: {case}");
            let (code, said) = refusal(&logs, &case, operation, &body).await;
            assert_eq!(
                code, NOT_FOUND,
                "case {case}: an id naming no definition is a not-found, got {code} {said:?}"
            );
            let request_id = envelope_request_id(&case, &said);
            assert_eq!(
                said,
                format!(
                    "{expected_text} (Service: AWSLogs; Status Code: 400; Error Code: \
                     {NOT_FOUND}; Request ID: {request_id}; Proxy: null)"
                ),
                "case {case}: expected the class's fixed sentence in the SDK envelope, got \
                 {said:?}"
            );
            // The sentence is FIXED: it carries nothing of the request, not even the id.
            assert!(
                !said.contains(&id),
                "case {case}: the fixed sentence carries no submitted value, got {said:?}"
            );
            assert!(
                !seen_request_ids.contains(&request_id),
                "case {case}: every refusal carries its own request id, got the \
                 already-reported {request_id:?}"
            );
            seen_request_ids.push(request_id);
        }
    }

    // The definition this account does hold is untouched by every refused call.
    let described = logs.ok("DescribeQueryDefinitions", &json!({})).await;
    let names: Vec<String> = described["queryDefinitions"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueryDefinitions reports a list, got {described}"))
        .iter()
        .map(|definition| definition["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["kept".to_string()],
        "case the stored definition: a refused call changes nothing, got {names:?}"
    );
}

/// Ordering log streams by event time together with a name prefix is refused with the
/// service's fixed sentence for the combination, whatever the prefix and whatever
/// direction was asked for — and each half of the combination on its own is answered.
#[tokio::test]
async fn ordering_streams_by_event_time_with_a_prefix_is_refused_in_the_services_wording() {
    let logs = Logs::new();
    const GROUP: &str = "/describe-streams/order";
    group_with_stream(&logs, GROUP, "alpha-one", None).await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": "beta-two"}),
    )
    .await;

    let expected = "Cannot order by LastEventTime with a logStreamNamePrefix.";
    // The prefixes span what the rule generalises over: one that matches a stream, one
    // that matches nothing, a one-character prefix and the longest `LogStreamName` the
    // model admits (`@length` 1..512) — the rule is about the COMBINATION, not the value.
    let prefixes = [
        "alpha".to_string(),
        "zzz-no-such-stream".to_string(),
        "a".to_string(),
        "p".repeat(512),
    ];
    for prefix in prefixes {
        for descending in [None, Some(true), Some(false)] {
            // Both selectors a request names the group by, so the refusal is not about
            // which one was used.
            for selector in ["logGroupName", "logGroupIdentifier"] {
                let case = format!("prefix {prefix:?}, descending {descending:?}, {selector}");
                let mut request = json!({
                    selector: GROUP,
                    "orderBy": "LastEventTime",
                    "logStreamNamePrefix": prefix,
                });
                if let Some(descending) = descending {
                    request["descending"] = json!(descending);
                }
                let (code, said) = refusal(&logs, &case, "DescribeLogStreams", &request).await;
                assert_eq!(
                    code, INVALID_PARAMETER,
                    "case {case}: the combination is an incorrectly specified parameter, got \
                     {code} {said:?}"
                );
                assert_eq!(
                    said, expected,
                    "case {case}: expected {expected:?}, got {said:?}"
                );
            }
        }
    }

    // Each half on its own is answered: the prefix alone, and the ordering alone.
    for (case, request) in [
        (
            "the prefix alone",
            json!({"logGroupName": GROUP, "logStreamNamePrefix": "alpha"}),
        ),
        (
            "the ordering alone",
            json!({"logGroupName": GROUP, "orderBy": "LastEventTime"}),
        ),
    ] {
        let page = logs.ok("DescribeLogStreams", &request).await;
        assert!(
            page.get("logStreams").and_then(Value::as_array).is_some(),
            "case {case}: half of the combination is answered, got {page}"
        );
    }
}

/// Every paginated read refuses a `nextToken` it cannot resume the request from with the
/// service's ONE fixed sentence — whichever read, and whichever way the token fails to
/// place: never minted here, not decodable, minted by another operation, or minted for a
/// different enumeration of the same operation.
#[tokio::test]
async fn a_nexttoken_a_read_cannot_resume_from_is_refused_with_one_sentence() {
    let logs = Logs::new();
    const GROUP: &str = "/next-token/one";
    const OTHER_GROUP: &str = "/next-token/two";
    group_with_stream(&logs, GROUP, "stream-a", None).await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": "stream-b"}),
    )
    .await;
    group_with_stream(&logs, OTHER_GROUP, "stream-a", None).await;
    let base = base_ms();
    for stream in ["stream-a", "stream-b"] {
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": stream,
                "logEvents": batch(&[(base, "one"), (base + 1_000, "two")]),
            }),
        )
        .await;
    }

    // The forms a token takes that this service cannot place: not its envelope at all,
    // empty-ish, hex that decodes to another operation's cursor kind, and a long run of
    // text.
    let unusable = [
        ("a token this service never minted", "not-a-token".to_string()),
        ("a single character", "x".to_string()),
        (
            "a cursor minted under another kind",
            "7b226b696e64223a224f74686572227d".to_string(),
        ),
        ("a long run of text", "z".repeat(300)),
    ];

    // The reads that take a `nextToken`, each with the members it needs to get as far as
    // the token.
    let reads: Vec<(&str, Value)> = vec![
        (
            "GetLogEvents",
            json!({"logGroupName": GROUP, "logStreamName": "stream-a"}),
        ),
        ("DescribeLogStreams", json!({"logGroupName": GROUP})),
        ("DescribeLogGroups", json!({})),
        ("ListLogGroups", json!({})),
        ("DescribeQueries", json!({})),
        ("DescribeQueryDefinitions", json!({})),
        ("DescribeExportTasks", json!({})),
        (
            "ListAggregateLogGroupSummaries",
            json!({"groupBy": "DATA_SOURCE_NAME_AND_TYPE"}),
        ),
    ];

    for (operation, body) in &reads {
        for (form, token) in &unusable {
            let case = format!("{operation}: {form}");
            let mut request = body.clone();
            request["nextToken"] = json!(token);
            let (code, said) = refusal(&logs, &case, operation, &request).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case}: an unusable token is an incorrectly specified parameter, got \
                 {code} {said:?}"
            );
            assert_eq!(
                said, INVALID_NEXT_TOKEN_MESSAGE,
                "case {case}: expected {INVALID_NEXT_TOKEN_MESSAGE:?}, got {said:?}"
            );
        }
    }

    // A token this service DID mint, handed back for a different enumeration: a page of
    // one log group's streams continued against another group, and one stream's events
    // continued against another stream. Both are tokens, and neither resumes the request
    // carrying it.
    let page = logs
        .ok(
            "DescribeLogStreams",
            &json!({"logGroupName": GROUP, "limit": 1}),
        )
        .await;
    let streams_token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page with more to come hands back a token, got {page}"))
        .to_string();
    let events = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": "stream-a", "limit": 1}),
        )
        .await;
    let events_token = events["nextForwardToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page of events hands back a forward token, got {events}"))
        .to_string();

    for (case, operation, request) in [
        (
            "a streams token continued against another log group",
            "DescribeLogStreams",
            json!({"logGroupName": OTHER_GROUP, "limit": 1, "nextToken": streams_token}),
        ),
        (
            "an events token continued against another log stream",
            "GetLogEvents",
            json!({
                "logGroupName": GROUP,
                "logStreamName": "stream-b",
                "nextToken": events_token,
            }),
        ),
    ] {
        let (code, said) = refusal(&logs, case, operation, &request).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: a token only resumes the enumeration it came from, got {code} \
             {said:?}"
        );
        assert_eq!(
            said, INVALID_NEXT_TOKEN_MESSAGE,
            "case {case}: expected {INVALID_NEXT_TOKEN_MESSAGE:?}, got {said:?}"
        );
    }
}

/// A query string that does not compile is refused with the service's PARSER DIAGNOSTIC: the
/// message names the symbol the compiler stopped on and where that symbol sits in the
/// submitted string, so two different faults do not read alike and a caller can tell where
/// the query is wrong. The diagnostic is stated inside the SDK envelope of the refusal it is
/// answered under, the way every message of this service that carries one states it.
#[tokio::test]
async fn a_query_that_does_not_compile_is_refused_with_the_parsers_diagnostic() {
    let logs = Logs::new();
    const GROUP: &str = "/start-query/diagnostics";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let now_secs = now_ms() / 1_000;

    // (case, the query string, the symbol the diagnostic must name) — the position is
    // asserted below from the symbol's own place in each string, so a message built from a
    // fixed string cannot pass.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a command the language does not define", "shuffle @message", "shuffle"),
        (
            "the same command further along the pipeline, so the position moves",
            "fields @message | shuffle @message",
            "shuffle",
        ),
        (
            "the same command further along still",
            "fields @message | sort @timestamp | shuffle @message",
            "shuffle",
        ),
        (
            "a stage left empty by a trailing pipe, where the query ends",
            "fields @message | ",
            "<EOF>",
        ),
        (
            "a limit that is not a number of log events",
            "fields @message | limit none",
            "none",
        ),
        (
            "a parse regex that does not compile",
            r"fields @message | parse @message /+/",
            "/+/",
        ),
        (
            "a field defined twice over by two parse commands",
            r#"fields @message | parse @message "lat=*" as lat | parse @message "lon=*" as lat"#,
            "lat",
        ),
        (
            // A query string carries newlines as readily as a console does, and the place
            // a diagnostic states is the LINE and the position within it.
            "a command the language does not define, on the third line",
            "fields @message\n| sort @timestamp\n| shuffle @message",
            "shuffle",
        ),
        ("an empty query string, which ends at once", "", "<EOF>"),
    ];

    for (case, query_string, symbol) in cases {
        let (status, body) = logs
            .call(
                "StartQuery",
                &json!({
                    "logGroupName": GROUP,
                    "startTime": now_secs - 3_600,
                    "endTime": now_secs,
                    "queryString": query_string,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a query that does not compile is refused, got {status} {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, MALFORMED_QUERY,
            "case {case}: expected {MALFORMED_QUERY}, got {code} with {body}"
        );
        // The envelope reports the status and the code the response itself answers with, and
        // this response's own request id; the diagnostic is what it wraps.
        let said = without_sdk_envelope(case, &message(case, &body), 400, MALFORMED_QUERY);
        // The symbol the compiler stopped on is its LAST occurrence in each row's query
        // string (for the redefinition row the first one is the definition), so the line
        // and position below are read off the query rather than restated per row.
        match query_string.rfind(symbol) {
            // The place is the symbol's own line, counted from one, and its character
            // position within that line, counted from zero — the way a parser reports one —
            // so a diagnostic that did not read the query cannot state it.
            Some(byte) => {
                let before = &query_string[..byte];
                let line = before.matches('\n').count() + 1;
                let position = before
                    .rsplit_once('\n')
                    .map_or(before, |(_, last_line)| last_line)
                    .chars()
                    .count();
                assert_eq!(
                    said,
                    format!(
                        "unexpected symbol found {symbol} at line {line} and position {position}"
                    ),
                    "case {case}: expected the parser's diagnostic for {symbol:?}, got {said:?}"
                );
            }
            // The end of the query is not a text the query carries: the diagnostic names it
            // as such, at a position inside the string it ran out of.
            None => {
                let opening = format!("unexpected symbol found {symbol} at line 1 and position ");
                let position = said
                    .strip_prefix(&opening)
                    .and_then(|tail| tail.parse::<usize>().ok())
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected {opening:?} and a position, got {said:?}")
                    });
                assert!(
                    position <= query_string.chars().count(),
                    "case {case}: the position is inside the submitted string, got {said:?}"
                );
            }
        }
        // The detail travels in the structure the model declares for it, locating the same
        // text inside the query.
        let location = &body["queryCompileError"]["location"];
        let (start, end) = (
            location["startCharOffset"].as_i64(),
            location["endCharOffset"].as_i64(),
        );
        assert!(
            start.is_some()
                && end.is_some()
                && start <= end
                && end.unwrap_or_default() <= query_string.chars().count() as i64,
            "case {case}: the compile error locates the text at fault inside the query, got \
             {body}"
        );
        assert!(
            !body["queryCompileError"]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case}: the compile error says what is wrong, got {body}"
        );
        // A generic sentence carries no diagnostic, so it is not what this class answers.
        assert_ne!(
            said, "The query string is not valid.",
            "case {case}: a single generic sentence is not a parser diagnostic"
        );
    }
}

/// A `logGroupNamePattern` outside the accepted grammar is refused with the constraint
/// template stating the expression the SERVICE publishes for the member: unanchored, and
/// including the single-pattern alternative the model's segments alone would exclude.
#[tokio::test]
async fn an_invalid_log_group_name_pattern_states_the_published_expression() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "/patterns/one"}))
        .await;

    // The expression the service publishes for `LogGroupNameRegexPattern`, written out
    // here rather than read from the implementation, so a change to the crate's own
    // constant makes these assertions fail: stated without the `^`…`$` anchors the model's
    // trait string carries, and carrying the `{3,512}` single-pattern alternative the
    // service accepts beside the model's `{3,24}` segments.
    const PUBLISHED_EXPRESSION: &str = concat!(
        r"(\^?[\.\-_\/#A-Za-z0-9]{3,512})",
        r"|(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}"
    );
    assert_eq!(
        LOG_GROUP_NAME_REGEX_PATTERN, PUBLISHED_EXPRESSION,
        "the crate states the published expression, got {LOG_GROUP_NAME_REGEX_PATTERN}"
    );
    assert!(
        !PUBLISHED_EXPRESSION.starts_with('^') && !PUBLISHED_EXPRESSION.ends_with('$'),
        "the published expression is unanchored, got {PUBLISHED_EXPRESSION}"
    );

    // Every shape of value the grammar excludes, beyond the member's own length bounds.
    let patterns = [
        ("six alternatives, one past the five the member allows", "aaa|bbb|ccc|ddd|eee|fff"),
        ("ten alternatives", "aaa|bbb|ccc|ddd|eee|fff|ggg|hhh|iii|jjj"),
        ("a character outside the documented class", "aws lambda"),
        ("a regular-expression metacharacter the class excludes", "^/aws/.*"),
        ("an alternative shorter than three characters", "/aws/lambda|ab"),
        ("an empty alternative", "/aws/lambda|"),
        ("an anchor with fewer than three characters after it", "^ab"),
    ];

    for operation in ["ListLogGroups", "ListAggregateLogGroupSummaries"] {
        for (case, pattern) in patterns {
            let case = format!("{operation}: {case}");
            let mut request = json!({"logGroupNamePattern": pattern});
            if operation == "ListAggregateLogGroupSummaries" {
                request["groupBy"] = json!("DATA_SOURCE_NAME_AND_TYPE");
            }
            let (code, said) = refusal(&logs, &case, operation, &request).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case}: a pattern outside the grammar is refused, got {code} {said:?}"
            );
            assert_eq!(
                said,
                format!(
                    "1 validation error detected: Value at 'logGroupNamePattern' failed to \
                     satisfy constraint: Member must satisfy regular expression pattern: \
                     {PUBLISHED_EXPRESSION}"
                ),
                "case {case}: expected the constraint template stating the published \
                 expression, got {said:?}"
            );
            assert!(
                !said.contains(pattern),
                "case {case}: the template carries no submitted value, got {said:?}"
            );
        }
    }
}

/// The clause keyword the rows below spell in UPPER CASE, so a lower-case occurrence of the
/// same text inside a log group's name or a compared value is not the clause.
const HAVING: &str = "HAVING";

/// The statement the compiler reads, derived here the way the service derives it rather than
/// read from the implementation: a `FROM` clause naming one log group directly is the short
/// way of writing the `logGroups(logGroupIdentifier: [...])` selector a query's source is
/// named by (`StartQuery`'s own documentation for the `SOURCE` command, which "can select log
/// groups based on log group name prefix, account ID, and log class"), so the written-out
/// selector stands in its place; a source already written as that call stands as it is.
fn statement_the_compiler_reads(case: &str, query_string: &str) -> String {
    let (before, rest) = query_string
        .split_once('`')
        .unwrap_or_else(|| panic!("case {case}: the statement quotes its source clause"));
    let (named, after) = rest
        .split_once('`')
        .unwrap_or_else(|| panic!("case {case}: the quoted source clause closes"));
    if named.trim_start().starts_with("logGroups(") {
        return query_string.to_string();
    }
    format!("{before}`logGroups(logGroupIdentifier: ['{named}'])`{after}")
}

/// The range the `HAVING` clause occupies in `statement`: the first character of its keyword
/// and the last character of the condition after it, which ends where the clause following it
/// begins. CHARACTER offsets — every statement below is ASCII, so each byte is one character.
fn clause_range(case: &str, statement: &str) -> (usize, usize) {
    let at = statement
        .find(HAVING)
        .unwrap_or_else(|| panic!("case {case}: the statement carries a {HAVING} clause"));
    let clause = &statement[at..];
    let clause = clause.split(" ORDER BY").next().unwrap_or(clause).trim_end();
    let start = statement[..at].chars().count();
    (start, start + clause.chars().count() - 1)
}

/// A SQL statement whose `HAVING` clause filters its groups on an aggregate function
/// expression does not compile, and the refusal states the RULE, the offending predicate as
/// the pipeline the statement translates to states it, and the RANGE that clause occupies in
/// the statement the compiler read — so a caller can point at the clause at fault.
///
/// The rows span what the rule generalises over: which aggregate the condition calls, how long
/// the source name before the clause is (which moves the whole range), whether the source is
/// written short or as the selector call it abbreviates, whether a further clause ends the
/// condition, and whether a name or a compared value carries the clause keyword itself.
#[tokio::test]
async fn an_aggregate_inside_a_sql_filter_is_refused_with_the_rule_and_the_clauses_range() {
    let logs = Logs::new();
    const GROUP: &str = "/sql/filter";
    const LONGER_GROUP: &str = "/sql/filter-on-an-aggregate-expression-0123456789";
    const KEYWORD_GROUP: &str = "/sql/having-in-the-name";
    for group in [GROUP, LONGER_GROUP, KEYWORD_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    let now_secs = now_ms() / 1_000;

    // (case, the statement, the predicate the pipeline the statement translates to states the
    // condition with: the filter command and the condition, its spacing removed and the name
    // of every function call lowercased)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a count of the rows of each group",
            format!("SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc HAVING COUNT(*) > 1"),
            "filtercount(*)>1",
        ),
        (
            "a sum over a field",
            format!(
                "SELECT svc, SUM(ms) AS total FROM `{GROUP}` GROUP BY svc HAVING SUM(ms) > 50"
            ),
            "filtersum(ms)>50",
        ),
        (
            "a longer source name, which moves the clause along",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{LONGER_GROUP}` GROUP BY svc \
                 HAVING COUNT(*) > 1"
            ),
            "filtercount(*)>1",
        ),
        (
            "an aggregate beside a condition over the group key",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc \
                 HAVING svc = 'a' AND MAX(ms) > 1"
            ),
            // Removing the spacing runs the operator into the call that follows it, and the
            // pipeline writes a call's name lowercased.
            "filtersvc='a'andmax(ms)>1",
        ),
        (
            "an ordering after the clause, which ends its condition",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc HAVING COUNT(*) > 1 \
                 ORDER BY svc"
            ),
            "filtercount(*)>1",
        ),
        (
            "a source already written as the selector call",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `logGroups(logGroupIdentifier: ['{GROUP}'])` \
                 GROUP BY svc HAVING COUNT(*) > 1"
            ),
            "filtercount(*)>1",
        ),
        (
            "a source name that carries the clause keyword",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{KEYWORD_GROUP}` GROUP BY svc \
                 HAVING COUNT(*) > 1"
            ),
            "filtercount(*)>1",
        ),
        (
            "a compared value that carries the clause keyword",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{GROUP}` WHERE run = 'having' GROUP BY svc \
                 HAVING COUNT(*) > 1"
            ),
            "filtercount(*)>1",
        ),
    ];

    for (case, query_string, predicate) in cases {
        let (status, body) = logs
            .call(
                "StartQuery",
                &json!({
                    "queryLanguage": "SQL",
                    "queryString": query_string,
                    "startTime": now_secs - 3_600,
                    "endTime": now_secs,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a filter on an aggregate does not compile, got {status} {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, MALFORMED_QUERY,
            "case {case}: expected {MALFORMED_QUERY}, got {code} with {body}"
        );
        // The envelope reports the status and the code the response itself answers with; the
        // diagnostic is what it wraps.
        let said = without_sdk_envelope(case, &message(case, &body), 400, MALFORMED_QUERY);
        let statement = statement_the_compiler_reads(case, &query_string);
        let (start, end) = clause_range(case, &statement);
        assert_eq!(
            said,
            format!(
                "No aggregation function expression allowed: {predicate} ([{start},{end}])"
            ),
            "case {case}: expected the rule, the translated predicate and the clause's range \
             in {statement:?}, got {said:?}"
        );
        // The range is a range of the statement the compiler read, so for a source written
        // the short way it sits PAST where that short form ended in the submitted text.
        let submitted = query_string
            .find(HAVING)
            .unwrap_or_else(|| panic!("case {case}: the request carries a {HAVING} clause"));
        if statement == query_string {
            assert_eq!(
                start, submitted,
                "case {case}: a written-out source moves nothing, got {said:?}"
            );
        } else {
            assert!(
                start > submitted,
                "case {case}: the written-out source moves the clause along, got {said:?}"
            );
        }
        // The detail travels in the structure the model declares for it, locating the same
        // text the message names: the message states the clause's first and LAST character,
        // while the modelled location states one PAST its last, as every compile failure of
        // this service reports one.
        let location = &body["queryCompileError"]["location"];
        assert_eq!(
            (
                location["startCharOffset"].as_i64(),
                location["endCharOffset"].as_i64()
            ),
            (
                Some(i64::try_from(start).expect("an offset fits in an i64")),
                Some(i64::try_from(end + 1).expect("an offset fits in an i64"))
            ),
            "case {case}: the compile error locates the clause the message names, got {body}"
        );
    }

    // Nothing was started: a refused statement leaves no query behind.
    let described = logs
        .ok("DescribeQueries", &json!({"logGroupName": GROUP}))
        .await;
    assert!(
        described["queries"]
            .as_array()
            .is_none_or(|queries| queries.is_empty()),
        "case the refused statements: no query is recorded, got {described}"
    );
}

/// The statement shape CloudWatch Logs was RECORDED refusing, with the span it answered,
/// reproduced here as literal values: the span places the clause 35 characters further along
/// than the submitted text does, which is exactly what writing the source name out as the
/// selector call adds — so a reference text of any other length, or an exclusive second
/// offset, fails this test where the derived table above would not notice.
///
/// Both recorded forms are here. Their conditions are the same length, so the one span serves
/// both — the span is the clause's place, not a function of what it filters on. The log group
/// and the marker are one recorded run's values filling the statement; what the span depends
/// on is their LENGTH, and the message carries neither of them.
#[tokio::test]
async fn the_recorded_aggregate_filter_refusal_states_the_recorded_span() {
    let logs = Logs::new();
    const GROUP: &str = "sim2real-logsql-having-734ccb6a6695";
    const MARKER: &str = "mk7986f4cdabce";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let now_secs = now_ms() / 1_000;

    // (case, the recorded statement, the diagnostic the service answered it with)
    let recorded: [(&str, String, &str); 2] = [
        (
            "the recorded count form",
            format!(
                "SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM `{GROUP}` \
                 WHERE run = '{MARKER}' GROUP BY svc HAVING COUNT(*) > 1"
            ),
            "No aggregation function expression allowed: filtercount(*)>1 ([164,182])",
        ),
        (
            "the recorded sum form",
            format!(
                "SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM `{GROUP}` \
                 WHERE run = '{MARKER}' GROUP BY svc HAVING SUM(ms) > 50"
            ),
            "No aggregation function expression allowed: filtersum(ms)>50 ([164,182])",
        ),
    ];

    for (case, query_string, expected) in recorded {
        let (status, body) = logs
            .call(
                "StartQuery",
                &json!({
                    "queryLanguage": "SQL",
                    "queryString": query_string,
                    "startTime": now_secs - 3_600,
                    "endTime": now_secs,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the recorded statement is refused, got {status} {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, MALFORMED_QUERY,
            "case {case}: expected {MALFORMED_QUERY}, got {code} with {body}"
        );
        let said = without_sdk_envelope(case, &message(case, &body), 400, MALFORMED_QUERY);
        assert_eq!(
            said, expected,
            "case {case}: expected the recorded diagnostic, got {said:?}"
        );
    }
}
