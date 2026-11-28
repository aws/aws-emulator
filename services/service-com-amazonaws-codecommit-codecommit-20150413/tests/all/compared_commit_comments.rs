// SPDX-License-Identifier: CC0-1.0

//! Comments on the comparison between two commits over the wire:
//! `PostCommentForComparedCommit` and `GetCommentsForComparedCommit`, driven as
//! awsJson1_1 requests through the router that serves the real emulator.
//!
//! The rule under test is that a read of a comparison's comments reports, from
//! the service's stored state, exactly the comments posted on that repository and
//! that pair of commits — with the comment's generated id, author, dates and the
//! blobs of the commented file taken from the two commits' stored trees — and
//! that the read changes nothing. The tables below span the class the rule covers
//! (content kind and size, located and unlocated comments, the before and the
//! after version of a file, a comparison with and without a before commit, page
//! sizes at both published bounds) rather than one recorded request.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::repository_lifecycle::{base64, call, emulator_with_engine, is_comment_id, ok, string_at};

/// The file every comparison in this suite is built around.
const FILE_PATH: &str = "docs/notes.md";
/// The before version: three lines.
const BEFORE_CONTENT: &[u8] = b"one\ntwo\nthree\n";
/// The after version: a fourth line, so a position valid in the after version is
/// past the end of the before version.
const AFTER_CONTENT: &[u8] = b"one\ntwo\nthree\nfour\n";

/// The access key id `call` signs with, which is the identity a posted comment is
/// authored by.
const CALLER_AKID: &str = "AKIDTESTKEY";

/// One repository holding two commits of [`FILE_PATH`]: the before and the after
/// of a comparison.
#[derive(Debug, Clone)]
struct Comparison {
    repository: String,
    before_commit_id: String,
    after_commit_id: String,
    before_blob_id: String,
    after_blob_id: String,
}

/// Build that repository through the implemented write path, so every id in the
/// comparison is one the service itself stored.
async fn comparison(
    router: &smithy_server_runtime::MultiServiceRouter,
    repository: &str,
) -> Comparison {
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
            "filePath": FILE_PATH,
            "fileContent": base64(BEFORE_CONTENT),
            "commitMessage": "init",
            "name": "Test Author",
            "email": "author@example.com",
        }),
    )
    .await;
    let before_commit_id = string_at(&first, "/commitId");
    let second = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": FILE_PATH,
            "fileContent": base64(AFTER_CONTENT),
            "parentCommitId": before_commit_id,
            "commitMessage": "add a line",
            "name": "Test Author",
            "email": "author@example.com",
        }),
    )
    .await;
    Comparison {
        repository: repository.to_string(),
        before_commit_id,
        after_commit_id: string_at(&second, "/commitId"),
        before_blob_id: string_at(&first, "/blobId"),
        after_blob_id: string_at(&second, "/blobId"),
    }
}

/// The modeled error code of a failed call, with the status asserted, named for
/// the case under test.
///
/// `pub(crate)` so the sibling pull-request comment suite drives refusals through
/// this one helper rather than a second copy of it.
pub(crate) async fn refused(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> String {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must be refused with HTTP 400, got {status} with {payload}"
    );
    payload
        .get("__type")
        .and_then(Value::as_str)
        .map(|shape| shape.rsplit('#').next().unwrap_or(shape).to_string())
        .unwrap_or_else(|| panic!("case {case:?}: no __type in {payload}"))
}

/// One row of the comment table: what is posted, and what the service must then
/// report for it.
#[derive(Debug, Clone)]
struct CommentCase {
    label: &'static str,
    content: String,
    /// The `location` as sent; `None` comments on the whole comparison.
    sent_location: Option<Value>,
    /// The `location` the service must report (a file path is stored in the form a
    /// git tree holds it, with leading separators removed).
    stored_location: Option<Value>,
    /// Does the request name the before commit? A comparison without one has no
    /// before side to report.
    with_before: bool,
    /// Does the reported comparison carry blob ids? Only a comment on a file does.
    on_file: bool,
    /// The `clientRequestToken` as sent.
    token: Option<&'static str>,
}

/// The table: rows differ along what the rule generalises over — content kind and
/// size (ascii, unicode, multiline, the documented 10,240-character maximum),
/// whether the comment names a location at all, which version of the file it is on
/// (`RelativeFileVersionEnum` = BEFORE | AFTER, from
/// `smithy_query(query="shape",shape="RelativeFileVersionEnum")`), how the path is
/// spelled, whether a position is named, whether the comparison has a before
/// commit, and whether an idempotency token is sent.
fn comment_cases() -> Vec<CommentCase> {
    vec![
        CommentCase {
            label: "whole-comparison",
            content: "a comment on the comparison".to_string(),
            sent_location: None,
            stored_location: None,
            with_before: true,
            on_file: false,
            token: None,
        },
        CommentCase {
            label: "after-version-last-line",
            content: "line four is new".to_string(),
            // Position 4 exists in the after version only.
            sent_location: Some(json!({
                "filePath": FILE_PATH,
                "filePosition": 4,
                "relativeFileVersion": "AFTER",
            })),
            stored_location: Some(json!({
                "filePath": FILE_PATH,
                "filePosition": 4,
                "relativeFileVersion": "AFTER",
            })),
            with_before: true,
            on_file: true,
            token: Some("token-after-version"),
        },
        CommentCase {
            label: "before-version-leading-separator",
            content: "⚙ reseña — ¿sí?\nlínea 2\n".to_string(),
            // A leading separator addresses the same file as none at all.
            sent_location: Some(json!({
                "filePath": format!("/{FILE_PATH}"),
                "filePosition": 3,
                "relativeFileVersion": "BEFORE",
            })),
            stored_location: Some(json!({
                "filePath": FILE_PATH,
                "filePosition": 3,
                "relativeFileVersion": "BEFORE",
            })),
            with_before: true,
            on_file: true,
            token: None,
        },
        CommentCase {
            label: "no-before-commit-defaulted-version",
            content: "the initial commit has no before side".to_string(),
            // No `relativeFileVersion` and no position: the after version of the file.
            sent_location: Some(json!({ "filePath": FILE_PATH })),
            stored_location: Some(json!({ "filePath": FILE_PATH })),
            with_before: false,
            on_file: true,
            token: Some("token-initial"),
        },
        CommentCase {
            label: "maximum-content-length",
            // The documented maximum: "Comments are limited to 10,240 characters."
            content: "x".repeat(10_240),
            sent_location: None,
            stored_location: None,
            with_before: true,
            on_file: false,
            token: None,
        },
        CommentCase {
            label: "location-naming-nothing",
            content: "an empty location places the comment nowhere".to_string(),
            // Every member of the location absent: the comment is on the whole
            // comparison, the same as sending no location at all.
            sent_location: Some(json!({})),
            stored_location: None,
            with_before: true,
            on_file: false,
            token: None,
        },
    ]
}

/// The `location` object the service reported, normalised to the members the model
/// declares so a row's expectation can be compared against it.
///
/// `pub(crate)` for the same reason [`refused`] is: one comparison of a reported
/// location, shared by every comment suite.
pub(crate) fn reported_location(entry: &Value) -> Option<Value> {
    entry.get("location").filter(|value| !value.is_null()).map(
        |location| {
            let mut normalised = json!({});
            for member in ["filePath", "filePosition", "relativeFileVersion"] {
                if let Some(value) = location.get(member).filter(|value| !value.is_null()) {
                    normalised[member] = value.clone();
                }
            }
            normalised
        },
    )
}

#[tokio::test]
async fn a_posted_comment_is_read_back_from_the_comparison_it_was_filed_under() {
    let (router, _engine) = emulator_with_engine();
    let mut comment_ids: BTreeSet<String> = BTreeSet::new();

    for (index, case) in comment_cases().into_iter().enumerate() {
        let label = case.label;
        let comparison = comparison(&router, &format!("comments-{index}")).await;

        let mut post_input = json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.after_commit_id,
            "content": case.content,
        });
        if case.with_before {
            post_input["beforeCommitId"] = json!(comparison.before_commit_id);
        }
        if let Some(location) = &case.sent_location {
            post_input["location"] = location.clone();
        }
        if let Some(token) = case.token {
            post_input["clientRequestToken"] = json!(token);
        }
        let posted = ok(&router, "PostCommentForComparedCommit", &post_input).await;

        // What the post reported about the comparison it filed the comment under.
        let expected_before_commit = case
            .with_before
            .then(|| comparison.before_commit_id.clone());
        let expected_before_blob = (case.on_file && case.with_before)
            .then(|| comparison.before_blob_id.clone());
        let expected_after_blob = case.on_file.then(|| comparison.after_blob_id.clone());
        assert_eq!(
            posted.get("repositoryName").and_then(Value::as_str),
            Some(comparison.repository.as_str()),
            "case {label:?}: repositoryName in {posted}"
        );
        assert_eq!(
            posted.get("afterCommitId").and_then(Value::as_str),
            Some(comparison.after_commit_id.as_str()),
            "case {label:?}: afterCommitId in {posted}"
        );
        assert_eq!(
            posted
                .get("beforeCommitId")
                .and_then(Value::as_str)
                .map(str::to_string),
            expected_before_commit,
            "case {label:?}: beforeCommitId in {posted}"
        );
        assert_eq!(
            posted
                .get("afterBlobId")
                .and_then(Value::as_str)
                .map(str::to_string),
            expected_after_blob,
            "case {label:?}: afterBlobId must be the blob the after commit stores, in {posted}"
        );
        assert_eq!(
            posted
                .get("beforeBlobId")
                .and_then(Value::as_str)
                .map(str::to_string),
            expected_before_blob,
            "case {label:?}: beforeBlobId must be the blob the before commit stores, in {posted}"
        );
        assert_eq!(
            reported_location(&posted),
            case.stored_location,
            "case {label:?}: location in {posted}"
        );

        let comment = posted
            .get("comment")
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| panic!("case {label:?}: no comment in {posted}"))
            .clone();
        let comment_id = string_at(&comment, "/commentId");
        assert!(
            is_comment_id(&comment_id),
            "case {label:?}: commentId must be a colon-joined pair of uuids, got {comment_id:?}"
        );
        assert!(
            comment_ids.insert(comment_id.clone()),
            "case {label:?}: commentId {comment_id} was already issued to another comment"
        );
        assert_eq!(
            comment.get("content").and_then(Value::as_str),
            Some(case.content.as_str()),
            "case {label:?}: content must be what was posted"
        );
        assert_eq!(
            comment.get("authorArn").and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:iam::{}:user/{CALLER_AKID}",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {label:?}: authorArn must be the identity that signed the call, in {comment}"
        );
        assert_eq!(
            comment.get("deleted").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: a posted comment is not deleted, in {comment}"
        );
        assert_eq!(
            comment.get("inReplyTo"),
            None,
            "case {label:?}: a comment posted on a comparison replies to nothing, in {comment}"
        );
        assert_eq!(
            comment.get("clientRequestToken").and_then(Value::as_str),
            case.token,
            "case {label:?}: clientRequestToken must be what was sent, in {comment}"
        );
        let creation_date = comment
            .get("creationDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: no creationDate in {comment}"));
        assert_eq!(
            comment.get("lastModifiedDate").and_then(Value::as_f64),
            Some(creation_date),
            "case {label:?}: a fresh comment was last modified when it was created"
        );

        // The read reports exactly that comment for the comparison it was filed
        // under, computed from state rather than echoed from this request.
        let mut get_input = json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.after_commit_id,
        });
        if case.with_before {
            get_input["beforeCommitId"] = json!(comparison.before_commit_id);
        }
        let read = ok(&router, "GetCommentsForComparedCommit", &get_input).await;
        assert_eq!(
            read.get("nextToken"),
            None,
            "case {label:?}: a single comment needs no continuation token, in {read}"
        );
        let data = read
            .get("commentsForComparedCommitData")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no commentsForComparedCommitData in {read}"))
            .clone();
        assert_eq!(
            data.len(),
            1,
            "case {label:?}: one comment was posted, so one comparison entry is reported: {read}"
        );
        let entry = &data[0];
        for member in [
            "repositoryName",
            "afterCommitId",
            "beforeCommitId",
            "afterBlobId",
            "beforeBlobId",
        ] {
            assert_eq!(
                entry.get(member),
                posted.get(member),
                "case {label:?}: {member} must be what the post reported, in {entry}"
            );
        }
        assert_eq!(
            reported_location(entry),
            case.stored_location,
            "case {label:?}: location in {entry}"
        );
        let comments = entry
            .get("comments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no comments in {entry}"));
        assert_eq!(
            comments.len(),
            1,
            "case {label:?}: the thread holds the one comment posted, in {entry}"
        );
        assert_eq!(
            comments[0], comment,
            "case {label:?}: the comment read back must be the comment posted"
        );

        // A read changes nothing a later caller sees.
        let again = ok(&router, "GetCommentsForComparedCommit", &get_input).await;
        assert_eq!(
            again, read,
            "case {label:?}: a second read must report the same comments"
        );
    }
}

#[tokio::test]
async fn a_read_reports_only_the_comments_of_the_comparison_it_names() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State;

    let (router, engine) = emulator_with_engine();
    let comparison = comparison(&router, "filtered").await;

    // Three comments: two on the before→after comparison, one on the after commit
    // alone (no before side).
    let mut posted: Vec<(&str, String)> = Vec::new();
    for (case, with_before) in [
        ("paired-one", true),
        ("paired-two", true),
        ("initial-only", false),
    ] {
        let mut input = json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.after_commit_id,
            "content": case,
        });
        if with_before {
            input["beforeCommitId"] = json!(comparison.before_commit_id);
        }
        let response = ok(&router, "PostCommentForComparedCommit", &input).await;
        posted.push((case, string_at(&response, "/comment/commentId")));
    }

    /// The contents reported for a read, in the order they were reported.
    fn contents(read: &Value) -> Vec<String> {
        read.get("commentsForComparedCommitData")
            .and_then(Value::as_array)
            .expect("commentsForComparedCommitData")
            .iter()
            .flat_map(|entry| {
                entry
                    .get("comments")
                    .and_then(Value::as_array)
                    .expect("comments")
                    .iter()
                    .map(|comment| string_at(comment, "/content"))
                    .collect::<Vec<String>>()
            })
            .collect()
    }

    // A named before commit narrows the read to that pair, in post order.
    let paired = ok(
        &router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": comparison.repository,
            "beforeCommitId": comparison.before_commit_id,
            "afterCommitId": comparison.after_commit_id,
        }),
    )
    .await;
    assert_eq!(
        contents(&paired),
        vec!["paired-one".to_string(), "paired-two".to_string()],
        "case \"before+after\": only the comments on that pair, oldest first: {paired}"
    );

    // No before commit: every comment posted on that after commit.
    let all = ok(
        &router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.after_commit_id,
        }),
    )
    .await;
    assert_eq!(
        contents(&all),
        vec![
            "paired-one".to_string(),
            "paired-two".to_string(),
            "initial-only".to_string()
        ],
        "case \"after only\": every comment on that after commit: {all}"
    );

    // Another commit of the same repository carries none of them.
    let other = ok(
        &router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.before_commit_id,
        }),
    )
    .await;
    assert_eq!(
        contents(&other),
        Vec::<String>::new(),
        "case \"other after commit\": no comment was posted on it: {other}"
    );

    // Every comment is one record in the repository's own state.
    let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
    let store = state.lock();
    let repository = store
        .repositories
        .get("filtered")
        .expect("the repository written above");
    assert_eq!(
        repository.comments.len(),
        posted.len(),
        "the store must hold one record per posted comment"
    );
    for (case, comment_id) in &posted {
        let record = repository
            .comments
            .get(comment_id)
            .unwrap_or_else(|| panic!("case {case:?}: no stored comment {comment_id}"));
        assert_eq!(
            record.content, *case,
            "case {case:?}: the stored content must be what was posted"
        );
    }
}

#[tokio::test]
async fn paging_follows_max_results_and_its_published_bounds() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::{
        COMMENTS_PAGE_DEFAULT, COMMENTS_PAGE_MAX,
    };

    let (router, _engine) = emulator_with_engine();
    let comparison = comparison(&router, "paged").await;

    // Seven comments on one comparison, so every page size below is exercised
    // with a partial last page.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        let response = ok(
            &router,
            "PostCommentForComparedCommit",
            &json!({
                "repositoryName": comparison.repository,
                "beforeCommitId": comparison.before_commit_id,
                "afterCommitId": comparison.after_commit_id,
                "content": format!("comment {index}"),
            }),
        )
        .await;
        expected.push(string_at(&response, "/comment/commentId"));
    }

    // Every page size — including both published bounds, 1 and 500 — walks the
    // same comments, in the same order, exactly once.
    for page_size in [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(7),
        Some(COMMENTS_PAGE_DEFAULT),
        Some(COMMENTS_PAGE_MAX),
    ] {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({
                "repositoryName": comparison.repository,
                "beforeCommitId": comparison.before_commit_id,
                "afterCommitId": comparison.after_commit_id,
            });
            if let Some(page_size) = page_size {
                input["maxResults"] = json!(page_size);
            }
            if let Some(token) = &next_token {
                input["nextToken"] = json!(token);
            }
            let read = ok(&router, "GetCommentsForComparedCommit", &input).await;
            let data = read
                .get("commentsForComparedCommitData")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no data in {read}"))
                .clone();
            assert!(
                data.len() <= page_size.unwrap_or(COMMENTS_PAGE_DEFAULT) as usize,
                "case {case:?}: a page must not exceed maxResults, got {} entries",
                data.len()
            );
            for entry in &data {
                walked.push(string_at(entry, "/comments/0/commentId"));
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            next_token = read
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every comment once, in post order"
        );
    }

    // One past each bound is not a valid number of results.
    for max_results in [0, -1, COMMENTS_PAGE_MAX + 1] {
        let case = format!("maxResults={max_results}");
        let code = refused(
            &router,
            &case,
            "GetCommentsForComparedCommit",
            &json!({
                "repositoryName": comparison.repository,
                "afterCommitId": comparison.after_commit_id,
                "maxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            code, "InvalidMaxResultsException",
            "case {case:?}: a maxResults outside 1..={COMMENTS_PAGE_MAX} must be refused"
        );
    }

    // A continuation token the service did not issue is not valid.
    let code = refused(
        &router,
        "nextToken=not-a-token",
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.after_commit_id,
            "nextToken": "not-a-token",
        }),
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "a continuation token the service never issued must be refused"
    );
}

#[tokio::test]
async fn a_repeated_client_request_token_posts_one_comment() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State;

    let (router, engine) = emulator_with_engine();
    let comparison = comparison(&router, "idempotent").await;
    let request = json!({
        "repositoryName": comparison.repository,
        "beforeCommitId": comparison.before_commit_id,
        "afterCommitId": comparison.after_commit_id,
        "content": "posted once",
        "clientRequestToken": "token-repeated",
    });

    let first = ok(&router, "PostCommentForComparedCommit", &request).await;
    let again = ok(&router, "PostCommentForComparedCommit", &request).await;
    assert_eq!(
        again, first,
        "the same request with the same token must return the first comment"
    );

    // A changed parameter under the same token is refused rather than posting a
    // second comment.
    for (case, member, value) in [
        ("content", "content", json!("posted twice")),
        ("location", "location", json!({ "filePath": FILE_PATH })),
    ] {
        let mut changed = request.clone();
        changed[member] = value;
        let code = refused(&router, case, "PostCommentForComparedCommit", &changed).await;
        assert_eq!(
            code, "IdempotencyParameterMismatchException",
            "case {case:?}: a reused token with a changed parameter must be refused"
        );
    }

    let read = ok(
        &router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": comparison.repository,
            "beforeCommitId": comparison.before_commit_id,
            "afterCommitId": comparison.after_commit_id,
        }),
    )
    .await;
    assert_eq!(
        read.get("commentsForComparedCommitData")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "the repeated post must have stored one comment: {read}"
    );
    assert_eq!(
        engine
            .state::<ComAmazonawsCodecommitCodecommit20150413State>()
            .lock()
            .repositories
            .get("idempotent")
            .expect("the repository written above")
            .comments
            .len(),
        1,
        "the refused posts must not have been stored"
    );
}

#[tokio::test]
async fn a_comment_the_comparison_cannot_carry_is_refused_with_the_modeled_error() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::{
        ComAmazonawsCodecommitCodecommit20150413State, MAX_COMMENT_CHARACTERS,
    };

    let (router, engine) = emulator_with_engine();
    let comparison = comparison(&router, "refusals").await;
    // A well-formed object name no commit of this repository has.
    let unknown_commit = "0".repeat(40);

    let post = |overrides: Value| {
        let mut input = json!({
            "repositoryName": comparison.repository,
            "beforeCommitId": comparison.before_commit_id,
            "afterCommitId": comparison.after_commit_id,
            "content": "a comment",
        });
        let Value::Object(overrides) = overrides else {
            panic!("overrides must be an object");
        };
        for (member, value) in overrides {
            if value.is_null() {
                input.as_object_mut().expect("an object").remove(&member);
            } else {
                input[member] = value;
            }
        }
        input
    };

    // Each row is a request that cannot be satisfied, and the error the model
    // declares for it.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "unknown repository",
            post(json!({ "repositoryName": "no-such-repository" })),
            "RepositoryDoesNotExistException",
        ),
        (
            "unknown after commit",
            post(json!({ "afterCommitId": unknown_commit, "beforeCommitId": null })),
            "CommitDoesNotExistException",
        ),
        (
            "unknown before commit",
            post(json!({ "beforeCommitId": unknown_commit })),
            "CommitDoesNotExistException",
        ),
        (
            "malformed after commit",
            post(json!({ "afterCommitId": "not-a-commit-id", "beforeCommitId": null })),
            "InvalidCommitIdException",
        ),
        (
            "same before and after commit",
            post(json!({ "beforeCommitId": comparison.after_commit_id })),
            "BeforeCommitIdAndAfterCommitIdAreSameException",
        ),
        (
            "empty content",
            post(json!({ "content": "" })),
            "CommentContentRequiredException",
        ),
        (
            "content one character past the limit",
            post(json!({ "content": "x".repeat(MAX_COMMENT_CHARACTERS + 1) })),
            "CommentContentSizeLimitExceededException",
        ),
        (
            "path no version of the comparison holds",
            post(json!({ "location": { "filePath": "docs/absent.md" } })),
            "PathDoesNotExistException",
        ),
        (
            "position past the end of the commented version",
            // The before version has three lines; the after version has four.
            post(json!({
                "location": { "filePath": FILE_PATH, "filePosition": 4, "relativeFileVersion": "BEFORE" }
            })),
            "InvalidFilePositionException",
        ),
        (
            "position below the first line",
            post(json!({
                "location": { "filePath": FILE_PATH, "filePosition": 0, "relativeFileVersion": "AFTER" }
            })),
            "InvalidFilePositionException",
        ),
        (
            "location naming no file",
            post(json!({ "location": { "filePosition": 1 } })),
            "InvalidFileLocationException",
        ),
        (
            "before version of a comparison with no before commit",
            post(json!({
                "beforeCommitId": null,
                "location": {
                    "filePath": FILE_PATH,
                    "filePosition": 1,
                    "relativeFileVersion": "BEFORE",
                }
            })),
            "InvalidRelativeFileVersionEnumException",
        ),
    ];
    for (case, input, expected) in rows {
        let code = refused(&router, case, "PostCommentForComparedCommit", &input).await;
        assert_eq!(code, expected, "case {case:?}: PostCommentForComparedCommit");
    }

    // The read refuses the same repository and commit faults.
    for (case, input, expected) in [
        (
            "read of an unknown repository",
            json!({ "repositoryName": "no-such-repository", "afterCommitId": comparison.after_commit_id }),
            "RepositoryDoesNotExistException",
        ),
        (
            "read of an unknown commit",
            json!({ "repositoryName": comparison.repository, "afterCommitId": unknown_commit }),
            "CommitDoesNotExistException",
        ),
        (
            "read of a malformed commit id",
            json!({ "repositoryName": comparison.repository, "afterCommitId": "not-a-commit-id" }),
            "InvalidCommitIdException",
        ),
    ] {
        let code = refused(&router, case, "GetCommentsForComparedCommit", &input).await;
        assert_eq!(code, expected, "case {case:?}: GetCommentsForComparedCommit");
    }

    // Not one refused post was stored.
    assert_eq!(
        engine
            .state::<ComAmazonawsCodecommitCodecommit20150413State>()
            .lock()
            .repositories
            .get("refusals")
            .expect("the repository written above")
            .comments
            .len(),
        0,
        "a refused post must store nothing"
    );
    let read = ok(
        &router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": comparison.repository,
            "afterCommitId": comparison.after_commit_id,
        }),
    )
    .await;
    assert_eq!(
        read.get("commentsForComparedCommitData")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a comparison nobody commented on reports no comments: {read}"
    );
}
