// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a Logs Insights QL semantic refusal says the query is wrong.
//!
//! The rule under test: a refusal that states a RULE about a clause of the query states the
//! span of that whole clause — its FIRST and its LAST character, written `([first,last])` — so
//! `last - first` is the clause's length minus one and a client or console underlining the span
//! underlines exactly the offending text. Two semantic faults state such a span: a call to a
//! function the language does not define, whose clause is the whole call (the name, its
//! brackets and the arguments between them), reported as `Unrecognized function name: <name>`;
//! and a stage that re-declares an ephemeral field an earlier stage created, whose clause is
//! the COMMAND that declares it again (the command word through the last character of what it
//! acts on), reported as `Ephemeral field is already defined: <field>`. The modelled detail
//! carries the same place as `QueryCompileErrorLocation`, where the second offset is ONE PAST
//! the last character.
//!
//! The tables span what the rule generalises over: how long the name is, where in the pipeline
//! the clause stands, which command carries it, whether the call takes no argument, one, several
//! or another call, whether its name is written in another case, and whether its arguments carry
//! a literal that holds a bracket of its own.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operation under test.
const START: &str = "StartQuery";

/// The modelled code a query that does not compile is refused under.
const MALFORMED: &str = "MalformedQueryException";

/// The fixed text each of the two refusals opens with, written out here rather than read from
/// the crate so a change of wording fails this suite.
const UNRECOGNIZED_FUNCTION: &str = "Unrecognized function name: ";
const ALREADY_DEFINED: &str = "Ephemeral field is already defined: ";

/// A log group and the window a query of it is started with, in the epoch SECONDS
/// `StartQuery`'s range is specified in.
async fn group_with_window(logs: &Logs, group: &str) -> (i64, i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let now_secs = now_ms() / 1_000;
    (now_secs - 3_600, now_secs)
}

/// Start `query_string`, assert it is refused as a malformed query, and answer the sentence it
/// states (without the SDK envelope) together with the modelled compile-error location.
async fn refused(
    logs: &Logs,
    case: &str,
    group: &str,
    window: (i64, i64),
    query_string: &str,
) -> (String, (Option<i64>, Option<i64>)) {
    let (status, body) = logs
        .call(
            START,
            &json!({
                "logGroupName": group,
                "startTime": window.0,
                "endTime": window.1,
                "queryString": query_string,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the query to be refused, got {status} with {body}"
    );
    let code = crate::harness::error_code(&body);
    assert_eq!(
        code, MALFORMED,
        "case {case:?}: expected {MALFORMED}, got {code} with {body}"
    );
    // The compile failure's own text is the SENTENCE of the message, which the SDK envelope
    // this service closes every refusal with follows — reporting the status, the code and this
    // response's own request id.
    let said = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a refusal carries a message, got {body}"))
        .to_string();
    let sentence = crate::harness::without_sdk_envelope(case, &said, 400, MALFORMED);
    let location = &body["queryCompileError"]["location"];
    (
        sentence,
        (
            location["startCharOffset"].as_i64(),
            location["endCharOffset"].as_i64(),
        ),
    )
}

/// The character span `clause` occupies in `query_string`: its first and its last character,
/// found at the clause's own place in the submitted text.
fn span(case: &str, query_string: &str, clause: &str) -> (usize, usize) {
    let at = query_string
        .find(clause)
        .unwrap_or_else(|| panic!("case {case:?}: the row's query carries {clause:?}"));
    let start = query_string[..at].chars().count();
    (start, start + clause.chars().count() - 1)
}

/// RULE: the unknown-function refusal names the rule, the name the call used, and the span of
/// the WHOLE call.
#[tokio::test]
async fn an_unknown_function_call_is_refused_with_the_span_of_the_whole_call() {
    let logs = Logs::new();
    const GROUP: &str = "/semantic-spans/functions";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string, the name the call used, the whole call clause)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an unknown aggregate in a stats command",
            "fields ms | stats bogusfn(ms) as z",
            "bogusfn",
            "bogusfn(ms)",
        ),
        (
            "a misspelled aggregate further along the pipeline",
            "fields @message | stats avgg(lat) as mean by svc",
            "avgg",
            "avgg(lat)",
        ),
        (
            "an unknown function in a filter expression",
            "fields @message | filter frobnicate(lat) > 1",
            "frobnicate",
            "frobnicate(lat)",
        ),
        (
            "an unknown function opening the pipeline",
            "fields upperrr(@message) as shouted",
            "upperrr",
            "upperrr(@message)",
        ),
        (
            "a call taking no argument at all",
            "fields @message | stats nosuchfn() as n",
            "nosuchfn",
            "nosuchfn()",
        ),
        (
            "a call taking several arguments",
            "fields @message | stats bogus3(lat, 95, ms) as p",
            "bogus3",
            "bogus3(lat, 95, ms)",
        ),
        (
            // The clause is the OUTER call: its brackets close past the inner one.
            "a call whose argument is another call",
            "fields @message | stats bogusfn(min(lat)) as z",
            "bogusfn",
            "bogusfn(min(lat))",
        ),
        (
            // The name is reported as the query wrote it, and the catalogue is matched
            // case-insensitively, so a name in another case is still unknown.
            "a call whose name is written in upper case",
            "fields @message | stats BOGUSFN(ms) as z",
            "BOGUSFN",
            "BOGUSFN(ms)",
        ),
        (
            // A bracket inside a quoted literal is the caller's own text, so it neither opens
            // nor closes the clause.
            "a call whose argument carries a bracket in a literal",
            r#"fields @message | filter frobnicate(svc, "a)b") > 1"#,
            "frobnicate",
            r#"frobnicate(svc, "a)b")"#,
        ),
    ];

    for (case, query_string, name, clause) in &cases {
        let (sentence, location) = refused(&logs, case, GROUP, window, query_string).await;
        let (start, last) = span(case, query_string, clause);
        assert_eq!(
            sentence,
            format!("{UNRECOGNIZED_FUNCTION}{name} ([{start},{last}])"),
            "case {case:?}: expected the rule, the name and the call's own span, got \
             {sentence:?}"
        );
        assert_eq!(
            last - start,
            clause.chars().count() - 1,
            "case {case:?}: the span's width is the clause's length minus one, got \
             {sentence:?}"
        );
        assert_eq!(
            location,
            (
                Some(i64::try_from(start).expect("an offset fits in an i64")),
                Some(i64::try_from(last + 1).expect("an offset fits in an i64")),
            ),
            "case {case:?}: the modelled location states one past the last character, got \
             {location:?}"
        );
    }
}

/// RULE: the redefinition refusal names the rule, the duplicated field and the span of the
/// CLAUSE that declares that field a second time.
#[tokio::test]
async fn a_redefined_ephemeral_field_is_refused_with_the_span_of_the_clause() {
    let logs = Logs::new();
    const GROUP: &str = "/semantic-spans/fields";
    let window = group_with_window(&logs, GROUP).await;

    // (case, the query string, the duplicated field, the clause that declares it again)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a fields projection naming a glob-parsed field",
            r#"filter @message like /mk/ | parse @message "svc=* lat=* status=*" as svc, lat, st | sort @timestamp asc | fields @message, lat"#,
            "lat",
            "fields @message, lat",
        ),
        (
            "a fields projection naming a captured field",
            r"parse @message /dur=(?<duration>\d+)/ | fields @timestamp, duration",
            "duration",
            "fields @timestamp, duration",
        ),
        (
            "a one-word projection of a parsed field",
            r#"parse @message "tg=*" as tg | fields tg"#,
            "tg",
            "fields tg",
        ),
        (
            "a second glob parse naming a field the first named",
            r#"parse @message "lat=*" as lat | parse @message "lon=*" as lat"#,
            "lat",
            r#"parse @message "lon=*" as lat"#,
        ),
        (
            "an aggregation aliased to a parsed field",
            r"parse @message /(?<num>\d+)/ | stats count(*) as num",
            "num",
            "stats count(*) as num",
        ),
    ];

    for (case, query_string, field, clause) in &cases {
        let (sentence, location) = refused(&logs, case, GROUP, window, query_string).await;
        let at = query_string
            .rfind(clause)
            .unwrap_or_else(|| panic!("case {case:?}: the row's query carries {clause:?}"));
        let start = query_string[..at].chars().count();
        let last = start + clause.chars().count() - 1;
        assert_eq!(
            sentence,
            format!("{ALREADY_DEFINED}{field} ([{start},{last}])"),
            "case {case:?}: expected the rule, the field and the clause's own span, got \
             {sentence:?}"
        );
        assert_eq!(
            last - start,
            clause.chars().count() - 1,
            "case {case:?}: the span's width is the clause's length minus one, got \
             {sentence:?}"
        );
        assert_eq!(
            location,
            (
                Some(i64::try_from(start).expect("an offset fits in an i64")),
                Some(i64::try_from(last + 1).expect("an offset fits in an i64")),
            ),
            "case {case:?}: the modelled location states one past the last character, got \
             {location:?}"
        );
    }

    // Nothing was started: a refused query leaves no query behind.
    let described: Value = logs
        .ok("DescribeQueries", &json!({"logGroupName": GROUP}))
        .await;
    assert!(
        described["queries"]
            .as_array()
            .is_none_or(|queries| queries.is_empty()),
        "case the refused queries: no query is recorded, got {described}"
    );
}
