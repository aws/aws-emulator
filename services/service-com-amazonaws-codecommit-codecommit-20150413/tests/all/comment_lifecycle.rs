// SPDX-License-Identifier: CC0-1.0

//! Comments on the comparison between two commits, over the wire:
//! `PostCommentForComparedCommit`, `GetComment`, `UpdateComment`,
//! `DeleteCommentContent` and `PutCommentReaction`, driven as awsJson1_1 requests
//! through the router that serves the real emulator.
//!
//! The rule under test: posting a comment persists it against the comparison it
//! names, and every later answer about that comment — the post's own, a read, an
//! edit, a deletion, a reaction — is computed from that stored record. So a
//! comment keeps its id, its author and its creation date; its content and
//! reactions are whatever the last accepted write left; a reporting call changes
//! nothing; and a request that names a repository, commit, path, position,
//! comment or reaction the service does not hold is refused with the error the
//! model declares for it. The tables below span the class the rule covers —
//! content kinds and the documented size bound, locations on either side of the
//! comparison, every form of every reaction in the service's table, two callers —
//! rather than one recorded request.

use serde_json::{json, Value};

use crate::repository_lifecycle::{base64, error, is_comment_id, ok, string_at};
use crate::wire::{
    caller_arn, emulator_with_callers, error_as, hold_long_term_key, ok_as, DEFAULT_ACCESS_KEY_ID,
};

/// The file every fixture repository holds, and the two versions of it the two
/// commits carry. The line counts (2 and 3) are what a comment's `filePosition`
/// is checked against.
pub(crate) const PATH: &str = "README.md";
const BEFORE_CONTENT: &[u8] = b"alpha\nbeta\n";
const AFTER_CONTENT: &[u8] = b"alpha\nbeta\ngamma\n";
const BEFORE_LINES: i64 = 2;
const AFTER_LINES: i64 = 3;

/// A second principal, so the tests can tell callers apart where the service
/// does: only an author may edit a comment, and a reaction belongs to the caller
/// that left it.
pub(crate) const OTHER_ACCESS_KEY_ID: &str = "AKIDOTHERUSER";

/// The documented comment size bound: "The comment is too large. Comments are
/// limited to 10,240 characters."
/// (`smithy_query(query="shape",shape="CommentContentSizeLimitExceededException")`).
pub(crate) const MAX_COMMENT_CHARACTERS: usize = 10_240;

/// A repository holding one file at two commits: everything a comment on a
/// comparison needs.
pub(crate) struct Fixture {
    pub(crate) router: smithy_server_runtime::MultiServiceRouter,
    /// The shared engine, so a suite needing MORE principals than this fixture
    /// names can give one a long-term access key of its own.
    pub(crate) engine: std::sync::Arc<engine::InMemoryEngine>,
    pub(crate) repository: String,
    pub(crate) before_commit: String,
    pub(crate) after_commit: String,
    pub(crate) before_blob: String,
    pub(crate) after_blob: String,
}

impl Fixture {
    /// Make `access_key_id` a long-term key the account holds for a user of its own,
    /// and answer that user's ARN: the principal a call signed with it speaks for.
    pub(crate) fn hold(&self, access_key_id: &str) -> String {
        hold_long_term_key(&self.engine, access_key_id)
    }
}

pub(crate) async fn fixture(repository: &str) -> Fixture {
    let (router, engine) = emulator_with_callers(&[OTHER_ACCESS_KEY_ID]);
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let first = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(BEFORE_CONTENT),
            "commitMessage": "the before version",
        }),
    )
    .await;
    let before_commit = string_at(&first, "/commitId");
    let second = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(AFTER_CONTENT),
            "parentCommitId": before_commit,
            "commitMessage": "the after version",
        }),
    )
    .await;
    Fixture {
        router,
        engine,
        repository: repository.to_string(),
        before_blob: string_at(&first, "/blobId"),
        after_blob: string_at(&second, "/blobId"),
        before_commit,
        after_commit: string_at(&second, "/commitId"),
    }
}

/// One row of the post table.
struct PostCase {
    label: &'static str,
    content: String,
    /// Whether the request names the before commit (a comment on the initial
    /// commit has no before side).
    with_before: bool,
    /// The `location` as sent, and the one the service must report back.
    sent_location: Option<Value>,
    expected_location: Option<Value>,
}

/// The table: rows differ along what the rule generalises over — content kind and
/// size (ASCII, unicode, multi-line, the documented 10,240-character maximum),
/// whether the comparison has a before side, and where in the comparison the
/// comment sits (nowhere, a line of the after version, a line of the before
/// version, a path written with a leading separator, a path with no position).
fn post_cases() -> Vec<PostCase> {
    vec![
        PostCase {
            label: "no location, no before commit",
            content: "Looks good to me.".to_string(),
            with_before: false,
            sent_location: None,
            expected_location: None,
        },
        PostCase {
            label: "unicode content on a two-ended comparison",
            content: "✅ se ve bien — ¿lo fusionamos?".to_string(),
            with_before: true,
            sent_location: None,
            expected_location: None,
        },
        PostCase {
            label: "the first line of the after version",
            content: "line one changed".to_string(),
            with_before: true,
            sent_location: Some(json!({
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "AFTER",
            })),
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "AFTER",
            })),
        },
        PostCase {
            label: "the last line of the before version",
            content: "this line went away".to_string(),
            with_before: true,
            sent_location: Some(json!({
                "filePath": PATH,
                "filePosition": BEFORE_LINES,
                "relativeFileVersion": "BEFORE",
            })),
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": BEFORE_LINES,
                "relativeFileVersion": "BEFORE",
            })),
        },
        PostCase {
            label: "a leading separator addresses the same file",
            content: "multi\nline\ncomment".to_string(),
            with_before: false,
            sent_location: Some(json!({
                "filePath": format!("/{PATH}"),
                "filePosition": AFTER_LINES,
                "relativeFileVersion": "AFTER",
            })),
            // The path is reported in the form a git tree holds it, with the
            // leading separator removed.
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": AFTER_LINES,
                "relativeFileVersion": "AFTER",
            })),
        },
        PostCase {
            label: "a file with no position",
            content: "about the file as a whole".to_string(),
            with_before: true,
            sent_location: Some(json!({ "filePath": PATH })),
            expected_location: Some(json!({ "filePath": PATH })),
        },
        PostCase {
            label: "the longest comment the service accepts",
            content: "x".repeat(MAX_COMMENT_CHARACTERS),
            with_before: true,
            sent_location: None,
            expected_location: None,
        },
    ]
}

#[tokio::test]
async fn a_posted_comment_is_read_back_from_state() {
    let fixture = fixture("comment-post-repo").await;
    let author = caller_arn(DEFAULT_ACCESS_KEY_ID);
    let mut ids: Vec<String> = Vec::new();

    for case in post_cases() {
        let label = case.label;
        let mut input = json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "content": case.content,
        });
        if case.with_before {
            input["beforeCommitId"] = json!(fixture.before_commit);
        }
        if let Some(location) = &case.sent_location {
            input["location"] = location.clone();
        }

        let posted = ok(&fixture.router, "PostCommentForComparedCommit", &input).await;

        assert_eq!(
            posted.pointer("/repositoryName"),
            Some(&json!(fixture.repository)),
            "case {label:?}: expected the repository echoed, got {posted}"
        );
        assert_eq!(
            posted.pointer("/afterCommitId"),
            Some(&json!(fixture.after_commit)),
            "case {label:?}: expected the after commit, got {posted}"
        );
        assert_eq!(
            posted.pointer("/beforeCommitId").cloned(),
            case.with_before.then(|| json!(fixture.before_commit)),
            "case {label:?}: expected the before commit only when one was sent, got {posted}"
        );
        assert_eq!(
            posted.pointer("/location").cloned(),
            case.expected_location,
            "case {label:?}: expected the stored location, got {posted}"
        );
        // The blob ids name the file at the location in each side of the
        // comparison — the ids `PutFile` reported for those two writes — and
        // nothing at all when the comment points at no file.
        let expects_blobs = case.sent_location.is_some();
        assert_eq!(
            posted.pointer("/afterBlobId").cloned(),
            expects_blobs.then(|| json!(fixture.after_blob)),
            "case {label:?}: expected the after blob of {PATH}, got {posted}"
        );
        assert_eq!(
            posted.pointer("/beforeBlobId").cloned(),
            (expects_blobs && case.with_before).then(|| json!(fixture.before_blob)),
            "case {label:?}: expected the before blob of {PATH}, got {posted}"
        );

        let comment_id = string_at(&posted, "/comment/commentId");
        assert!(
            is_comment_id(&comment_id),
            "case {label:?}: expected a comment id of the form <uuid>:<uuid>, got {comment_id:?}"
        );
        assert!(
            !ids.contains(&comment_id),
            "case {label:?}: expected a comment id no other post took, got {comment_id:?}"
        );
        ids.push(comment_id.clone());

        let comment = posted
            .pointer("/comment")
            .unwrap_or_else(|| panic!("case {label:?}: no comment in {posted}"));
        assert_eq!(
            comment.pointer("/content"),
            Some(&json!(case.content)),
            "case {label:?}: expected the content as posted, got {comment}"
        );
        assert_eq!(
            comment.pointer("/authorArn"),
            Some(&json!(author)),
            "case {label:?}: expected the calling principal as author, got {comment}"
        );
        assert!(
            comment
                .pointer("/deleted")
                .is_none_or(|deleted| deleted == &json!(false)),
            "case {label:?}: expected a comment that is not deleted, got {comment}"
        );
        assert_eq!(
            comment.pointer("/creationDate"),
            comment.pointer("/lastModifiedDate"),
            "case {label:?}: expected an unedited comment's dates to agree, got {comment}"
        );
        assert!(
            comment.pointer("/inReplyTo").is_none(),
            "case {label:?}: expected no inReplyTo on a top-level comment, got {comment}"
        );
        assert_eq!(
            comment.pointer("/reactionCounts"),
            Some(&json!({})),
            "case {label:?}: expected an empty reaction count map on a fresh comment, got {comment}"
        );

        // The round trip: the reader answers with the stored record, member for
        // member, and reading it twice changes nothing.
        let read = ok(
            &fixture.router,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            read.pointer("/comment"),
            Some(comment),
            "case {label:?}: expected GetComment to report the posted comment, got {read}"
        );
        let again = ok(
            &fixture.router,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            again, read,
            "case {label:?}: expected a read to leave the comment unchanged, got {again}"
        );
    }
}

#[tokio::test]
async fn a_client_request_token_makes_a_post_idempotent() {
    let fixture = fixture("comment-token-repo").await;
    let input = |token: &str, content: &str| {
        json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "beforeCommitId": fixture.before_commit,
            "content": content,
            "clientRequestToken": token,
        })
    };

    let first = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &input("token-one", "the first word"),
    )
    .await;
    let replay = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &input("token-one", "the first word"),
    )
    .await;
    assert_eq!(
        replay, first,
        "case \"same token, same parameters\": expected the first request's answer, got {replay}"
    );

    // The same token with a changed parameter is the mismatch the model names.
    let code = error(
        &fixture.router,
        "PostCommentForComparedCommit",
        &input("token-one", "a different word"),
        400,
    )
    .await;
    assert_eq!(
        code, "IdempotencyParameterMismatchException",
        "case \"same token, changed parameters\": expected IdempotencyParameterMismatchException, got {code:?}"
    );

    // A different token is a different comment.
    let second = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &input("token-two", "the first word"),
    )
    .await;
    assert_ne!(
        string_at(&second, "/comment/commentId"),
        string_at(&first, "/comment/commentId"),
        "case \"a second token\": expected a new comment, got {second}"
    );
    assert_eq!(
        string_at(&second, "/comment/clientRequestToken"),
        "token-two",
        "case \"a second token\": expected the token on the stored comment, got {second}"
    );

    // A token is judged against the request it was first accepted for, so
    // editing the comment afterwards does not turn a replay of that request into
    // a parameter mismatch: it still answers with that same comment.
    let comment_id = string_at(&first, "/comment/commentId");
    ok(
        &fixture.router,
        "UpdateComment",
        &json!({ "commentId": comment_id, "content": "the first word, edited" }),
    )
    .await;
    let after_edit = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &input("token-one", "the first word"),
    )
    .await;
    assert_eq!(
        string_at(&after_edit, "/comment/commentId"),
        comment_id,
        "case \"a replay after an edit\": expected the comment the token first created, got {after_edit}"
    );
    assert_eq!(
        after_edit.pointer("/comment/content"),
        Some(&json!("the first word, edited")),
        "case \"a replay after an edit\": expected the stored content, got {after_edit}"
    );
}

/// One row of the post-refusal table: a patch over a valid request, and the error
/// the model declares for it.
struct RefusalCase {
    label: &'static str,
    patch: Value,
    expected: &'static str,
}

#[tokio::test]
async fn a_post_that_names_what_the_service_does_not_hold_is_refused() {
    let fixture = fixture("comment-refusal-repo").await;
    // A commit id of the right form that the repository does not hold.
    let absent_commit = "f".repeat(40);

    let cases = vec![
        RefusalCase {
            label: "an unknown repository",
            patch: json!({ "repositoryName": "no-such-repository" }),
            expected: "RepositoryDoesNotExistException",
        },
        RefusalCase {
            label: "an after commit the repository does not hold",
            patch: json!({ "afterCommitId": absent_commit }),
            expected: "CommitDoesNotExistException",
        },
        RefusalCase {
            label: "a before commit the repository does not hold",
            patch: json!({ "beforeCommitId": absent_commit }),
            expected: "CommitDoesNotExistException",
        },
        RefusalCase {
            label: "an after commit id that is not an object name",
            patch: json!({ "afterCommitId": "not-a-commit-id" }),
            expected: "InvalidCommitIdException",
        },
        RefusalCase {
            label: "a blank after commit id",
            patch: json!({ "afterCommitId": "" }),
            expected: "CommitIdRequiredException",
        },
        RefusalCase {
            label: "the same commit on both sides",
            patch: json!({ "beforeCommitId": fixture.after_commit }),
            expected: "BeforeCommitIdAndAfterCommitIdAreSameException",
        },
        RefusalCase {
            label: "an empty comment",
            patch: json!({ "content": "" }),
            expected: "CommentContentRequiredException",
        },
        RefusalCase {
            label: "one character past the documented comment limit",
            patch: json!({ "content": "x".repeat(MAX_COMMENT_CHARACTERS + 1) }),
            expected: "CommentContentSizeLimitExceededException",
        },
        RefusalCase {
            label: "a location with a position but no path",
            patch: json!({ "location": { "filePosition": 1 } }),
            expected: "InvalidFileLocationException",
        },
        RefusalCase {
            label: "a path that cannot name a file in a tree",
            patch: json!({ "location": { "filePath": "../escape.md" } }),
            expected: "InvalidPathException",
        },
        RefusalCase {
            label: "a path the commit does not hold",
            patch: json!({ "location": { "filePath": "docs/absent.md" } }),
            expected: "PathDoesNotExistException",
        },
        RefusalCase {
            label: "a position before the first line",
            patch: json!({ "location": { "filePath": PATH, "filePosition": 0 } }),
            expected: "InvalidFilePositionException",
        },
        RefusalCase {
            label: "a position past the last line of the after version",
            patch: json!({
                "location": {
                    "filePath": PATH,
                    "filePosition": AFTER_LINES + 1,
                    "relativeFileVersion": "AFTER",
                },
            }),
            expected: "InvalidFilePositionException",
        },
        RefusalCase {
            label: "a position past the last line of the before version",
            patch: json!({
                "location": {
                    "filePath": PATH,
                    "filePosition": BEFORE_LINES + 1,
                    "relativeFileVersion": "BEFORE",
                },
            }),
            expected: "InvalidFilePositionException",
        },
        RefusalCase {
            label: "a file version outside the modeled enum",
            patch: json!({ "location": { "filePath": PATH, "relativeFileVersion": "SIDEWAYS" } }),
            expected: "InvalidRelativeFileVersionEnumException",
        },
    ];

    for case in cases {
        let mut input = json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "beforeCommitId": fixture.before_commit,
            "content": "a comment",
        });
        for (member, value) in case
            .patch
            .as_object()
            .expect("each patch is an object")
            .clone()
        {
            input[member] = value;
        }
        let code = error(
            &fixture.router,
            "PostCommentForComparedCommit",
            &input,
            400,
        )
        .await;
        assert_eq!(
            code, case.expected,
            "case {:?}: expected {}, got {code:?}",
            case.label, case.expected
        );
    }

    // BEFORE needs a before side to be read in, so it is not valid on a
    // comparison that has none.
    let code = error(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "content": "a comment",
            "location": {
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "BEFORE",
            },
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidRelativeFileVersionEnumException",
        "case \"BEFORE with no before commit\": expected InvalidRelativeFileVersionEnumException, got {code:?}"
    );
}

#[tokio::test]
async fn a_comment_id_that_names_no_comment_is_refused() {
    let fixture = fixture("comment-reader-repo").await;
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a blank comment id",
            json!({ "commentId": "  " }),
            "CommentIdRequiredException",
        ),
        (
            "a comment id that is not a UUID pair",
            json!({ "commentId": "comment-1" }),
            "InvalidCommentIdException",
        ),
        (
            "half a comment id: a single UUID",
            json!({ "commentId": "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b" }),
            "InvalidCommentIdException",
        ),
        (
            "a well-formed comment id the service never minted",
            json!({
                "commentId":
                    "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:1f9c5a2b-2d3e-4f50-9b0c-1d2e3f4a5b6c",
            }),
            "CommentDoesNotExistException",
        ),
    ];

    // Every operation that takes a `commentId` classifies it the same way: the
    // other members each operation requires are filled in, so the id is the only
    // thing wrong with the request.
    let operations: Vec<(&str, Value)> = vec![
        ("GetComment", json!({})),
        ("DeleteCommentContent", json!({})),
        ("UpdateComment", json!({ "content": "an edit" })),
        (
            "PutCommentReaction",
            json!({ "reactionValue": ":thumbsup:" }),
        ),
    ];
    for (operation, extra) in operations {
        for (label, input, expected) in &cases {
            let mut input = input.clone();
            for (member, value) in extra.as_object().expect("each extra is an object") {
                input[member.as_str()] = value.clone();
            }
            let code = error(&fixture.router, operation, &input, 400).await;
            assert_eq!(
                code, *expected,
                "case {label:?} on {operation}: expected {expected}, got {code:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_comment_is_edited_and_deleted_only_by_its_author() {
    let fixture = fixture("comment-edit-repo").await;
    let posted = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "beforeCommitId": fixture.before_commit,
            "content": "the first draft",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");
    let creation_date = posted.pointer("/comment/creationDate").cloned();

    // "Only comment authors can modify or delete their comments."
    let code = error_as(
        &fixture.router,
        OTHER_ACCESS_KEY_ID,
        "UpdateComment",
        &json!({ "commentId": comment_id, "content": "someone else's edit" }),
        400,
    )
    .await;
    assert_eq!(
        code, "CommentNotCreatedByCallerException",
        "case \"an edit by another caller\": expected CommentNotCreatedByCallerException, got {code:?}"
    );
    let unchanged = ok(
        &fixture.router,
        "GetComment",
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert_eq!(
        unchanged.pointer("/comment/content"),
        Some(&json!("the first draft")),
        "case \"a refused edit\": expected the stored content untouched, got {unchanged}"
    );

    // The author's edit replaces the content and leaves the identity and the
    // creation date alone.
    for content in ["the second draft", "the third ✍ draft"] {
        let updated = ok(
            &fixture.router,
            "UpdateComment",
            &json!({ "commentId": comment_id, "content": content }),
        )
        .await;
        assert_eq!(
            updated.pointer("/comment/content"),
            Some(&json!(content)),
            "case {content:?}: expected the new content, got {updated}"
        );
        assert_eq!(
            string_at(&updated, "/comment/commentId"),
            comment_id,
            "case {content:?}: expected the comment to keep its id, got {updated}"
        );
        assert_eq!(
            updated.pointer("/comment/creationDate").cloned(),
            creation_date,
            "case {content:?}: expected the creation date to stand, got {updated}"
        );
        let read = ok(
            &fixture.router,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            read.pointer("/comment"),
            updated.pointer("/comment"),
            "case {content:?}: expected the edit to be what a later read returns, got {read}"
        );
    }

    // An edit is refused on the same grounds a post is.
    for (label, content, expected) in [
        ("a blank edit", String::from(" "), "CommentContentRequiredException"),
        (
            "one character past the comment limit",
            "y".repeat(MAX_COMMENT_CHARACTERS + 1),
            "CommentContentSizeLimitExceededException",
        ),
    ] {
        let code = error(
            &fixture.router,
            "UpdateComment",
            &json!({ "commentId": comment_id, "content": content }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code:?}"
        );
    }

    // Deleting the content keeps the comment, reporting itself as deleted.
    let deleted = ok(
        &fixture.router,
        "DeleteCommentContent",
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert_eq!(
        deleted.pointer("/comment/deleted"),
        Some(&json!(true)),
        "case \"a deleted comment\": expected deleted to be true, got {deleted}"
    );
    assert!(
        deleted.pointer("/comment/content").is_none(),
        "case \"a deleted comment\": expected no content, got {deleted}"
    );
    assert_eq!(
        string_at(&deleted, "/comment/commentId"),
        comment_id,
        "case \"a deleted comment\": expected the comment to keep its id, got {deleted}"
    );

    // "You cannot edit or delete a deleted comment."
    for (operation, input) in [
        ("DeleteCommentContent", json!({ "commentId": comment_id })),
        (
            "UpdateComment",
            json!({ "commentId": comment_id, "content": "after the deletion" }),
        ),
        (
            "PutCommentReaction",
            json!({ "commentId": comment_id, "reactionValue": ":thumbsup:" }),
        ),
        ("GetComment", json!({ "commentId": comment_id })),
    ] {
        let code = error(&fixture.router, operation, &input, 400).await;
        assert_eq!(
            code, "CommentDeletedException",
            "case {operation:?} on a deleted comment: expected CommentDeletedException, got {code:?}"
        );
    }
}

/// Every form of every reaction, with the reaction VALUE it is reported as on a
/// comment (`callerReactions` and the keys of `reactionCounts`, both
/// `ReactionValue` —
/// `smithy_query(query="shape",shape="ReactionCountsMap,CallerReactions")`). The
/// rows are projected out of the crate's one test-side reaction catalog
/// ([`crate::comment_reaction_surfacing::CATALOG`]), which names its source.
fn reaction_forms() -> Vec<(String, &'static str)> {
    crate::comment_reaction_surfacing::sent_forms()
        .into_iter()
        .map(|(sent, row)| (sent, row.name))
        .collect()
}

#[tokio::test]
async fn a_reaction_is_one_per_caller_and_counted_from_state() {
    let fixture = fixture("comment-reaction-repo").await;
    let posted = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "content": "react to me",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    /// The comment as `access_key_id` sees it.
    async fn read(
        router: &smithy_server_runtime::MultiServiceRouter,
        access_key_id: &str,
        comment_id: &str,
    ) -> Value {
        ok_as(
            router,
            access_key_id,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await
    }

    // Every accepted form of every reaction: the caller holds exactly that one
    // reaction afterwards, reported under the reaction's own name.
    for (sent, reported) in reaction_forms() {
        ok(
            &fixture.router,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": sent }),
        )
        .await;
        let comment = read(&fixture.router, DEFAULT_ACCESS_KEY_ID, &comment_id).await;
        assert_eq!(
            comment.pointer("/comment/callerReactions"),
            Some(&json!([reported])),
            "case {sent:?}: expected the caller's one reaction, got {comment}"
        );
        assert_eq!(
            comment.pointer(&format!("/comment/reactionCounts/{reported}")),
            Some(&json!(1)),
            "case {sent:?}: expected one principal holding it, got {comment}"
        );
        assert_eq!(
            comment
                .pointer("/comment/reactionCounts")
                .and_then(Value::as_object)
                .map(serde_json::Map::len),
            Some(1),
            "case {sent:?}: expected the caller's previous reaction replaced, got {comment}"
        );
    }

    // A second caller's reaction is counted beside the first, and each caller
    // sees only its own in `callerReactions`.
    ok_as(
        &fixture.router,
        OTHER_ACCESS_KEY_ID,
        "PutCommentReaction",
        &json!({ "commentId": comment_id, "reactionValue": "🎉" }),
    )
    .await;
    let as_author = read(&fixture.router, DEFAULT_ACCESS_KEY_ID, &comment_id).await;
    assert_eq!(
        as_author.pointer("/comment/reactionCounts"),
        Some(&json!({ "ROCKET": 1, "HOORAY": 1 })),
        "case \"two callers\": expected one count per reaction held, got {as_author}"
    );
    assert_eq!(
        as_author.pointer("/comment/callerReactions"),
        Some(&json!(["ROCKET"])),
        "case \"two callers\": expected the author's own reaction, got {as_author}"
    );
    let as_other = read(&fixture.router, OTHER_ACCESS_KEY_ID, &comment_id).await;
    assert_eq!(
        as_other.pointer("/comment/callerReactions"),
        Some(&json!(["HOORAY"])),
        "case \"two callers\": expected the other caller's own reaction, got {as_other}"
    );

    // A blank value, and the documented `none`, remove the caller's reaction.
    for removal in ["", "none", "NONE", "  "] {
        ok(
            &fixture.router,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": ":eyes:" }),
        )
        .await;
        ok(
            &fixture.router,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": removal }),
        )
        .await;
        let comment = read(&fixture.router, DEFAULT_ACCESS_KEY_ID, &comment_id).await;
        assert!(
            comment.pointer("/comment/callerReactions").is_none(),
            "case {removal:?}: expected the caller's reaction removed, got {comment}"
        );
        assert_eq!(
            comment.pointer("/comment/reactionCounts"),
            Some(&json!({ "HOORAY": 1 })),
            "case {removal:?}: expected the other caller's reaction to stand, got {comment}"
        );
    }

    // A value no row of the reaction table names — in any of the four forms — is
    // refused.
    for invalid in [":shrug:", "SHRUG", "LIKE", "😀", ":+2:", "U+1F600", "thumbs up"] {
        let code = error(
            &fixture.router,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": invalid }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidReactionValueException",
            "case {invalid:?}: expected InvalidReactionValueException, got {code:?}"
        );
    }
}

/// How many principals' reactions one comment holds, mirroring
/// `state::MAX_REACTIONS_PER_COMMENT`: the principal a reaction is keyed by comes
/// from the caller's own credential, so the comment is bounded on it.
const MAX_REACTIONS_PER_COMMENT: usize = 100;

#[tokio::test]
async fn a_comment_holds_a_bounded_number_of_principals_reactions() {
    let fixture = fixture("comment-reaction-bound-repo").await;
    let posted = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "content": "reacted to by a crowd",
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    // One reaction per principal, up to the bound, each accepted.
    for index in 0..MAX_REACTIONS_PER_COMMENT {
        let reactor = format!("AKIDREACTOR{index}");
        fixture.hold(&reactor);
        ok_as(
            &fixture.router,
            &reactor,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": ":eyes:" }),
        )
        .await;
    }

    // A principal the comment does not hold yet is refused past the bound …
    let one_too_many = "AKIDONETOOMANY";
    fixture.hold(one_too_many);
    let code = error_as(
        &fixture.router,
        one_too_many,
        "PutCommentReaction",
        &json!({ "commentId": comment_id, "reactionValue": ":eyes:" }),
        400,
    )
    .await;
    assert_eq!(
        code, "ReactionLimitExceededException",
        "case \"one principal past the bound\": expected ReactionLimitExceededException, got {code:?}"
    );

    // … while a principal that already holds one still gets to change it, and
    // the counts stay at the bound.
    ok_as(
        &fixture.router,
        "AKIDREACTOR0",
        "PutCommentReaction",
        &json!({ "commentId": comment_id, "reactionValue": ":rocket:" }),
    )
    .await;
    let comment = ok_as(
        &fixture.router,
        "AKIDREACTOR0",
        "GetComment",
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert_eq!(
        comment.pointer("/comment/callerReactions"),
        Some(&json!(["ROCKET"])),
        "case \"a change at the bound\": expected the changed reaction, got {comment}"
    );
    assert_eq!(
        comment.pointer("/comment/reactionCounts"),
        Some(&json!({ "EYES": MAX_REACTIONS_PER_COMMENT - 1, "ROCKET": 1 })),
        "case \"a change at the bound\": expected the counts to stay at the bound, got {comment}"
    );
}
