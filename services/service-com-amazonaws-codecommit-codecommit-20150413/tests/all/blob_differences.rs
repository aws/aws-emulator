// SPDX-License-Identifier: CC0-1.0

//! `GetBlobDifferences` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: the answer is the line-level comparison of the two blobs the
//! named repository holds — every run of lines the two differ in, each carrying the
//! requested number of unchanged context lines, in file order, with the 1-based line
//! numbers and the content of the lines the store actually holds. The tables below
//! span what that rule generalises over — where in the file the change sits, whether
//! it is an addition, a deletion or a replacement, how many runs there are and how
//! far apart, the context width (every published bound and one past it), whether
//! whitespace counts, whether the content is text at all, and the page size — rather
//! than one recorded request.
//!
//! The harness (`emulator`, `ok`, `error`, `string_at`, `base64`) is the shared one
//! in [`crate::repository_lifecycle`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, error, ok, string_at};

/// The HTTP status every error this suite asserts models (`errorKind: client`,
/// `httpError: 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The path the two versions of the compared file are written to. Which path holds a
/// blob does not change the comparison — the operation addresses blobs by object
/// name — so one path serves every row.
const PATH: &str = "src/main.txt";

/// Write `before` and then `after` as that file in a fresh repository, and answer the
/// two `blobId`s those writes reported: the ids the documented workflow feeds this
/// operation. Identical content is one object, so it is written once.
async fn blob_pair(
    router: &MultiServiceRouter,
    repository: &str,
    before: &[u8],
    after: &[u8],
) -> (String, String) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let first = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(before),
            "commitMessage": "the before version",
        }),
    )
    .await;
    let before_id = string_at(&first, "/blobId");
    if before == after {
        return (before_id.clone(), before_id);
    }
    let second = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(after),
            "parentCommitId": string_at(&first, "/commitId"),
            "commitMessage": "the after version",
        }),
    )
    .await;
    (before_id, string_at(&second, "/blobId"))
}

/// One `GetBlobDifferences` request body.
fn request(repository: &str, before: &str, after: &str) -> Value {
    json!({
        "repositoryName": repository,
        "beforeBlobId": before,
        "afterBlobId": after,
    })
}

fn number(payload: &Value, member: &str) -> i64 {
    payload
        .get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a number at {member} in {payload}"))
}

fn hunks(payload: &Value) -> Vec<Value> {
    payload
        .get("hunks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a hunks list in {payload}"))
}

fn changes(hunk: &Value) -> Vec<Value> {
    hunk.get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a changes list in {hunk}"))
}

/// One hunk as text: its before and after spans in a unified diff's `@@` form,
/// then each of its lines as `<C|A|D><beforeLine>/<afterLine>:<content>`, with `-`
/// for the side that does not hold the line. Rendering the whole shape, so a row's
/// expectation pins every member the hunk carries.
fn render(hunk: &Value) -> String {
    let lines: Vec<String> = changes(hunk)
        .iter()
        .map(|change| {
            let kind = match change.get("type").and_then(Value::as_str) {
                Some("CONTEXT") => "C",
                Some("ADD") => "A",
                Some("DELETE") => "D",
                other => panic!("unexpected change type {other:?} in {change}"),
            };
            let side = |member: &str| {
                change
                    .get(member)
                    .and_then(Value::as_i64)
                    .map_or_else(|| "-".to_string(), |line| line.to_string())
            };
            format!(
                "{kind}{}/{}:{}",
                side("beforeLineNumber"),
                side("afterLineNumber"),
                change
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("<no content>")
            )
        })
        .collect();
    format!(
        "@@ -{},{} +{},{} @@ {}",
        number(hunk, "beforeStartLine"),
        number(hunk, "beforeLineCount"),
        number(hunk, "afterStartLine"),
        number(hunk, "afterLineCount"),
        lines.join(" ")
    )
}

fn rendered(payload: &Value) -> Vec<String> {
    hunks(payload).iter().map(render).collect()
}

/// The lines of a blob as this operation reports them: content split at the line
/// feed that terminates a line, without that terminator.
fn lines_of(content: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// One row of the comparison table: two versions of a file, and the hunks the
/// operation must answer for them with NO context lines, where a hunk is exactly a
/// run of changed lines.
struct Case {
    label: &'static str,
    before: &'static str,
    after: &'static str,
    expected: Vec<&'static str>,
}

/// The table. Rows differ along what the rule generalises over: where the change
/// sits (start, middle, end), its kind (replacement, insertion, deletion), its
/// length (one line, a run, the whole file), how many runs there are and how far
/// apart, whether the file ends with a line terminator, and whether the inserted
/// line is blank.
fn comparison_cases() -> Vec<Case> {
    vec![
        Case {
            label: "a replaced line in the middle",
            before: "alpha\nbeta\ngamma\n",
            after: "alpha\nBETA\ngamma\n",
            expected: vec!["@@ -2,1 +2,1 @@ D2/-:beta A-/2:BETA"],
        },
        Case {
            label: "a line appended at the end",
            before: "alpha\nbeta\n",
            after: "alpha\nbeta\ngamma\n",
            expected: vec!["@@ -2,0 +3,1 @@ A-/3:gamma"],
        },
        Case {
            label: "a line inserted before the first",
            before: "beta\n",
            after: "alpha\nbeta\n",
            // The hunk covers no line of the before blob, and a start line is a
            // 1-based line number, so it begins at the first line rather than at 0.
            expected: vec!["@@ -1,0 +1,1 @@ A-/1:alpha"],
        },
        Case {
            label: "a line deleted from the middle",
            before: "alpha\nbeta\ngamma\n",
            after: "alpha\ngamma\n",
            expected: vec!["@@ -2,1 +1,0 @@ D2/-:beta"],
        },
        Case {
            label: "a run of lines inserted in the middle",
            before: "alpha\nomega\n",
            after: "alpha\none\ntwo\nomega\n",
            expected: vec!["@@ -1,0 +2,2 @@ A-/2:one A-/3:two"],
        },
        Case {
            label: "a blank line inserted",
            before: "alpha\nbeta\n",
            after: "alpha\n\nbeta\n",
            expected: vec!["@@ -1,0 +2,1 @@ A-/2:"],
        },
        Case {
            label: "two changes seven lines apart are two hunks",
            before: "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n",
            after: "L1\nl2\nl3\nl4\nl5\nl6\nl7\nL8\n",
            expected: vec![
                "@@ -1,1 +1,1 @@ D1/-:l1 A-/1:L1",
                "@@ -8,1 +8,1 @@ D8/-:l8 A-/8:L8",
            ],
        },
        Case {
            label: "every line replaced",
            before: "a\nb\n",
            after: "A\nB\n",
            expected: vec!["@@ -1,2 +1,2 @@ D1/-:a D2/-:b A-/1:A A-/2:B"],
        },
        Case {
            label: "content that does not end with a line terminator",
            before: "alpha\nbeta",
            after: "alpha\nBETA",
            expected: vec!["@@ -2,1 +2,1 @@ D2/-:beta A-/2:BETA"],
        },
        Case {
            label: "identical content",
            before: "same\nlines\n",
            after: "same\nlines\n",
            expected: vec![],
        },
    ]
}

#[tokio::test]
async fn the_hunks_are_the_runs_of_lines_the_two_blobs_differ_in() {
    let router = emulator();

    for (index, case) in comparison_cases().iter().enumerate() {
        let label = case.label;
        let repository = format!("blobdiff-{index}");
        let (before_id, after_id) = blob_pair(
            &router,
            &repository,
            case.before.as_bytes(),
            case.after.as_bytes(),
        )
        .await;

        let mut input = request(&repository, &before_id, &after_id);
        input["contextLines"] = json!(0);
        let got = ok(&router, "GetBlobDifferences", &input).await;

        assert_eq!(
            rendered(&got),
            case.expected,
            "case {label:?}: the hunks must be the runs of changed lines, got {got}"
        );
        assert_eq!(
            got.get("isBinary").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: text content is not binary, got {got}"
        );
        assert_eq!(
            number(&got, "beforeBlobSize"),
            case.before.len() as i64,
            "case {label:?}: beforeBlobSize must be the stored blob's byte count, got {got}"
        );
        assert_eq!(
            number(&got, "afterBlobSize"),
            case.after.len() as i64,
            "case {label:?}: afterBlobSize must be the stored blob's byte count, got {got}"
        );
        assert_eq!(
            got.get("NextToken"),
            None,
            "case {label:?}: one page holds every hunk, so no token is returned, got {got}"
        );

        // A read does not change what the next caller sees.
        let again = ok(&router, "GetBlobDifferences", &input).await;
        assert_eq!(
            again, got,
            "case {label:?}: the comparison must be repeatable, got {again} then {got}"
        );
    }
}

#[tokio::test]
async fn the_widest_context_reports_every_line_of_both_blobs_in_order() {
    let router = emulator();

    for (index, case) in comparison_cases().iter().enumerate() {
        let label = case.label;
        let repository = format!("blobdiff-whole-{index}");
        let (before_id, after_id) = blob_pair(
            &router,
            &repository,
            case.before.as_bytes(),
            case.after.as_bytes(),
        )
        .await;

        // Every row is shorter than the widest context the member accepts, so one
        // hunk covers the whole comparison — and that one hunk must then hold every
        // line of each blob, at its own line number, in file order.
        let mut input = request(&repository, &before_id, &after_id);
        input["contextLines"] = json!(20);
        let got = ok(&router, "GetBlobDifferences", &input).await;
        let found = hunks(&got);
        if case.expected.is_empty() {
            assert!(
                found.is_empty(),
                "case {label:?}: identical blobs have no hunks, got {got}"
            );
            continue;
        }
        assert_eq!(
            found.len(),
            1,
            "case {label:?}: a context wider than the file leaves one hunk, got {got}"
        );
        let hunk = &found[0];

        for (member, side, expected) in [
            ("beforeLineNumber", "before", lines_of(case.before)),
            ("afterLineNumber", "after", lines_of(case.after)),
        ] {
            let held: Vec<(i64, String)> = changes(hunk)
                .iter()
                .filter_map(|change| {
                    let line = change.get(member).and_then(Value::as_i64)?;
                    let content = change
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Some((line, content))
                })
                .collect();
            let numbers: Vec<i64> = held.iter().map(|(line, _)| *line).collect();
            let contents: Vec<&str> = held.iter().map(|(_, text)| text.as_str()).collect();
            assert_eq!(
                numbers,
                (1..=expected.len() as i64).collect::<Vec<i64>>(),
                "case {label:?}: the {side} lines must be numbered 1..{} in order, got {got}",
                expected.len()
            );
            assert_eq!(
                contents, expected,
                "case {label:?}: the {side} lines must be the ones the blob holds, got {got}"
            );
        }
    }
}

/// The file the context table compares: eight distinct lines, with the fourth
/// replaced, so the context around ONE change is what varies.
const CONTEXT_BEFORE: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n";
const CONTEXT_AFTER: &str = "l1\nl2\nl3\nL4\nl5\nl6\nl7\nl8\n";

/// Where the one change sits, and how long the file is.
const CHANGED_LINE: i64 = 4;
const FILE_LINES: i64 = 8;

#[tokio::test]
async fn context_lines_widen_the_hunk_around_the_change() {
    let router = emulator();
    let (before_id, after_id) = blob_pair(
        &router,
        "blobdiff-context",
        CONTEXT_BEFORE.as_bytes(),
        CONTEXT_AFTER.as_bytes(),
    )
    .await;
    let base = request("blobdiff-context", &before_id, &after_id);

    // `contextLines` is "the number of unchanged lines of context to include before
    // and after each block of changes in a hunk. Valid values are 0 through 20.
    // Defaults to 3." Every value in that published range, plus the default, which
    // must answer as 3 does.
    let mut requests: Vec<(String, Value)> = (0..=20)
        .map(|context| {
            let mut input = base.clone();
            input["contextLines"] = json!(context);
            (context.to_string(), input)
        })
        .collect();
    requests.push(("the default".to_string(), base.clone()));

    for (label, input) in requests {
        let context = input
            .get("contextLines")
            .and_then(Value::as_i64)
            .unwrap_or(3);
        let got = ok(&router, "GetBlobDifferences", &input).await;
        let found = hunks(&got);
        assert_eq!(
            found.len(),
            1,
            "context {label}: one change is one hunk, got {got}"
        );

        // The hunk spans the changed line plus `context` lines on each side, clipped
        // to the file.
        let first = (CHANGED_LINE - context).max(1);
        let last = (CHANGED_LINE + context).min(FILE_LINES);
        let span = last - first + 1;
        for (start, lines) in [("beforeStartLine", "beforeLineCount"), ("afterStartLine", "afterLineCount")] {
            assert_eq!(
                number(&found[0], start),
                first,
                "context {label}: {start} must be {first}, got {got}"
            );
            assert_eq!(
                number(&found[0], lines),
                span,
                "context {label}: {lines} must be {span}, got {got}"
            );
        }
        // The changed line is reported as a deletion and an addition whatever the
        // context is, and every other line of the hunk is context.
        let kinds: Vec<&str> = changes(&found[0])
            .iter()
            .filter_map(|change| change.get("type").and_then(Value::as_str))
            .map(|kind| if kind == "CONTEXT" { "C" } else { "X" })
            .collect();
        assert_eq!(
            kinds.iter().filter(|kind| **kind == "X").count(),
            2,
            "context {label}: the one changed line is one deletion and one addition, got {got}"
        );
        assert_eq!(
            kinds.len() as i64,
            span * 2 - (span - 1),
            "context {label}: the hunk carries the span's lines once, plus the replaced \
             line's second side, got {got}"
        );
    }
}

#[tokio::test]
async fn a_context_width_outside_the_published_range_is_refused() {
    let router = emulator();
    let (before_id, after_id) = blob_pair(
        &router,
        "blobdiff-context-bounds",
        CONTEXT_BEFORE.as_bytes(),
        CONTEXT_AFTER.as_bytes(),
    )
    .await;
    let base = request("blobdiff-context-bounds", &before_id, &after_id);

    // One past each published bound.
    for context in [-1, 21, 1000] {
        let mut input = base.clone();
        input["contextLines"] = json!(context);
        let (code, message) =
            error_with_message(&router, "GetBlobDifferences", &input, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "ValidationException",
            "context {context}: lies outside 0 through 20, got {code}"
        );
        assert_eq!(
            message,
            "The specified number of context lines is not valid. Valid values are 0 through 20.",
            "context {context}: the refusal states the range it accepts, got {message:?}"
        );
    }
}

#[tokio::test]
async fn ignoring_whitespace_treats_a_whitespace_only_change_as_unchanged() {
    let router = emulator();

    // Each row changes a line in whitespace ALONE, in a different way: trailing
    // spaces, a trailing tab, the indentation character, the width of the
    // indentation, and the spacing inside the line.
    let whitespace_only: Vec<(&str, &str, &str)> = vec![
        ("trailing spaces", "alpha\nbeta\n", "alpha\nbeta   \n"),
        ("a trailing tab", "alpha\nbeta\n", "alpha\nbeta\t\n"),
        ("tabs for spaces", "    alpha\n", "\talpha\n"),
        ("a wider indentation", "  alpha\n", "        alpha\n"),
        ("spacing inside the line", "a = b\n", "a=b\n"),
    ];

    for (index, (label, before, after)) in whitespace_only.iter().enumerate() {
        let repository = format!("blobdiff-space-{index}");
        let (before_id, after_id) =
            blob_pair(&router, &repository, before.as_bytes(), after.as_bytes()).await;
        let base = request(&repository, &before_id, &after_id);

        // The two blobs ARE different objects of different sizes...
        assert_ne!(
            before_id, after_id,
            "case {label:?}: the two versions must be distinct objects"
        );

        // ...and with whitespace ignored the comparison reports no change at all.
        let mut ignoring = base.clone();
        ignoring["ignoreWhitespace"] = json!(true);
        let ignored = ok(&router, "GetBlobDifferences", &ignoring).await;
        assert!(
            hunks(&ignored).is_empty(),
            "case {label:?}: a whitespace-only change is unchanged when whitespace is \
             ignored, got {ignored}"
        );
        // The sizes still come from the stored blobs, not from the comparison.
        assert_eq!(
            number(&ignored, "beforeBlobSize"),
            before.len() as i64,
            "case {label:?}: beforeBlobSize must be the stored blob's byte count, got {ignored}"
        );
        assert_eq!(
            number(&ignored, "afterBlobSize"),
            after.len() as i64,
            "case {label:?}: afterBlobSize must be the stored blob's byte count, got {ignored}"
        );

        // By default whitespace counts, so the same pair is a change.
        for (how, mut input) in [
            ("left out", base.clone()),
            ("false", {
                let mut explicit = base.clone();
                explicit["ignoreWhitespace"] = json!(false);
                explicit
            }),
        ] {
            input["contextLines"] = json!(0);
            let got = ok(&router, "GetBlobDifferences", &input).await;
            assert!(
                !hunks(&got).is_empty(),
                "case {label:?}: with ignoreWhitespace {how} the change is reported, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn ignoring_whitespace_still_reports_a_change_in_the_line_itself() {
    let router = emulator();
    // The line differs in its text AND in whitespace: the change is reported, and the
    // content reported is the line as each blob holds it, whitespace included.
    let (before_id, after_id) = blob_pair(
        &router,
        "blobdiff-space-change",
        b"  alpha\nbeta\n",
        b"\tALPHA\nbeta  \n",
    )
    .await;
    let mut input = request("blobdiff-space-change", &before_id, &after_id);
    input["ignoreWhitespace"] = json!(true);
    input["contextLines"] = json!(0);
    let got = ok(&router, "GetBlobDifferences", &input).await;
    assert_eq!(
        rendered(&got),
        vec!["@@ -1,1 +1,1 @@ D1/-:  alpha A-/1:\tALPHA"],
        "a line whose text changed is reported, with the content each blob holds, got {got}"
    );
}

#[tokio::test]
async fn binary_content_is_not_compared_line_by_line() {
    let router = emulator();

    // Each row is content no line-level diff can be reported for, and the rows differ
    // along what makes it so and how large it is: a NUL byte at the start, in the
    // middle and at the end of a larger blob, every byte value, and bytes that are
    // not valid UTF-8 text.
    let binary: Vec<(&str, Vec<u8>)> = vec![
        ("a leading NUL byte", b"\0alpha\nbeta\n".to_vec()),
        ("a NUL byte in the middle", b"alpha\n\0\nbeta\n".to_vec()),
        ("a NUL byte at the end of 16 KiB", {
            let mut content = b"line\n".repeat(3_000);
            content.push(0);
            content
        }),
        ("every byte value", (0..=255u8).collect()),
        ("bytes that are not UTF-8", vec![0xf0, 0x9f, 0x92, 0x0a, 0xff]),
    ];

    for (index, (label, content)) in binary.iter().enumerate() {
        let repository = format!("blobdiff-binary-{index}");
        let text = b"alpha\nbeta\n".to_vec();
        let (text_id, binary_id) = blob_pair(&router, &repository, &text, content).await;

        // Either side being binary is enough: the comparison is binary both ways.
        for (which, before, after) in [
            ("the after blob", &text_id, &binary_id),
            ("the before blob", &binary_id, &text_id),
        ] {
            let got = ok(
                &router,
                "GetBlobDifferences",
                &request(&repository, before, after),
            )
            .await;
            assert_eq!(
                got.get("isBinary").and_then(Value::as_bool),
                Some(true),
                "case {label:?} ({which}): the comparison is binary, got {got}"
            );
            assert!(
                hunks(&got).is_empty(),
                "case {label:?} ({which}): a binary comparison reports no hunks, got {got}"
            );
            assert_eq!(
                got.get("NextToken"),
                None,
                "case {label:?} ({which}): there is nothing to page through, got {got}"
            );
        }

        // The sizes are still the stored blobs' own.
        let got = ok(
            &router,
            "GetBlobDifferences",
            &request(&repository, &text_id, &binary_id),
        )
        .await;
        assert_eq!(
            number(&got, "beforeBlobSize"),
            text.len() as i64,
            "case {label:?}: beforeBlobSize must be the stored blob's byte count, got {got}"
        );
        assert_eq!(
            number(&got, "afterBlobSize"),
            content.len() as i64,
            "case {label:?}: afterBlobSize must be the stored blob's byte count, got {got}"
        );
    }
}

#[tokio::test]
async fn an_unspecified_before_blob_compares_against_an_empty_before_state() {
    let router = emulator();
    let after = "alpha\nbeta\ngamma\n";
    let (_, after_id) = blob_pair(
        &router,
        "blobdiff-added",
        b"placeholder\n",
        after.as_bytes(),
    )
    .await;

    // "If you do not specify a value, the operation returns a diff against an empty
    // before-state. This is equivalent to treating the file as newly added." A blank
    // value names no blob either, so it is the same as leaving it out.
    for (label, before) in [("left out", None), ("blank", Some("")), ("spaces", Some("  "))] {
        let mut input = json!({
            "repositoryName": "blobdiff-added",
            "afterBlobId": after_id,
            "contextLines": 0,
        });
        if let Some(before) = before {
            input["beforeBlobId"] = json!(before);
        }
        let got = ok(&router, "GetBlobDifferences", &input).await;
        assert_eq!(
            rendered(&got),
            vec!["@@ -1,0 +1,3 @@ A-/1:alpha A-/2:beta A-/3:gamma"],
            "beforeBlobId {label}: every line of the after blob is an addition, got {got}"
        );
        assert_eq!(
            number(&got, "beforeBlobSize"),
            0,
            "beforeBlobId {label}: \"Returns 0 when you do not specify beforeBlobId\", got {got}"
        );
        assert_eq!(
            number(&got, "afterBlobSize"),
            after.len() as i64,
            "beforeBlobId {label}: afterBlobSize must be the stored blob's byte count, got {got}"
        );
    }
}

/// A file with one changed line every tenth line, so the comparison has six hunks to
/// page through: `(before, after)`.
fn paged_versions() -> (String, String) {
    let before: Vec<String> = (1..=60).map(|line| format!("line-{line:02}")).collect();
    let after: Vec<String> = before
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if (index + 1) % 10 == 5 {
                line.to_uppercase()
            } else {
                line.clone()
            }
        })
        .collect();
    (
        format!("{}\n", before.join("\n")),
        format!("{}\n", after.join("\n")),
    )
}

#[tokio::test]
async fn hunks_are_paged_by_max_results_and_the_token_resumes_the_comparison() {
    let router = emulator();
    let (before, after) = paged_versions();
    let (before_id, after_id) = blob_pair(
        &router,
        "blobdiff-paged",
        before.as_bytes(),
        after.as_bytes(),
    )
    .await;
    let mut base = request("blobdiff-paged", &before_id, &after_id);
    base["contextLines"] = json!(0);

    let whole = ok(&router, "GetBlobDifferences", &base).await;
    let all = rendered(&whole);
    assert_eq!(
        all.len(),
        6,
        "the comparison has one hunk per changed line, got {whole}"
    );
    assert_eq!(
        whole.get("NextToken"),
        None,
        "one page holds all six hunks, so no token is returned, got {whole}"
    );

    // Rows differ along the page size: one hunk at a time, several at a time, exactly
    // the whole list, and more than it.
    for page in [1usize, 2, 4, 6, 7, 1_000] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut input = base.clone();
            input["MaxResults"] = json!(page);
            if let Some(token) = &token {
                input["NextToken"] = json!(token);
            }
            let got = ok(&router, "GetBlobDifferences", &input).await;
            let batch = rendered(&got);
            assert!(
                batch.len() <= page,
                "page {page}: a page carries at most {page} hunks, got {got}"
            );
            collected.extend(batch);
            pages += 1;
            token = got
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= 6,
                "page {page}: paging must terminate, got {pages} pages"
            );
        }
        assert_eq!(
            collected, all,
            "page {page}: paging must report every hunk once, in order"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page.min(all.len()).max(1)),
            "page {page}: the hunks must come in the fewest pages that hold them"
        );
    }
}

#[tokio::test]
async fn get_blob_differences_is_refused_with_the_modeled_error() {
    let router = emulator();
    let (before_id, after_id) = blob_pair(
        &router,
        "blobdiff-guarded",
        b"alpha\nbeta\n",
        b"alpha\nBETA\n",
    )
    .await;
    // A well-formed object name no repository holds.
    let absent = "0".repeat(40);

    // Each row is a request that cannot be served, and the error the model names for
    // it. They differ along which part of the request is at fault: the repository,
    // each blob id's presence, form and existence, the context width, the page size
    // and the continuation token.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            request("no-such-repository", &before_id, &after_id),
            "RepositoryDoesNotExistException",
        ),
        (
            "no repository name at all",
            json!({ "beforeBlobId": before_id, "afterBlobId": after_id }),
            "RepositoryNameRequiredException",
        ),
        (
            // `RepositoryName`'s modeled pattern is ^[\w\.-]+$.
            "a repository name outside the modeled pattern",
            request("not a name!", &before_id, &after_id),
            "InvalidRepositoryNameException",
        ),
        (
            "no afterBlobId at all",
            json!({ "repositoryName": "blobdiff-guarded", "beforeBlobId": before_id }),
            "BlobIdRequiredException",
        ),
        (
            "an empty afterBlobId",
            request("blobdiff-guarded", &before_id, ""),
            "BlobIdRequiredException",
        ),
        (
            "a blank afterBlobId",
            request("blobdiff-guarded", &before_id, "   "),
            "BlobIdRequiredException",
        ),
        (
            "an afterBlobId one character short of an object name",
            request("blobdiff-guarded", &before_id, &"a".repeat(39)),
            "InvalidBlobIdException",
        ),
        (
            "an afterBlobId one character longer than an object name",
            request("blobdiff-guarded", &before_id, &"a".repeat(41)),
            "InvalidBlobIdException",
        ),
        (
            "an afterBlobId of the right width that is not hexadecimal",
            request("blobdiff-guarded", &before_id, &"z".repeat(40)),
            "InvalidBlobIdException",
        ),
        (
            "a beforeBlobId that is not an object name",
            request("blobdiff-guarded", "not-an-object-name", &after_id),
            "InvalidBlobIdException",
        ),
        (
            "an afterBlobId this repository does not hold",
            request("blobdiff-guarded", &before_id, &absent),
            "BlobIdDoesNotExistException",
        ),
        (
            "a beforeBlobId this repository does not hold",
            request("blobdiff-guarded", &absent, &after_id),
            "BlobIdDoesNotExistException",
        ),
        (
            "a page size of zero",
            json!({
                "repositoryName": "blobdiff-guarded",
                "beforeBlobId": before_id,
                "afterBlobId": after_id,
                "MaxResults": 0,
            }),
            "InvalidMaxResultsException",
        ),
        (
            "a negative page size",
            json!({
                "repositoryName": "blobdiff-guarded",
                "beforeBlobId": before_id,
                "afterBlobId": after_id,
                "MaxResults": -5,
            }),
            "InvalidMaxResultsException",
        ),
        (
            "a continuation token this service never issued",
            json!({
                "repositoryName": "blobdiff-guarded",
                "beforeBlobId": before_id,
                "afterBlobId": after_id,
                "NextToken": "not-a-token",
            }),
            "InvalidContinuationTokenException",
        ),
        (
            "a continuation token past the end of the comparison",
            json!({
                "repositoryName": "blobdiff-guarded",
                "beforeBlobId": before_id,
                "afterBlobId": after_id,
                "NextToken": "99",
            }),
            "InvalidContinuationTokenException",
        ),
        (
            "a context width past the published maximum",
            json!({
                "repositoryName": "blobdiff-guarded",
                "beforeBlobId": before_id,
                "afterBlobId": after_id,
                "contextLines": 21,
            }),
            "ValidationException",
        ),
    ];

    for (label, input, expected) in cases {
        let code = error(&router, "GetBlobDifferences", &input, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, expected,
            "case {label:?} ({input}): expected {expected}, got {code}"
        );
    }

    // None of the refusals changed what the repository holds.
    let got = ok(
        &router,
        "GetBlobDifferences",
        &request("blobdiff-guarded", &before_id, &after_id),
    )
    .await;
    assert_eq!(
        rendered(&got),
        vec!["@@ -1,2 +1,2 @@ C1/1:alpha D2/-:beta A-/2:BETA"],
        "a refused comparison must leave the stored blobs comparable, got {got}"
    );
}

/// Content of `lines` lines, each distinct, so a comparison over it is unambiguous.
fn numbered_lines(lines: usize) -> String {
    (1..=lines)
        .map(|line| format!("line-{line}\n"))
        .collect::<Vec<String>>()
        .concat()
}

#[tokio::test]
async fn a_comparison_past_what_the_emulator_compares_is_refused() {
    let router = emulator();

    // The emulator compares at most 8,192 lines and 512 KB of ONE blob, and at most
    // 8,192 lines across BOTH sides of one request (`MAX_COMPARED_LINES`,
    // `MAX_COMPARED_BYTES`, `MAX_COMPARED_LINES_COMBINED` — the per-side pair is the
    // same bounds the crate's three-way merge compares a file within). Each bound is
    // driven at the bound, where the comparison is served, and past it, where the error
    // the model declares for a file this operation cannot compare is returned. The rows
    // differ along WHICH bound the pair is past, and along how the lines are distributed
    // between the two sides: one side carrying nearly all of them, its bytes with barely
    // any lines at all, and the two sides sharing them evenly.
    let cases: Vec<(&str, String, String, bool)> = vec![
        (
            "at the combined line bound, nearly all on one side",
            "line-1\n".to_string(),
            numbered_lines(8 * 1024 - 1),
            true,
        ),
        (
            "one line past the combined bound, nearly all on one side",
            "line-1\n".to_string(),
            numbered_lines(8 * 1024),
            false,
        ),
        (
            "one side past the per-side line bound",
            "line-1\n".to_string(),
            numbered_lines(8 * 1024 + 1),
            false,
        ),
        (
            "one line of 600 KB, past the byte bound",
            "line-1\n".to_string(),
            format!("{}\n", "x".repeat(600 * 1024)),
            false,
        ),
        (
            "the lines shared evenly, at the combined bound",
            numbered_lines(4 * 1024),
            format!("changed\n{}", numbered_lines(4 * 1024 - 1)),
            true,
        ),
        (
            "the lines shared evenly, one past the combined bound",
            numbered_lines(4 * 1024 + 1),
            numbered_lines(4 * 1024),
            false,
        ),
    ];

    for (index, (label, before, after, served)) in cases.iter().enumerate() {
        let repository = format!("blobdiff-bound-{index}");
        let (before_id, after_id) =
            blob_pair(&router, &repository, before.as_bytes(), after.as_bytes()).await;
        let mut input = request(&repository, &before_id, &after_id);
        input["contextLines"] = json!(0);
        if *served {
            let got = ok(&router, "GetBlobDifferences", &input).await;
            assert_eq!(
                number(&got, "afterBlobSize"),
                after.len() as i64,
                "case {label:?}: the comparison must be served, got {got}"
            );
            assert!(
                !hunks(&got).is_empty(),
                "case {label:?}: the changed lines must be reported, got {got}"
            );
        } else {
            let (code, message) =
                error_with_message(&router, "GetBlobDifferences", &input, CLIENT_ERROR_STATUS)
                    .await;
            assert_eq!(
                code, "FileTooLargeException",
                "case {label:?}: a pair past the bound cannot be compared, got {code}"
            );
            assert!(
                !message.is_empty(),
                "case {label:?}: the refusal must state what it refuses, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_blob_ids_get_differences_reports_are_the_ones_this_operation_compares() {
    let router = emulator();
    let before = "alpha\nbeta\ngamma\n";
    let after = "alpha\nBETA\ngamma\ndelta\n";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "blobdiff-workflow" }),
    )
    .await;
    let first = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "blobdiff-workflow",
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(before.as_bytes()),
        }),
    )
    .await;
    let second = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "blobdiff-workflow",
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(after.as_bytes()),
            "parentCommitId": string_at(&first, "/commitId"),
        }),
    )
    .await;
    let before_id = string_at(&first, "/blobId");
    let after_id = string_at(&second, "/blobId");

    // "For the typical usage workflow, see GetDifferences": the ids that operation
    // reports for the changed file are what this one compares, and the comparison of
    // THOSE ids is the comparison of the two stored versions.
    let differences = ok(
        &router,
        "GetDifferences",
        &json!({
            "repositoryName": "blobdiff-workflow",
            "beforeCommitSpecifier": string_at(&first, "/commitId"),
            "afterCommitSpecifier": string_at(&second, "/commitId"),
            "afterPath": PATH,
        }),
    )
    .await;
    let difference = differences
        .pointer("/differences/0")
        .unwrap_or_else(|| panic!("no difference reported in {differences}"))
        .clone();
    assert_eq!(
        string_at(&difference, "/afterBlob/blobId"),
        after_id,
        "GetDifferences must report the object name the write reported, got {difference}"
    );
    assert_eq!(
        string_at(&difference, "/beforeBlob/blobId"),
        before_id,
        "GetDifferences must report the before version's object name, got {difference}"
    );

    let mut input = json!({
        "repositoryName": "blobdiff-workflow",
        "beforeBlobId": string_at(&difference, "/beforeBlob/blobId"),
        "afterBlobId": string_at(&difference, "/afterBlob/blobId"),
        "contextLines": 0,
    });
    let got = ok(&router, "GetBlobDifferences", &input).await;
    assert_eq!(
        rendered(&got),
        vec![
            "@@ -2,1 +2,1 @@ D2/-:beta A-/2:BETA",
            "@@ -3,0 +4,1 @@ A-/4:delta",
        ],
        "the ids GetDifferences reported must compare as the two stored versions, got {got}"
    );

    // An object name is a hexadecimal number: its case does not change which object
    // it names.
    input["beforeBlobId"] = json!(before_id.to_ascii_uppercase());
    input["afterBlobId"] = json!(after_id.to_ascii_uppercase());
    let from_upper = ok(&router, "GetBlobDifferences", &input).await;
    assert_eq!(
        from_upper, got,
        "an upper-case object name names the same object, got {from_upper}"
    );
}
