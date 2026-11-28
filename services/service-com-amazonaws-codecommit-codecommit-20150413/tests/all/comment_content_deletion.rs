// SPDX-License-Identifier: CC0-1.0

//! Deleting the content of a comment: `DeleteCommentContent` end to end, over the
//! wire, against the comments the sibling harness posts on a comparison between
//! two commits.
//!
//! The rule under test: `DeleteCommentContent` clears the CONTENT of the comment
//! its id names and leaves the record itself in place — so the answer, and every
//! later read of that comment, reports it with `deleted` true, no `content`, and
//! the id, author, creation date and reactions it already had; a second call
//! against the same comment, and any attempt to read, edit or react to it, is
//! refused with `CommentDeletedException` and the sentence that exception is
//! recorded with, which carries no value from the request. Other comments are
//! untouched.
//!
//! The table below spans the class that rule covers — content kind and size
//! (ASCII, unicode, multi-line, one character, the documented 10,240-character
//! maximum), whether the comment sits at a line of a file, at a file, or nowhere,
//! whether the comparison has a before side, and whether another principal had
//! reacted to the comment — rather than the one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::comment_deleted_refusals::comment_deleted_message;
use crate::comment_lifecycle::{fixture, Fixture, MAX_COMMENT_CHARACTERS, OTHER_ACCESS_KEY_ID, PATH};
use crate::repository_lifecycle::{ok, string_at};
use crate::wire::ok_as;

/// The code AND message of a refused call: every error this suite asserts is a
/// modelled 400, and the parse is the crate's single one
/// ([`crate::harness::error_with_message`], over `crate::wire::refusal_as`).
async fn refusal(router: &MultiServiceRouter, operation: &str, body: &Value) -> (String, String) {
    crate::harness::error_with_message(router, operation, body, 400).await
}

/// One row of the deletion table.
struct DeleteCase {
    label: &'static str,
    content: String,
    /// Whether the comment is posted on a comparison that has a before side.
    with_before: bool,
    /// Where in the comparison the comment sits, as sent.
    location: Option<Value>,
    /// Whether a second principal reacts to the comment before its content goes:
    /// the reactions on a comment outlive its content.
    reacted: bool,
}

/// The rows differ along what the rule generalises over: the content that is
/// cleared, where the comment sits, which comparison it was filed under, and
/// whether anything else hangs off the comment.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "a short ASCII comment, nowhere in particular",
            content: "Looks good to me.".to_string(),
            with_before: true,
            location: None,
            reacted: false,
        },
        DeleteCase {
            label: "a unicode comment another caller reacted to",
            content: "✅ se ve bien — ¿lo fusionamos?".to_string(),
            with_before: true,
            location: None,
            reacted: true,
        },
        DeleteCase {
            label: "a multi-line comment on the first line of the after version",
            content: "multi\nline\ncomment".to_string(),
            with_before: true,
            location: Some(json!({
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "AFTER",
            })),
            reacted: true,
        },
        DeleteCase {
            label: "a one-character comment on a comparison with no before side",
            content: "?".to_string(),
            with_before: false,
            location: None,
            reacted: false,
        },
        DeleteCase {
            label: "the longest comment the service accepts, on a file",
            content: "x".repeat(MAX_COMMENT_CHARACTERS),
            with_before: true,
            location: Some(json!({ "filePath": PATH })),
            reacted: false,
        },
    ]
}

/// Post one row's comment and report the id it was minted under.
async fn post(fixture: &Fixture, case: &DeleteCase) -> String {
    let mut input = json!({
        "repositoryName": fixture.repository,
        "afterCommitId": fixture.after_commit,
        "content": case.content,
    });
    if case.with_before {
        input["beforeCommitId"] = json!(fixture.before_commit);
    }
    if let Some(location) = &case.location {
        input["location"] = location.clone();
    }
    let posted = ok(&fixture.router, "PostCommentForComparedCommit", &input).await;
    let comment_id = string_at(&posted, "/comment/commentId");
    if case.reacted {
        ok_as(
            &fixture.router,
            OTHER_ACCESS_KEY_ID,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": ":thumbsup:" }),
        )
        .await;
    }
    comment_id
}

/// The stored comment `comment_id` names, as `GetCommentsForComparedCommit`
/// reports it: the reader that still answers for a comment whose content is gone.
async fn read_back(fixture: &Fixture, case: &DeleteCase, comment_id: &str) -> Value {
    let mut input = json!({
        "repositoryName": fixture.repository,
        "afterCommitId": fixture.after_commit,
    });
    if case.with_before {
        input["beforeCommitId"] = json!(fixture.before_commit);
    }
    let read = ok(&fixture.router, "GetCommentsForComparedCommit", &input).await;
    read.get("commentsForComparedCommitData")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {:?}: no comparison data in {read}", case.label))
        .iter()
        .filter_map(|entry| entry.get("comments").and_then(Value::as_array))
        .flatten()
        .find(|comment| comment.get("commentId") == Some(&json!(comment_id)))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the comparison no longer reports the comment {comment_id}: {read}",
                case.label
            )
        })
}

fn epoch_seconds(comment: &Value, member: &str, label: &str) -> f64 {
    comment
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {label:?}: no {member} in {comment}"))
}

#[tokio::test]
async fn deleting_a_comments_content_keeps_the_comment_and_refuses_a_second_deletion() {
    let fixture = fixture("comment-content-deletion-repo").await;
    let cases = delete_cases();

    // Every row's comment is posted first, so each deletion is checked against a
    // repository that holds the others too.
    let mut ids: Vec<String> = Vec::new();
    for case in &cases {
        ids.push(post(&fixture, case).await);
    }

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let comment_id = &ids[index];

        // Before the deletion the content is what was posted.
        let before = ok(
            &fixture.router,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            before.pointer("/comment/content"),
            Some(&json!(case.content)),
            "case {label:?}: expected the posted content before the deletion, got {before}"
        );
        let created = epoch_seconds(
            before
                .pointer("/comment")
                .unwrap_or_else(|| panic!("case {label:?}: no comment in {before}")),
            "creationDate",
            label,
        );
        let reaction_counts = before.pointer("/comment/reactionCounts").cloned();
        assert_eq!(
            reaction_counts
                .as_ref()
                .and_then(Value::as_object)
                .is_some_and(|counts| !counts.is_empty()),
            case.reacted,
            "case {label:?}: expected counted reactions only where one was left, got {before}"
        );

        // The deletion answers with the stored comment as it now stands.
        let deleted = ok(
            &fixture.router,
            "DeleteCommentContent",
            &json!({ "commentId": comment_id }),
        )
        .await;
        let comment = deleted
            .pointer("/comment")
            .unwrap_or_else(|| panic!("case {label:?}: no comment in {deleted}"))
            .clone();
        assert_eq!(
            comment.get("deleted"),
            Some(&json!(true)),
            "case {label:?}: expected deleted true, got {comment}"
        );
        assert!(
            comment.get("content").is_none(),
            "case {label:?}: expected the content gone, got {comment}"
        );
        assert_eq!(
            comment.get("commentId"),
            Some(&json!(comment_id)),
            "case {label:?}: expected the comment to keep its id, got {comment}"
        );
        assert_eq!(
            comment.get("authorArn"),
            before.pointer("/comment/authorArn"),
            "case {label:?}: expected the author to stand, got {comment}"
        );
        assert_eq!(
            epoch_seconds(&comment, "creationDate", label),
            created,
            "case {label:?}: expected the creation date to stand, got {comment}"
        );
        assert!(
            epoch_seconds(&comment, "lastModifiedDate", label) >= created,
            "case {label:?}: expected the deletion to be dated at or after the post, got {comment}"
        );
        assert_eq!(
            comment.get("reactionCounts").cloned(),
            reaction_counts,
            "case {label:?}: expected the reactions to outlive the content, got {comment}"
        );

        // That answer is the stored record, not a one-off: the reader that still
        // answers for a deleted comment reports the very same comment.
        let read = read_back(&fixture, case, comment_id).await;
        assert_eq!(
            read, comment,
            "case {label:?}: expected a later read to report the deleted comment unchanged"
        );

        // "You cannot edit or delete a deleted comment": the second deletion is
        // refused rather than repeated, and so is every other call about it. The
        // message is the sentence the exception is recorded with — see
        // [`comment_deleted_message`], the one place this suite and its sibling
        // state it.
        for (operation, input) in [
            ("DeleteCommentContent", json!({ "commentId": comment_id })),
            ("GetComment", json!({ "commentId": comment_id })),
            (
                "UpdateComment",
                json!({ "commentId": comment_id, "content": "after the deletion" }),
            ),
            (
                "PutCommentReaction",
                json!({ "commentId": comment_id, "reactionValue": ":heart:" }),
            ),
        ] {
            let (code, message) = refusal(&fixture.router, operation, &input).await;
            assert_eq!(
                code, "CommentDeletedException",
                "case {label:?}: {operation} on a deleted comment: expected \
                 CommentDeletedException, got {code:?}"
            );
            assert_eq!(
                message,
                comment_deleted_message(operation),
                "case {label:?}: {operation} on a deleted comment: unexpected message {message:?}"
            );
            assert!(
                !message.contains(comment_id.as_str()),
                "case {label:?}: {operation}: expected a message carrying no request value, got \
                 {message:?}"
            );
        }

        // The refusals changed nothing: the record is still what the deletion left.
        let after_refusals = read_back(&fixture, case, comment_id).await;
        assert_eq!(
            after_refusals, comment,
            "case {label:?}: expected the refused calls to leave the comment alone"
        );

        // Only the named comment lost its content: the rows not yet deleted still
        // report theirs, and the ones already deleted stay deleted.
        for (other_index, (other, other_id)) in cases.iter().zip(&ids).enumerate() {
            if other_index == index {
                continue;
            }
            let other_read = read_back(&fixture, other, other_id).await;
            let already_deleted = other_index < index;
            assert_eq!(
                other_read.get("content").cloned(),
                (!already_deleted).then(|| json!(other.content)),
                "case {label:?}: expected {:?} to keep its own content, got {other_read}",
                other.label
            );
        }
    }
}
