// SPDX-License-Identifier: CC0-1.0

//! A tag key's SPELLING is not special on any path a tag travels — not on the
//! write, not on the read, and not on the removal.
//!
//! `TagMap` is `map<TagKey, TagValue>` and the model constrains neither member
//! (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue")` reports no
//! `@length` and no `@pattern` on either), so the service stores every key a
//! caller supplies VERBATIM and removes exactly the keys `UntagResource` names:
//! no key is filtered, rewritten, dropped or removed along with another because
//! of how it is spelled — including a key carrying the `aws:` prefix AWS
//! reserves elsewhere, an upper-case variant of it, colons, an ARN, punctuation,
//! a space or non-ASCII characters. The only bound the service enforces on a key
//! is the documented tag-key length, which the longest row sits exactly on.
//!
//! [`crate::tag_key_round_trip`] pins the WRITE and READ halves of that rule
//! (`CreateSavingsPlan`/`TagResource` into `ListTagsForResource`/
//! `DescribeSavingsPlans`). This suite pins the half that outlives a write: a
//! selective `UntagResource` leaves every key it does not name — so a plan whose
//! ordinary key is untagged still reports its reserved-prefix key — removing a
//! key the plan does not carry changes nothing, and the oddly spelled key is
//! itself removable.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// The key written beside each case's key, and the one every case removes first.
/// Plainly spelled, so what survives that removal is the case's own key.
const SIBLING: &str = "Owner";

/// A key no case writes, so removing it is a removal of nothing.
const ABSENT: &str = "NeverWritten";

/// The tag map a reader reported — `{}` when the reader reported no tags at all.
fn tags(body: &Value) -> serde_json::Map<String, Value> {
    body.get("tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn removing_one_tag_key_leaves_every_other_spelling_intact() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, key) — the key set a caller may use. `MAX_TAG_KEY_CHARS` is the
    // only bound the service enforces, so the longest row sits exactly on it.
    let longest = "k".repeat(state::MAX_TAG_KEY_CHARS);
    let cases: [(&str, &str); 9] = [
        ("a plain key", "env"),
        ("the prefix AWS reserves elsewhere", "aws:tag:domain"),
        ("the bare reserved prefix", "aws:"),
        ("the reserved prefix in upper case", "AWS:Owner"),
        ("an ARN as a key", "arn:aws:iam::000000000000:role/App"),
        ("punctuation", "dots.and-dashes_1/slash@at"),
        ("a space", "cost centre"),
        ("non-ASCII", "équipe:coût"),
        ("the longest key the service accepts", longest.as_str()),
    ];

    for (case, key) in cases {
        let plan_id = emulator
            .purchase(json!({
                "savingsPlanOfferingId": offering_id,
                "commitment": "0.1",
            }))
            .await;
        let arn = text(&emulator.plan(&plan_id).await, "savingsPlanArn");

        // Both keys are written in one call, so the two differ only in spelling.
        let written = emulator
            .call(
                "TagResource",
                json!({ "resourceArn": arn, "tags": { key: "kept", SIBLING: "sibling" } }),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?} ({key:?}): TagResource must accept the pair, got {} {:?}",
            written.status, written.body
        );
        let listed = emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await;
        assert_eq!(
            tags(&listed.body),
            tags(&json!({ "tags": { key: "kept", SIBLING: "sibling" } })),
            "case {case:?} ({key:?}): both keys written must come back verbatim, got {:?}",
            listed.body
        );

        // Removing a key the plan does not carry removes nothing.
        let noop = emulator
            .call(
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": [ABSENT] }),
            )
            .await;
        assert_eq!(
            noop.status, 200,
            "case {case:?} ({key:?}): untagging an absent key is accepted, got {} {:?}",
            noop.status, noop.body
        );
        let listed = emulator
            .call("ListTagsForResource", json!({ "resourceArn": arn }))
            .await;
        assert_eq!(
            tags(&listed.body).len(),
            2,
            "case {case:?} ({key:?}): untagging {ABSENT:?} must leave both keys, got {:?}",
            listed.body
        );

        // The selective removal: only the plainly spelled sibling is named, so
        // the case's own key — however it is spelled — must survive it on every
        // reader.
        let removed = emulator
            .call(
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": [SIBLING] }),
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?} ({key:?}): UntagResource must accept the removal, got {} {:?}",
            removed.status, removed.body
        );
        for (reader, body) in [
            (
                "ListTagsForResource",
                emulator
                    .call("ListTagsForResource", json!({ "resourceArn": arn }))
                    .await
                    .body,
            ),
            ("DescribeSavingsPlans", emulator.plan(&plan_id).await),
        ] {
            assert_eq!(
                tags(&body).get(key).and_then(Value::as_str),
                Some("kept"),
                "case {case:?} ({key:?}): removing {SIBLING:?} must leave it; {reader} reported \
                 {:?}",
                tags(&body)
            );
            assert_eq!(
                tags(&body).len(),
                1,
                "case {case:?} ({key:?}): exactly the unremoved key must remain; {reader} \
                 reported {:?}",
                tags(&body)
            );
        }

        // And the oddly spelled key is itself removable, by the same spelling.
        let removed = emulator
            .call(
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": [key] }),
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?} ({key:?}): UntagResource must accept its own removal, got {} {:?}",
            removed.status, removed.body
        );
        for (reader, body) in [
            (
                "ListTagsForResource",
                emulator
                    .call("ListTagsForResource", json!({ "resourceArn": arn }))
                    .await
                    .body,
            ),
            ("DescribeSavingsPlans", emulator.plan(&plan_id).await),
        ] {
            assert!(
                tags(&body).is_empty(),
                "case {case:?} ({key:?}): the plan must carry no tags now; {reader} reported {:?}",
                tags(&body)
            );
        }
    }
}
