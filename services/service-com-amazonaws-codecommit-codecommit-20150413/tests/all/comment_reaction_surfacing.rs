// SPDX-License-Identifier: CC0-1.0

//! A comment's reaction state from the write that records it to every read path
//! that carries it: `PutCommentReaction`, `GetComment`, `GetCommentReactions` and
//! `GetCommentsForComparedCommit`, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: a reaction value names a reaction of the service's
//! catalog in ANY of the forms the API publishes for it — the reaction's name,
//! its emoji short code (and the further short codes that spell the same
//! reaction), its graphic and its Unicode codepoint — and the reaction a comment
//! then holds is reported from stored state on every path that carries one: as a
//! reaction VALUE, which is the reaction's name, in `callerReactions` and in the
//! keys of `reactionCounts`, a map a comment always carries and which is empty
//! where nobody has reacted, and as the three spellings of
//! `ReactionValueFormats` in the reactions of the comment.
//!
//! The tables below span the class that rule covers — every reaction in the
//! catalog in every form a request may name it by, comments holding no reaction,
//! comments posted at a file, at a line and as a reply, several principals
//! sharing a reaction, a principal holding none, and a reaction removed again —
//! rather than one recorded request.

use serde_json::{json, Value};

use crate::comment_lifecycle::{fixture, Fixture, OTHER_ACCESS_KEY_ID, PATH};
use crate::repository_lifecycle::{ok, string_at};
use crate::wire::{caller_arn, ok_as, DEFAULT_ACCESS_KEY_ID};

/// One reaction of the service's catalog, in every form the API publishes for it.
///
/// This table is the crate's ONE test-side reaction catalog: the suites that read
/// a comment's reactions ([`crate::comment_reactions_read`]) and that drive a
/// comment's whole lifecycle ([`crate::comment_lifecycle`]) project their own rows
/// out of it, so a change to the catalog is made in one place.
///
/// The catalog is the emoji reaction table in the CodeCommit User Guide, which
/// the model's `reactionValue` documentation links to
/// (`smithy_query(query="doc",shape="PutCommentReactionInput",member="reactionValue")`);
/// the three reported spellings are the members of `ReactionValueFormats`
/// (`smithy_query(query="shape",shape="ReactionValueFormats")`) and the name is
/// the reaction VALUE a comment carries its reactions as
/// (`smithy_query(query="shape",shape="ReactionCountsMap,CallerReactions")`).
pub(crate) struct CatalogRow {
    pub(crate) name: &'static str,
    pub(crate) short_code: &'static str,
    pub(crate) emoji: &'static str,
    pub(crate) unicode: &'static str,
    /// Further short codes, and emoji spellings, that name the same reaction.
    pub(crate) aliases: &'static [&'static str],
}

pub(crate) const CATALOG: &[CatalogRow] = &[
    CatalogRow {
        name: "THUMBSUP",
        short_code: ":thumbsup:",
        emoji: "\u{1f44d}",
        unicode: "U+1F44D",
        aliases: &[":+1:"],
    },
    CatalogRow {
        name: "THUMBSDOWN",
        short_code: ":thumbsdown:",
        emoji: "\u{1f44e}",
        unicode: "U+1F44E",
        aliases: &[":-1:"],
    },
    CatalogRow {
        name: "SMILE",
        short_code: ":smile:",
        emoji: "\u{1f604}",
        unicode: "U+1F604",
        aliases: &[],
    },
    CatalogRow {
        name: "CONFUSED",
        short_code: ":confused:",
        emoji: "\u{1f615}",
        unicode: "U+1F615",
        aliases: &[],
    },
    CatalogRow {
        name: "EYES",
        short_code: ":eyes:",
        emoji: "\u{1f440}",
        unicode: "U+1F440",
        aliases: &[],
    },
    CatalogRow {
        name: "HEART",
        short_code: ":heart:",
        emoji: "\u{2764}",
        unicode: "U+2764",
        // The graphic as a keyboard sends it, with the variation selector.
        aliases: &["\u{2764}\u{fe0f}"],
    },
    CatalogRow {
        name: "HOORAY",
        short_code: ":hooray:",
        emoji: "\u{1f389}",
        unicode: "U+1F389",
        aliases: &[":tada:"],
    },
    CatalogRow {
        name: "ROCKET",
        short_code: ":rocket:",
        emoji: "\u{1f680}",
        unicode: "U+1F680",
        aliases: &[],
    },
];

/// Every form a request may name a reaction by, paired with the catalog row it
/// names: the name as published and in lower case, the short code, the graphic,
/// the codepoint, and every further spelling of the same reaction.
pub(crate) fn sent_forms() -> Vec<(String, &'static CatalogRow)> {
    CATALOG
        .iter()
        .flat_map(|row| {
            let mut forms = vec![
                row.name.to_string(),
                row.name.to_lowercase(),
                row.short_code.to_string(),
                row.emoji.to_string(),
                row.unicode.to_string(),
            ];
            forms.extend(row.aliases.iter().map(|alias| (*alias).to_string()));
            forms.into_iter().map(move |sent| (sent, row))
        })
        .collect()
}

/// A `reactionCounts` map, as the wire carries it: one entry per reaction held,
/// keyed by the reaction's name.
fn counts(held: &[(&str, u64)]) -> Value {
    Value::Object(
        held.iter()
            .map(|(name, count)| ((*name).to_string(), json!(count)))
            .collect(),
    )
}

/// A comment on the fixture's comparison, posted where `location` says.
async fn comment_on(fixture: &Fixture, content: &str, location: Option<Value>) -> String {
    let mut body = json!({
        "repositoryName": fixture.repository,
        "afterCommitId": fixture.after_commit,
        "beforeCommitId": fixture.before_commit,
        "content": content,
    });
    if let Some(location) = location {
        body["location"] = location;
    }
    let posted = ok(&fixture.router, "PostCommentForComparedCommit", &body).await;
    assert_eq!(
        posted.pointer("/comment/reactionCounts"),
        Some(&json!({})),
        "case {content:?}: a comment is posted holding no reaction, got {posted}"
    );
    string_at(&posted, "/comment/commentId")
}

/// The comment `comment_id` names, as the bulk comparison read reports it to
/// `access_key_id` — the path a review UI reads reaction state from.
async fn comment_in_comparison(fixture: &Fixture, access_key_id: &str, comment_id: &str) -> Value {
    let read = ok_as(
        &fixture.router,
        access_key_id,
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
        .unwrap_or_else(|| panic!("no commentsForComparedCommitData in {read}"))
        .iter()
        .filter_map(|entry| entry.get("comments").and_then(Value::as_array))
        .flatten()
        .find(|comment| comment.get("commentId") == Some(&json!(comment_id)))
        .unwrap_or_else(|| panic!("the comparison read does not report {comment_id}: {read}"))
        .clone()
}

/// The comment `comment_id` names, as `GetComment` reports it to `access_key_id`.
async fn comment_read(fixture: &Fixture, access_key_id: &str, comment_id: &str) -> Value {
    let read = ok_as(
        &fixture.router,
        access_key_id,
        "GetComment",
        &json!({ "commentId": comment_id }),
    )
    .await;
    read.get("comment")
        .unwrap_or_else(|| panic!("no comment in {read}"))
        .clone()
}

/// The reactions `GetCommentReactions` reports for `comment_id`.
async fn reactions_of(fixture: &Fixture, comment_id: &str) -> Vec<Value> {
    let read = ok(
        &fixture.router,
        "GetCommentReactions",
        &json!({ "commentId": comment_id }),
    )
    .await;
    read.get("reactionsForComment")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no reactionsForComment in {read}"))
        .clone()
}

#[tokio::test]
async fn every_published_form_of_a_reaction_is_accepted_and_read_back_on_every_path() {
    let fixture = fixture("comment-reaction-surfacing-forms").await;
    let comment_id = comment_on(&fixture, "react to me in every form", None).await;
    let caller = caller_arn(DEFAULT_ACCESS_KEY_ID);

    for (sent, row) in sent_forms() {
        // The write is accepted, whichever form names the reaction …
        ok(
            &fixture.router,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": sent }),
        )
        .await;

        // … and one reaction per caller means the caller now holds exactly that
        // one, reported as a reaction value by its name on the per-comment read,
        let expected_counts = counts(&[(row.name, 1)]);
        let comment = comment_read(&fixture, DEFAULT_ACCESS_KEY_ID, &comment_id).await;
        assert_eq!(
            comment.get("callerReactions"),
            Some(&json!([row.name])),
            "case {sent:?}: expected the caller's one reaction, got {comment}"
        );
        assert_eq!(
            comment.get("reactionCounts"),
            Some(&expected_counts),
            "case {sent:?}: expected {expected_counts}, got {comment}"
        );

        // on the bulk comparison read,
        let embedded = comment_in_comparison(&fixture, DEFAULT_ACCESS_KEY_ID, &comment_id).await;
        assert_eq!(
            embedded.get("callerReactions"),
            Some(&json!([row.name])),
            "case {sent:?}: expected the caller's reaction in the comparison read, got {embedded}"
        );
        assert_eq!(
            embedded.get("reactionCounts"),
            Some(&expected_counts),
            "case {sent:?}: expected {expected_counts} in the comparison read, got {embedded}"
        );

        // and in its three published spellings, with the principal that left it,
        // on the enumeration of who reacted.
        let entries = reactions_of(&fixture, &comment_id).await;
        assert_eq!(
            entries,
            vec![json!({
                "reaction": {
                    "emoji": row.emoji,
                    "shortCode": row.short_code,
                    "unicode": row.unicode,
                },
                "reactionUsers": [caller],
                "reactionsFromDeletedUsersCount": 0,
            })],
            "case {sent:?}: expected the one reaction held and who left it, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn a_comment_nobody_reacted_to_reports_an_empty_reaction_count_map() {
    let fixture = fixture("comment-reaction-surfacing-empty").await;

    // Rows differ along where the comment sits, which is what a comment's own
    // members generalise over: nowhere in particular, at a file, at a line of the
    // after version, and as a reply inside another comment's thread.
    let root = comment_on(&fixture, "nowhere in particular", None).await;
    let at_file = comment_on(
        &fixture,
        "at the file",
        Some(json!({ "filePath": PATH })),
    )
    .await;
    let at_line = comment_on(
        &fixture,
        "at a line",
        Some(json!({
            "filePath": PATH,
            "filePosition": 1,
            "relativeFileVersion": "AFTER",
        })),
    )
    .await;
    let replied = ok(
        &fixture.router,
        "PostCommentReply",
        &json!({ "inReplyTo": root, "content": "a reply nobody reacted to" }),
    )
    .await;
    let reply = string_at(&replied, "/comment/commentId");
    assert_eq!(
        replied.pointer("/comment/reactionCounts"),
        Some(&json!({})),
        "case \"a reply\": a comment is posted holding no reaction, got {replied}"
    );

    for (label, comment_id) in [
        ("nowhere in particular", &root),
        ("at the file", &at_file),
        ("at a line", &at_line),
        ("a reply", &reply),
    ] {
        // Two callers: a comment nobody has reacted to holds nothing for either,
        // and the count map is EMPTY rather than absent on both read paths.
        for access_key_id in [DEFAULT_ACCESS_KEY_ID, OTHER_ACCESS_KEY_ID] {
            for (path, comment) in [
                ("GetComment", comment_read(&fixture, access_key_id, comment_id).await),
                (
                    "GetCommentsForComparedCommit",
                    comment_in_comparison(&fixture, access_key_id, comment_id).await,
                ),
            ] {
                assert_eq!(
                    comment.get("reactionCounts"),
                    Some(&json!({})),
                    "case {label:?} on {path} as {access_key_id}: expected an empty count map, \
                     got {comment}"
                );
                assert_eq!(
                    comment.get("callerReactions"),
                    None,
                    "case {label:?} on {path} as {access_key_id}: expected no reaction of the \
                     caller's own, got {comment}"
                );
            }
        }
        assert!(
            reactions_of(&fixture, comment_id).await.is_empty(),
            "case {label:?}: expected no reaction entries at all"
        );
    }
}

#[tokio::test]
async fn reaction_counts_count_principals_while_caller_reactions_name_the_asking_one() {
    let fixture = fixture("comment-reaction-surfacing-principals").await;
    let comment_id = comment_on(&fixture, "reacted to by a crowd", None).await;

    // Three principals: two sharing one reaction, each sending it in a different
    // published form, and a third leaving another. A fourth holds none.
    let third = "AKIDTHIRDREACTOR";
    let bystander = "AKIDNOREACTION";
    fixture.hold(third);
    fixture.hold(bystander);
    for (access_key_id, sent) in [
        (DEFAULT_ACCESS_KEY_ID, "HEART"),
        (OTHER_ACCESS_KEY_ID, "\u{2764}\u{fe0f}"),
        (third, ":rocket:"),
    ] {
        ok_as(
            &fixture.router,
            access_key_id,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": sent }),
        )
        .await;
    }

    let shared = counts(&[("HEART", 2), ("ROCKET", 1)]);
    for (access_key_id, own) in [
        (DEFAULT_ACCESS_KEY_ID, Some("HEART")),
        (OTHER_ACCESS_KEY_ID, Some("HEART")),
        (third, Some("ROCKET")),
        (bystander, None),
    ] {
        for (path, comment) in [
            ("GetComment", comment_read(&fixture, access_key_id, &comment_id).await),
            (
                "GetCommentsForComparedCommit",
                comment_in_comparison(&fixture, access_key_id, &comment_id).await,
            ),
        ] {
            assert_eq!(
                comment.get("reactionCounts"),
                Some(&shared),
                "case {access_key_id} on {path}: expected {shared} counted over principals, \
                 got {comment}"
            );
            assert_eq!(
                comment.get("callerReactions"),
                own.map(|own| json!([own])).as_ref(),
                "case {access_key_id} on {path}: expected {own:?} as the caller's own, \
                 got {comment}"
            );
        }
    }

    // Removing one principal's reaction leaves the one it shared standing for the
    // other, and drops the reaction nobody holds any more.
    for (removal, access_key_id, expected) in [
        ("", DEFAULT_ACCESS_KEY_ID, counts(&[("HEART", 1), ("ROCKET", 1)])),
        ("none", third, counts(&[("HEART", 1)])),
    ] {
        ok_as(
            &fixture.router,
            access_key_id,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": removal }),
        )
        .await;
        let comment = comment_in_comparison(&fixture, access_key_id, &comment_id).await;
        assert_eq!(
            comment.get("reactionCounts"),
            Some(&expected),
            "case {removal:?} by {access_key_id}: expected {expected}, got {comment}"
        );
        assert_eq!(
            comment.get("callerReactions"),
            None,
            "case {removal:?} by {access_key_id}: expected the caller to hold none, got {comment}"
        );
    }
}
