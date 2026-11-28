// SPDX-License-Identifier: CC0-1.0

//! What a call against a comment whose content has been deleted is refused WITH:
//! the code AND the text of `CommentDeletedException`, over the wire, for every
//! operation that takes a `commentId`.
//!
//! The rule under test: a modelled exception carries the message the service is
//! recorded to produce for the operation that raised it — so
//! `CommentDeletedException` carries the fixed sentence pair "The specified comment
//! has already been marked as deleted." and "Verify that you specified a valid
//! comment ID, and then try again.", separated by the TWO spaces the recording
//! carries, led by "You cannot delete this comment." for the calls that ask to
//! edit or delete the dead comment itself (a repeated `DeleteCommentContent`, an
//! `UpdateComment`), and never carrying a value taken from the request. The
//! deleted record is what decides this, so the
//! refusal does not depend on which comment it is, who is asking, or whether the
//! caller is the author; and being refused changes nothing about the stored
//! comment.
//!
//! The table below spans what the rule generalises over — the comment (content
//! kind and size, where it sits in the comparison, whether another principal had
//! reacted to it, which principal posted it), the operation that is refused, and
//! the principal that calls it — rather than the one recorded request.

use serde_json::{json, Value};

use crate::comment_lifecycle::{fixture, Fixture, MAX_COMMENT_CHARACTERS, OTHER_ACCESS_KEY_ID, PATH};
use crate::repository_lifecycle::string_at;
use crate::wire::{ok_as, refusal_as, DEFAULT_ACCESS_KEY_ID};

/// A third principal, so a row can carry a reaction left by someone who is
/// neither the author nor the caller that is refused.
const REACTOR_ACCESS_KEY_ID: &str = "AKIDREACTINGUSER";

/// The message `CommentDeletedException` carries, by the OPERATION that refused
/// the call. The sentence pair alone is the recorded answer where the call only
/// asks about the comment whose content has been deleted (`GetComment`) or about
/// something hanging off it (`PutCommentReaction`, for which no lead clause is
/// recorded); the calls that ask to edit or delete the dead comment itself — a
/// repeated `DeleteCommentContent` and an `UpdateComment` — are recorded leading
/// it with "You cannot delete this comment.". The sentences are separated by TWO
/// spaces, which is part of the recorded constant — hence the explicit `concat!`,
/// whose line continuation cannot swallow them.
pub(crate) fn comment_deleted_message(operation: &str) -> String {
    let already_deleted = concat!(
        "The specified comment has already been marked as deleted.",
        "  ",
        "Verify that you specified a valid comment ID, and then try again.",
    );
    match operation {
        "DeleteCommentContent" | "UpdateComment" => {
            format!("You cannot delete this comment.  {already_deleted}")
        }
        _ => already_deleted.to_string(),
    }
}

/// One row of the table: a comment, as it is posted and reacted to before its
/// content goes.
struct Case {
    label: &'static str,
    content: String,
    /// Where in the comparison the comment sits, as sent.
    location: Option<Value>,
    /// The principal that posts it, and so its author.
    author: &'static str,
    /// Whether a third principal reacts to it before the deletion.
    reacted: bool,
}

/// The rows differ along the comment the refusal is about: its content kind and
/// size, where it sits, who wrote it, and whether anything else hangs off it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a short ASCII comment, nowhere in particular",
            content: "Looks good to me.".to_string(),
            location: None,
            author: DEFAULT_ACCESS_KEY_ID,
            reacted: false,
        },
        Case {
            label: "a unicode comment another principal reacted to",
            content: "✅ se ve bien — ¿lo fusionamos?".to_string(),
            location: None,
            author: DEFAULT_ACCESS_KEY_ID,
            reacted: true,
        },
        Case {
            label: "a multi-line comment on the first line of the after version",
            content: "multi\nline\ncomment".to_string(),
            location: Some(json!({
                "filePath": PATH,
                "filePosition": 1,
                "relativeFileVersion": "AFTER",
            })),
            author: OTHER_ACCESS_KEY_ID,
            reacted: true,
        },
        Case {
            label: "a comment on a file as a whole, written by the other caller",
            content: "about the file as a whole".to_string(),
            location: Some(json!({ "filePath": PATH })),
            author: OTHER_ACCESS_KEY_ID,
            reacted: false,
        },
        Case {
            label: "the longest comment the service accepts",
            content: "x".repeat(MAX_COMMENT_CHARACTERS),
            location: None,
            author: DEFAULT_ACCESS_KEY_ID,
            reacted: false,
        },
    ]
}

/// Every operation that names a comment by id, with the other members it requires
/// filled in, so the deleted comment is the only thing wrong with the request.
fn operations() -> Vec<(&'static str, Value)> {
    vec![
        ("GetComment", json!({})),
        ("DeleteCommentContent", json!({})),
        ("UpdateComment", json!({ "content": "after the deletion" })),
        (
            "PutCommentReaction",
            json!({ "reactionValue": ":thumbsup:" }),
        ),
    ]
}

/// Post one row's comment, have the third principal react to it when the row says
/// so, and report the id it was minted under.
async fn post(fixture: &Fixture, case: &Case) -> String {
    let mut input = json!({
        "repositoryName": fixture.repository,
        "afterCommitId": fixture.after_commit,
        "beforeCommitId": fixture.before_commit,
        "content": case.content,
    });
    if let Some(location) = &case.location {
        input["location"] = location.clone();
    }
    let posted = ok_as(
        &fixture.router,
        case.author,
        "PostCommentForComparedCommit",
        &input,
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");
    if case.reacted {
        ok_as(
            &fixture.router,
            REACTOR_ACCESS_KEY_ID,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": ":eyes:" }),
        )
        .await;
    }
    comment_id
}

/// The stored comment `comment_id` names, as `GetCommentsForComparedCommit`
/// reports it to `caller`: the reader that still answers for a comment whose
/// content is gone.
async fn read_back(fixture: &Fixture, caller: &str, label: &str, comment_id: &str) -> Value {
    let read = ok_as(
        &fixture.router,
        caller,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "beforeCommitId": fixture.before_commit,
        }),
    )
    .await;
    read.get("commentsForComparedCommitData")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no comparison data in {read}"))
        .iter()
        .filter_map(|entry| entry.get("comments").and_then(Value::as_array))
        .flatten()
        .find(|comment| comment.get("commentId") == Some(&json!(comment_id)))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {label:?}: the comparison no longer reports {comment_id}: {read}")
        })
}

#[tokio::test]
async fn a_deleted_comment_is_refused_with_the_exceptions_recorded_sentence() {
    let fixture = fixture("comment-deleted-refusal-repo").await;

    for case in cases() {
        let label = case.label;
        let comment_id = post(&fixture, &case).await;

        // The content goes, and the comment stays: this is the state every
        // refusal below is decided from.
        let deleted = ok_as(
            &fixture.router,
            case.author,
            "DeleteCommentContent",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            deleted.pointer("/comment/deleted"),
            Some(&json!(true)),
            "case {label:?}: expected the comment to report itself deleted, got {deleted}"
        );
        let stored = read_back(&fixture, case.author, label, &comment_id).await;

        // Whoever asks, and whichever operation names the comment, the refusal is
        // the same modelled 400 with the same sentence — including for the
        // author, whose authorship does not buy a different answer.
        for caller in [DEFAULT_ACCESS_KEY_ID, OTHER_ACCESS_KEY_ID] {
            for (operation, extra) in operations() {
                let mut input = json!({ "commentId": comment_id });
                for (member, value) in extra.as_object().expect("each extra is an object") {
                    input[member.as_str()] = value.clone();
                }
                let (code, message) =
                    refusal_as(&fixture.router, caller, operation, &input, 400).await;
                assert_eq!(
                    code, "CommentDeletedException",
                    "case {label:?}: {operation} as {caller}: expected CommentDeletedException, \
                     got {code:?}"
                );
                assert_eq!(
                    message,
                    comment_deleted_message(operation),
                    "case {label:?}: {operation} as {caller}: unexpected message {message:?}"
                );
                for from_request in [
                    comment_id.as_str(),
                    case.content.as_str(),
                    fixture.repository.as_str(),
                    PATH,
                    caller,
                ] {
                    assert!(
                        !message.contains(from_request),
                        "case {label:?}: {operation} as {caller}: expected a message carrying no \
                         request value, got {message:?} carrying {from_request:?}"
                    );
                }
            }
        }

        // The refused calls reported a failure and wrote nothing: the record is
        // still the one the deletion left, for either caller.
        for caller in [DEFAULT_ACCESS_KEY_ID, OTHER_ACCESS_KEY_ID] {
            let after = read_back(&fixture, caller, label, &comment_id).await;
            assert_eq!(
                after.get("content"),
                None,
                "case {label:?}: expected the deleted comment to stay without content, got {after}"
            );
            if caller == case.author {
                assert_eq!(
                    after, stored,
                    "case {label:?}: expected the refusals to leave the comment alone"
                );
            }
            assert_eq!(
                after.get("reactionCounts").cloned(),
                Some(if case.reacted {
                    json!({ "EYES": 1 })
                } else {
                    json!({})
                }),
                "case {label:?}: expected the stored reactions to stand, got {after}"
            );
        }
    }
}
