// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a tag set delivered ON THE WRITE: the `x-amz-tagging` header a
//! `PutObject` carries.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the object
//! helpers and the tag-set reader are the ones [`super::bucket_lifecycle`],
//! [`super::object_and_tagging_roundtrip`] and [`super::object_tagging`] already own), so
//! what is pinned is the wire behaviour — status, bound headers, payload, error code —
//! rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * "The tag-set for the object. The tag-set must be encoded as URL Query parameters.
//!   (For example, "Key1=Value1")" (`PutObjectRequest$Tagging`) — so a write that carries
//!   a tag set in that header STORES it on the object it writes and answers its modelled
//!   200 with the object's ETag, exactly as an untagged write does; `NotImplemented` is
//!   not an outcome of this operation. The header is an
//!   `application/x-www-form-urlencoded` string, so percent escapes and `+` are decoded,
//!   an empty header is an empty tag set and a pair with no `=` is a key with an empty
//!   value.
//! * The set a write stores is the same state a `PutObjectTagging` of it would leave: the
//!   two paths read back identically through `GetObjectTagging`.
//! * A write REPLACES the whole object ("You cannot use PutObject to only update a single
//!   piece of metadata for an existing object"), so the tag set of the object it replaces
//!   goes with it: a later write with no header leaves the key with no tags.
//! * The stored set is judged against the OBJECT's published limits — "Amazon S3 limits
//!   the maximum number of tags to 10 tags per object", the 128-character key and
//!   256-character value bounds, and the key-uniqueness a tag set has — so a set past one
//!   of them is refused with HTTP 400 and the write stores nothing.
//! * "The number of tags, if any, on the object" (`GetObjectOutput$TagCount`,
//!   `HeadObjectOutput$TagCount`), so a read of a tagged object reports the count of the
//!   set it holds and a read of an untagged one reports none.

use service_com_amazonaws_s3_amazons3::state::{
    MAX_TAGS_PER_OBJECT, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::bucket_tagging::{tag, tag_set_of};
use super::object_and_tagging_roundtrip::{
    bucket, expected_etag, get_object, put_object, text,
};
use super::object_tagging::{get_object_tagging, put_object_tagging};

/// The bytes every object in this suite is written with: the shape a COPY source file has.
const BODY: &[u8] = b"a,b\n1,2\n";

/// `PUT /<bucket>/<key>` with the tag set in `x-amz-tagging`, which is what an upload that
/// stages a tagged data file puts on the wire.
async fn put_object_tagged(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    tagging: &str,
) -> Answer {
    put_object(router, bucket, key, BODY, &[("x-amz-tagging", tagging)]).await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`, the other reader that reports an object's tag
/// count.
async fn head_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send_bytes(router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await
}

/// One row of the header table: the case name, the header value, and the tag set the
/// object must hold afterwards.
type HeaderRow = (&'static str, String, Vec<(String, String)>);

/// The table of `x-amz-tagging` headers every row of which must be stored as the tag set
/// it encodes.
///
/// The rows differ along what the "encoded as URL Query parameters" rule generalises over:
/// how many pairs the header carries (none, one, three and the published per-object
/// maximum), the order they arrive in, how a value is escaped (percent escapes, `+` for a
/// space, a percent-escaped non-ASCII pair, escaped reserved characters), the degenerate
/// forms the grammar allows (an empty value, a pair with no `=` at all) and how long a key
/// and a value are (both published character bounds exactly).
fn header_rows() -> Vec<HeaderRow> {
    vec![
        (
            "one tag, the shape a staging upload sends",
            "sim2real=true".to_string(),
            vec![tag("sim2real", "true")],
        ),
        (
            "an empty header carries no pairs, which is an empty tag set",
            String::new(),
            vec![],
        ),
        (
            "three pairs keep the order the header listed them in, not key order",
            "zeta=3&alpha=1&middle=2".to_string(),
            vec![tag("zeta", "3"), tag("alpha", "1"), tag("middle", "2")],
        ),
        (
            "percent escapes are decoded",
            "project=two%20words".to_string(),
            vec![tag("project", "two words")],
        ),
        (
            "a plus is the form-encoded space",
            "project=two+words".to_string(),
            vec![tag("project", "two words")],
        ),
        (
            "escaped reserved characters survive decoding",
            "expression=a%20%2B%20b%20%3D%20c%2Fd".to_string(),
            vec![tag("expression", "a + b = c/d")],
        ),
        (
            "a percent-escaped non-ASCII key and value",
            "k%C3%A9y=v%C3%A4lue".to_string(),
            vec![tag("kéy", "välue")],
        ),
        (
            "an empty value is a value",
            "emptied=".to_string(),
            vec![tag("emptied", "")],
        ),
        (
            "a pair with no = is a key with an empty value",
            "novalue".to_string(),
            vec![tag("novalue", "")],
        ),
        (
            "a key and a value exactly at the published character bounds",
            format!(
                "{}={}",
                "k".repeat(MAX_TAG_KEY_CHARS),
                "v".repeat(MAX_TAG_VALUE_CHARS)
            ),
            vec![tag(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        ),
        (
            "the per-object tag count bound",
            (0..MAX_TAGS_PER_OBJECT)
                .map(|index| format!("key-{index}=value-{index}"))
                .collect::<Vec<_>>()
                .join("&"),
            (0..MAX_TAGS_PER_OBJECT)
                .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn a_write_that_carries_its_tag_set_stores_the_object_and_the_tags() {
    let router = router();
    bucket(&router, "tagged-writes").await;

    for (index, (case, header, tags)) in header_rows().into_iter().enumerate() {
        // The key varies with the row so each case stands on its own object, under the
        // prefixed path shape a staged data file is written at.
        let key = format!("dags/staged-{index}.csv");

        let written = put_object_tagged(&router, "tagged-writes", &key, &header).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a write carrying a tag set answers the modelled 200, got {} \
             with body {:?}",
            written.status,
            text(&written)
        );
        assert_eq!(
            written.header("etag"),
            Some(expected_etag(BODY)),
            "case {case:?}: the write must report the object's entity tag, got {:?}",
            written.header("etag")
        );

        // The object is stored, bytes and all — the header decided its tags, nothing else.
        let read = get_object(&router, "tagged-writes", &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the written object must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: the stored bytes are the ones the write carried, got {:?}",
            text(&read)
        );

        // And the tag set the header encoded is the tag set the object holds.
        let tagging = get_object_tagging(&router, "tagged-writes", &key, "").await;
        assert_eq!(
            tagging.status, 200,
            "case {case:?}: GetObjectTagging on the object just written answers 200, got \
             {} with body {:?}",
            tagging.status,
            text(&tagging)
        );
        assert_eq!(
            tag_set_of(&tagging),
            tags,
            "case {case:?}: the stored tag set must be the one the header encoded, got {:?}",
            tag_set_of(&tagging)
        );

        // "The number of tags, if any, on the object": the count of that set, and nothing
        // at all when the set is empty.
        let expected_count = (!tags.is_empty()).then(|| tags.len().to_string());
        assert_eq!(
            read.header("x-amz-tagging-count"),
            expected_count,
            "case {case:?}: expected GetObject to report {expected_count:?} tags, got {:?}",
            read.header("x-amz-tagging-count")
        );
        let headed = head_object(&router, "tagged-writes", &key).await;
        assert_eq!(
            headed.header("x-amz-tagging-count"),
            expected_count,
            "case {case:?}: expected HeadObject to report {expected_count:?} tags, got {:?}",
            headed.header("x-amz-tagging-count")
        );
    }
}

#[tokio::test]
async fn a_header_tag_set_is_the_same_state_a_tagging_call_leaves() {
    let router = router();
    bucket(&router, "tagging-paths-agree").await;

    for (index, (case, header, tags)) in header_rows().into_iter().enumerate() {
        // The same set, delivered the two ways AWS offers: on the write, and through the
        // dedicated tagging call afterwards.
        let on_write = format!("on-write/{index}");
        let written = put_object_tagged(&router, "tagging-paths-agree", &on_write, &header).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the tagged write must succeed, got {} with body {:?}",
            written.status,
            text(&written)
        );

        let after_write = format!("after-write/{index}");
        let plain = put_object(&router, "tagging-paths-agree", &after_write, BODY, &[]).await;
        assert_eq!(
            plain.status, 200,
            "case {case:?}: the untagged write must succeed, got {} with body {:?}",
            plain.status,
            text(&plain)
        );
        let tagged = put_object_tagging(&router, "tagging-paths-agree", &after_write, &tags).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: the tagging call must succeed, got {} with body {:?}",
            tagged.status,
            text(&tagged)
        );

        let from_header = get_object_tagging(&router, "tagging-paths-agree", &on_write, "").await;
        let from_call = get_object_tagging(&router, "tagging-paths-agree", &after_write, "").await;
        assert_eq!(
            tag_set_of(&from_header),
            tag_set_of(&from_call),
            "case {case:?}: the two ways of delivering one tag set must leave the same \
             state, got {:?} and {:?}",
            tag_set_of(&from_header),
            tag_set_of(&from_call)
        );
    }
}

#[tokio::test]
async fn a_write_replaces_the_whole_tag_set_of_the_object_it_replaces() {
    let router = router();
    bucket(&router, "tag-replacement").await;

    let first = put_object_tagged(&router, "tag-replacement", "staged.csv", "one=1&two=2").await;
    assert_eq!(
        first.status, 200,
        "the first write must succeed, got {} with body {:?}",
        first.status,
        text(&first)
    );
    let read = get_object_tagging(&router, "tag-replacement", "staged.csv", "").await;
    assert_eq!(
        tag_set_of(&read),
        vec![tag("one", "1"), tag("two", "2")],
        "the first write's tag set must be stored, got {:?}",
        tag_set_of(&read)
    );

    // A second write carrying a different set replaces it wholesale: nothing of the first
    // set survives.
    let second = put_object_tagged(&router, "tag-replacement", "staged.csv", "three=3").await;
    assert_eq!(
        second.status, 200,
        "the replacing write must succeed, got {} with body {:?}",
        second.status,
        text(&second)
    );
    let reread = get_object_tagging(&router, "tag-replacement", "staged.csv", "").await;
    assert_eq!(
        tag_set_of(&reread),
        vec![tag("three", "3")],
        "a write replaces the whole tag set rather than merging into it, got {:?}",
        tag_set_of(&reread)
    );

    // And a write carrying NO header leaves the key with no tags at all, because the
    // object it replaced took its tag set with it.
    let third = put_object(&router, "tag-replacement", "staged.csv", BODY, &[]).await;
    assert_eq!(
        third.status, 200,
        "the untagged write must succeed, got {} with body {:?}",
        third.status,
        text(&third)
    );
    let after = get_object_tagging(&router, "tag-replacement", "staged.csv", "").await;
    assert_eq!(
        tag_set_of(&after),
        Vec::new(),
        "a write with no tag set leaves the key with none, got {:?}",
        tag_set_of(&after)
    );
    let read_back = get_object(&router, "tag-replacement", "staged.csv", "", &[]).await;
    assert_eq!(
        read_back.header("x-amz-tagging-count"),
        None,
        "an object with no tags reports no count at all, got {:?}",
        read_back.header("x-amz-tagging-count")
    );
}

#[tokio::test]
async fn a_header_tag_set_past_a_published_limit_is_refused_and_stores_nothing() {
    let router = router();
    bucket(&router, "tagged-write-refusals").await;

    // The object the refusals must not touch: written with a tag set that is fine.
    let held = put_object_tagged(&router, "tagged-write-refusals", "held.csv", "keep=me").await;
    assert_eq!(
        held.status, 200,
        "the setup write must succeed, got {} with body {:?}",
        held.status,
        text(&held)
    );

    // One row per published bound, each ONE past it, plus the key-uniqueness the set has,
    // the empty key the modelled `Tag$Key` (`length {min 1}`) could never carry, and the
    // character class every tag shares. The code says WHICH class of restriction broke: a
    // set bigger than the resource may hold is `BadRequest`, an individual tag that
    // does not pass input validation is `InvalidTag`, and a HEADER that is not the URL query
    // string it is declared to be — a value half that is the encoding of no tag value, or one
    // tag named twice — is `InvalidArgument` about that header (see
    // `super::inline_tagging_header_grammar`).
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "one pair past the per-object count bound",
            (0..=MAX_TAGS_PER_OBJECT)
                .map(|index| format!("key-{index}=v"))
                .collect::<Vec<_>>()
                .join("&"),
            "BadRequest",
        ),
        (
            "many times the per-object count bound",
            (0..MAX_TAGS_PER_OBJECT * 20)
                .map(|index| format!("key-{index}=v"))
                .collect::<Vec<_>>()
                .join("&"),
            "BadRequest",
        ),
        (
            "a key one character past its bound",
            format!("{}=v", "k".repeat(MAX_TAG_KEY_CHARS + 1)),
            "InvalidTag",
        ),
        (
            "a value one character past its bound",
            format!("key={}", "v".repeat(MAX_TAG_VALUE_CHARS + 1)),
            "InvalidTag",
        ),
        (
            "the same key twice",
            "duplicate=first&duplicate=second".to_string(),
            "InvalidArgument",
        ),
        (
            "the same key twice among pairs that are fine",
            "alpha=1&beta=2&alpha=3".to_string(),
            "InvalidArgument",
        ),
        ("an empty key", "=value".to_string(), "InvalidTag"),
        (
            "a percent-escaped astral-plane symbol as the value",
            "mood=%F0%9F%98%80".to_string(),
            "InvalidArgument",
        ),
    ];

    for (case, header, code) in rows {
        let refused = put_object_tagged(
            &router,
            "tagged-write-refusals",
            "refused.csv",
            &header,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the tag set to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the code S3 refuses this restriction with, got {:?}",
            refused.error_code()
        );

        // A refused write stores nothing: neither the object nor its tags.
        let absent = get_object(&router, "tagged-write-refusals", "refused.csv", "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused write must store nothing, got GetObject {} with body \
             {:?}",
            absent.status,
            text(&absent)
        );

        // ... and leaves the object that IS held exactly as it was.
        let untouched = get_object_tagging(&router, "tagged-write-refusals", "held.csv", "").await;
        assert_eq!(
            tag_set_of(&untouched),
            vec![tag("keep", "me")],
            "case {case:?}: a refused write must leave the held object's tag set \
             untouched, got {:?}",
            tag_set_of(&untouched)
        );
    }
}
