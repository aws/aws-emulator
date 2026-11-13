// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an OBJECT's TAG SET: `PutObjectTagging` stores it and
//! `GetObjectTagging` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the
//! `Tagging` payload builder and the tag-set reader are the ones
//! [`super::bucket_lifecycle`], [`super::bucket_tagging`] and
//! [`super::object_and_tagging_roundtrip`] already own), so what is pinned is the wire
//! behaviour — status, payload, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutObjectTagging` "sets the supplied tag-set to an object that already exists in a
//!   bucket" and `GetObjectTagging` "returns the tag-set of an object", so a tag set reads
//!   back exactly as it was stored, in the order the request carried it; the container is
//!   the complete `Tagging`, so a second put REPLACES the set rather than merging into it.
//! * An object with no tag set associated reports an EMPTY `TagSet` (HTTP 200):
//!   `GetObjectTagging`'s documentation names no special error for that case, where
//!   `GetBucketTagging`'s names `NoSuchTagSet`, and `TagSet` is a required output member.
//! * A tag set AWS would refuse with `InvalidTag` — past the published 10 tags per object,
//!   128-character key or 256-character value, or naming one key twice — is refused with
//!   HTTP 400 and leaves the stored tag set untouched. The COUNT is the object's own
//!   published limit: a set a bucket accepts (up to 50) is refused on an object.
//! * A tagging call whose address this emulator does not hold answers which part is
//!   missing — `NoSuchBucket` or `NoSuchKey`, both HTTP 404 — and stores nothing.
//! * A tag set belongs to ONE object: it follows neither another key, nor the same key in
//!   another bucket, nor the bucket's own tag set, and a `PutObject` that replaces the
//!   object replaces its tag set with none ("You cannot use PutObject to only update a
//!   single piece of metadata for an existing object").
//! * `versionId` names ONE stored version, and a tag set belongs to that version: a call
//!   naming a version the key does not carry is that version's own 404
//!   (`NoSuchVersion`) and changes nothing.

use service_com_amazonaws_s3_amazons3::state::{
    MAX_TAGS_PER_BUCKET, MAX_TAGS_PER_OBJECT, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_tagging::{get_bucket_tagging, put_bucket_tagging, tag, tag_set_of, tagging_payload};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_storage::delete_object;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has. Asserted
/// against the emulator's own id where it is used, so it cannot silently become the owner.
const NOT_THE_OWNER: &str = "123456789012";

/// `PUT /<bucket>/<key>?tagging` — `PutObjectTagging`.
///
/// `pub(crate)` for the same reason as the reader below it: the suite for a tag set
/// delivered on the WRITE ([`super::object_write_tagging_header`]) compares the two paths
/// against each other rather than reimplementing this one.
pub(crate) async fn put_object_tagging(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    tags: &[(String, String)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}?tagging"),
        &[],
        &tagging_payload(tags),
    )
    .await
}

/// `GET /<bucket>/<key>?tagging` — `GetObjectTagging`, with whatever extra query
/// parameters the case under test carries.
///
/// `pub(crate)`, so the suite for a tag set delivered on the WRITE
/// ([`super::object_write_tagging_header`]) reads it back through this one reader rather
/// than standing up a second one.
pub(crate) async fn get_object_tagging(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}/{key}?tagging{query}"),
        &[],
        "",
    )
    .await
}

/// An object written at `key`, so a tagging call has something to tag.
///
/// `pub(crate)`: the suite for the REMOVAL of an object's tag set
/// ([`super::object_tag_set_removal`]) writes the objects it untags through this one.
pub(crate) async fn object(router: &MultiServiceRouter, bucket_name: &str, key: &str) {
    let written = put_object(router, bucket_name, key, b"payload", &[]).await;
    assert_eq!(
        written.status, 200,
        "the object {bucket_name}/{key} must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
}

/// One row of the round-trip table: the case name, the key the object is stored at, and
/// the tag set the put carries.
type RoundTripRow = (&'static str, &'static str, Vec<(String, String)>);

/// The table of tag sets every row of which must round-trip unchanged on an OBJECT.
///
/// The rows differ along what the store-and-report rule generalises over: how many tags
/// the set carries (none, one, three and the published per-object maximum), the order they
/// arrive in, how long a key and a value are (one character, empty, and both published
/// character bounds exactly), and which characters they contain (ASCII, non-ASCII, XML
/// metacharacters). The key each row is stored under varies too — a flat name, a prefixed
/// one and a deep path — because the rule is about the object at a key, whatever its
/// shape.
fn round_trip_rows() -> Vec<RoundTripRow> {
    vec![
        (
            "one tag, the shape a provisioning script sends",
            "flat-key",
            vec![tag("sim2real", "iam-role-trust-s3-service")],
        ),
        (
            "no tags at all: an empty TagSet is a tag set",
            "emptied",
            vec![],
        ),
        (
            "three tags keep the order the request carried, not key order",
            "dags/ordered.py",
            vec![tag("zeta", "3"), tag("alpha", "1"), tag("middle", "2")],
        ),
        (
            "a one-character key and an empty value",
            "minimal",
            vec![tag("k", "")],
        ),
        (
            "a key and a value exactly at the published character bounds",
            "dags/sub/dir/at-the-bounds.txt",
            vec![tag(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        ),
        (
            "non-ASCII key and value are stored as given",
            "unicode.txt",
            vec![tag("プロジェクト", "Grüße-日本語")],
        ),
        (
            "a value carrying every punctuation character a tag may hold",
            "escaped.xml",
            vec![tag("expression", "a + b = c.d_e:f/g@h")],
        ),
        (
            "the per-object tag count bound",
            "at-the-count-bound",
            (0..MAX_TAGS_PER_OBJECT)
                .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn an_objects_tag_set_round_trips_through_put_and_get_object_tagging() {
    let router = router();
    bucket(&router, "object-tagging").await;

    for (case, key, tags) in round_trip_rows() {
        object(&router, "object-tagging", key).await;

        let put = put_object_tagging(&router, "object-tagging", key, &tags).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutObjectTagging, got {} with \
             body {:?}",
            put.status,
            text(&put)
        );

        let read = get_object_tagging(&router, "object-tagging", key, "").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetObjectTagging on the object just \
             tagged, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            tag_set_of(&read),
            tags,
            "case {case:?}: GetObjectTagging must report exactly the stored tag set, in \
             order, got {:?}",
            tag_set_of(&read)
        );

        // "You can associate tags with an object by sending a PUT request against the
        // tagging subresource" — the request carries the whole `Tagging` container, so the
        // second put REPLACES the set: nothing of the first one may survive it.
        let replacement = vec![tag("only", "this-one")];
        let second = put_object_tagging(&router, "object-tagging", key, &replacement).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            second.status,
            text(&second)
        );
        let reread = get_object_tagging(&router, "object-tagging", key, "").await;
        assert_eq!(
            tag_set_of(&reread),
            replacement,
            "case {case:?}: a put replaces the whole tag set rather than merging into it, \
             got {:?}",
            tag_set_of(&reread)
        );

        // Tagging an object changes nothing else about it: its bytes read back unchanged.
        let object_read = get_object(&router, "object-tagging", key, "", &[]).await;
        assert_eq!(
            object_read.body.as_ref(),
            b"payload",
            "case {case:?}: tagging must not touch the object's bytes, got {:?}",
            text(&object_read)
        );
    }
}

#[tokio::test]
async fn an_object_with_no_tag_set_reports_an_empty_tag_set() {
    let router = router();
    bucket(&router, "untagged-objects").await;

    // Keys of different shapes, each written and never tagged: every one answers 200 with
    // an empty `TagSet`, never the `NoSuchTagSet` that is `GetBucketTagging`'s rule.
    for key in ["never-tagged", "dags/never-tagged.py", "a/deep/path.bin"] {
        object(&router, "untagged-objects", key).await;

        let read = get_object_tagging(&router, "untagged-objects", key, "").await;
        assert_eq!(
            read.status, 200,
            "case {key:?}: an untagged object answers 200 with an empty TagSet, got {} \
             with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            tag_set_of(&read),
            Vec::new(),
            "case {key:?}: an untagged object reports no tags, got {:?}",
            tag_set_of(&read)
        );
    }

    // And an emptied tag set is the same state as one that was never set: a put of an
    // empty `TagSet` over a stored set leaves the object reporting none.
    object(&router, "untagged-objects", "emptied").await;
    let stored = vec![tag("before", "emptying")];
    let put = put_object_tagging(&router, "untagged-objects", "emptied", &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        text(&put)
    );
    let emptied = put_object_tagging(&router, "untagged-objects", "emptied", &[]).await;
    assert_eq!(
        emptied.status, 200,
        "an empty TagSet is a tag set a put accepts, got {} with body {:?}",
        emptied.status,
        text(&emptied)
    );
    let read = get_object_tagging(&router, "untagged-objects", "emptied", "").await;
    assert_eq!(
        read.status, 200,
        "an emptied object answers 200, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        tag_set_of(&read),
        Vec::new(),
        "an emptied tag set reads back as no tags, got {:?}",
        tag_set_of(&read)
    );
}

/// One row of the refusal table: the case name, the tag set it sends, and the code S3
/// refuses it with.
type RefusedTagSet = (&'static str, Vec<(String, String)>, &'static str);

#[tokio::test]
async fn a_tag_set_past_a_published_limit_is_refused_and_changes_nothing() {
    let router = router();
    bucket(&router, "object-tag-refusals").await;
    object(&router, "object-tag-refusals", "tagged").await;

    // The tag set the object holds throughout: every refusal below must leave it exactly
    // as it is.
    let stored = vec![tag("keep", "me"), tag("and", "me-too")];
    let put = put_object_tagging(&router, "object-tag-refusals", "tagged", &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        text(&put)
    );

    // (case, the refused tag set, the code S3 refuses it with). One row per published
    // bound, each ONE past it, plus the key-uniqueness the set itself has and the
    // character class every tag shares. The code says WHICH class of restriction broke: a
    // set bigger than the resource may hold is `BadRequest`, and an individual tag that
    // does not pass input validation is `InvalidTag`.
    let rows: Vec<RefusedTagSet> = vec![
        (
            "one tag past the per-object count bound",
            (0..=MAX_TAGS_PER_OBJECT)
                .map(|index| tag(&format!("key-{index}"), "v"))
                .collect(),
            "BadRequest",
        ),
        (
            "a key one character past its bound",
            vec![tag(&"k".repeat(MAX_TAG_KEY_CHARS + 1), "v")],
            "InvalidTag",
        ),
        (
            "a value one character past its bound",
            vec![tag("key", &"v".repeat(MAX_TAG_VALUE_CHARS + 1))],
            "InvalidTag",
        ),
        (
            "the same key twice",
            vec![tag("duplicate", "first"), tag("duplicate", "second")],
            "InvalidTag",
        ),
        (
            "the same key twice among tags that are fine",
            vec![tag("alpha", "1"), tag("beta", "2"), tag("alpha", "3")],
            "InvalidTag",
        ),
        (
            "a value carrying an astral-plane symbol",
            vec![tag("mood", "\u{1f600}")],
            "InvalidTag",
        ),
        (
            "a value carrying an XML metacharacter",
            vec![tag("expression", "a < b")],
            "InvalidTag",
        ),
        (
            "a KEY carrying a character outside the class",
            vec![tag("br(acket)", "v")],
            "InvalidTag",
        ),
    ];

    for (case, tags, code) in rows {
        let refused = put_object_tagging(&router, "object-tag-refusals", "tagged", &tags).await;
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

        let read = get_object_tagging(&router, "object-tag-refusals", "tagged", "").await;
        assert_eq!(
            tag_set_of(&read),
            stored,
            "case {case:?}: a refused put must leave the stored tag set untouched, got {:?}",
            tag_set_of(&read)
        );
    }

    // The COUNT bound is the one published limit that differs between the two tag sets: a
    // set of more than ten tags is a tag set a BUCKET holds (up to fifty) and an OBJECT
    // does not, so the very same set is accepted on the bucket and refused on its object.
    let eleven: Vec<(String, String)> = (0..=MAX_TAGS_PER_OBJECT)
        .map(|index| tag(&format!("shared-{index}"), "v"))
        .collect();
    assert!(
        eleven.len() > MAX_TAGS_PER_OBJECT && eleven.len() <= MAX_TAGS_PER_BUCKET,
        "the cross-check set must sit between the two published counts, got {} tags",
        eleven.len()
    );
    let on_bucket = put_bucket_tagging(&router, "object-tag-refusals", &eleven).await;
    assert_eq!(
        on_bucket.status, 204,
        "a bucket holds up to {MAX_TAGS_PER_BUCKET} tags, so the set must be accepted \
         there, got {} with body {:?}",
        on_bucket.status,
        text(&on_bucket)
    );
    let on_object = put_object_tagging(&router, "object-tag-refusals", "tagged", &eleven).await;
    assert_eq!(
        on_object.status, 400,
        "an object holds at most {MAX_TAGS_PER_OBJECT} tags, so the same set must be \
         refused there, got {} with body {:?}",
        on_object.status,
        text(&on_object)
    );
}

#[tokio::test]
async fn tagging_an_address_this_emulator_does_not_hold_answers_the_missing_part() {
    let router = router();
    bucket(&router, "held-object-bucket").await;
    object(&router, "held-object-bucket", "held").await;

    let tags = vec![tag("sim2real", "no-such-address")];

    // A bucket this emulator does not hold: a different name, a prefix of the held one, an
    // extension of it, and one differing only in case (bucket names are case-sensitive).
    for absent in [
        "absent-bucket",
        "held-object",
        "held-object-bucket-2",
        "Held-Object-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutObjectTagging",
                put_object_tagging(&router, absent, "held", &tags).await,
            ),
            (
                "GetObjectTagging",
                get_object_tagging(&router, absent, "held", "").await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {absent:?}/{operation}: expected HTTP 404 for a bucket this \
                 emulator does not hold, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {absent:?}/{operation}: expected the bucket to be named as the \
                 missing part, got {:?}",
                answer.error_code()
            );
        }
    }

    // A key the held bucket does not hold, in the shapes a client sends.
    for absent in ["absent-key", "hel", "held-2", "Held", "dags/held"] {
        for (operation, answer) in [
            (
                "PutObjectTagging",
                put_object_tagging(&router, "held-object-bucket", absent, &tags).await,
            ),
            (
                "GetObjectTagging",
                get_object_tagging(&router, "held-object-bucket", absent, "").await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {absent:?}/{operation}: expected HTTP 404 for a key this bucket \
                 does not hold, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchKey",
                "case {absent:?}/{operation}: expected the key to be named as the missing \
                 part, got {:?}",
                answer.error_code()
            );
        }
    }

    // None of the refusals above created anything: the bucket still holds exactly the one
    // object, still untagged.
    let read = get_object_tagging(&router, "held-object-bucket", "held", "").await;
    assert_eq!(
        read.status, 200,
        "the held object must still be readable, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        tag_set_of(&read),
        Vec::new(),
        "a refused tagging call must not have tagged the held object, got {:?}",
        tag_set_of(&read)
    );
    let absent_object = get_object_tagging(&router, "held-object-bucket", "absent-key", "").await;
    assert_eq!(
        absent_object.status, 404,
        "a refused tagging call must not have created the key, got {}",
        absent_object.status
    );
}

#[tokio::test]
async fn a_tag_set_belongs_to_one_object_and_goes_when_the_object_does() {
    let router = router();
    bucket(&router, "tag-scope-one").await;
    bucket(&router, "tag-scope-two").await;

    // The same key in two buckets, and a second key beside it, each tagged differently:
    // what a read reports must be the tag set of the object it addressed.
    let addresses = [
        ("tag-scope-one", "shared-key", vec![tag("where", "one")]),
        ("tag-scope-two", "shared-key", vec![tag("where", "two")]),
        (
            "tag-scope-one",
            "other-key",
            vec![tag("where", "one"), tag("which", "other")],
        ),
    ];
    for (bucket_name, key, tags) in &addresses {
        object(&router, bucket_name, key).await;
        let put = put_object_tagging(&router, bucket_name, key, tags).await;
        assert_eq!(
            put.status, 200,
            "case {bucket_name}/{key}: the put must succeed, got {} with body {:?}",
            put.status,
            text(&put)
        );
    }
    for (bucket_name, key, tags) in &addresses {
        let read = get_object_tagging(&router, bucket_name, key, "").await;
        assert_eq!(
            &tag_set_of(&read),
            tags,
            "case {bucket_name}/{key}: each object reports its own tag set, got {:?}",
            tag_set_of(&read)
        );
    }

    // An object's tag set is not the bucket's: tagging objects leaves the bucket with no
    // tag set, which is the case `GetBucketTagging` answers `NoSuchTagSet` for.
    let bucket_read = get_bucket_tagging(&router, "tag-scope-one").await;
    assert_eq!(
        bucket_read.error_code(),
        "NoSuchTagSet",
        "tagging an object must not tag its bucket, got {} with body {:?}",
        bucket_read.status,
        text(&bucket_read)
    );

    // A `PutObject` REPLACES the whole object, so the tag set of the object it replaced
    // goes with it: the key is readable again and carries no tags.
    object(&router, "tag-scope-one", "shared-key").await;
    let after_replace = get_object_tagging(&router, "tag-scope-one", "shared-key", "").await;
    assert_eq!(
        after_replace.status, 200,
        "the replaced object must still be readable, got {} with body {:?}",
        after_replace.status,
        text(&after_replace)
    );
    assert_eq!(
        tag_set_of(&after_replace),
        Vec::new(),
        "a write replaces the whole object, so its tag set goes with it, got {:?}",
        tag_set_of(&after_replace)
    );
    // And the other bucket's object of the same key kept its own.
    let untouched = get_object_tagging(&router, "tag-scope-two", "shared-key", "").await;
    assert_eq!(
        tag_set_of(&untouched),
        vec![tag("where", "two")],
        "replacing one bucket's object must not touch another's, got {:?}",
        tag_set_of(&untouched)
    );

    // Deleting the object releases its tag set with it: there is no object left to report
    // a tag set of.
    let deleted = delete_object(&router, "tag-scope-one", "other-key").await;
    assert_eq!(
        deleted.status, 204,
        "the delete must succeed, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    let gone = get_object_tagging(&router, "tag-scope-one", "other-key", "").await;
    assert_eq!(
        gone.status, 404,
        "a deleted object has no tag set to report, got {} with body {:?}",
        gone.status,
        text(&gone)
    );
    assert_eq!(
        gone.error_code(),
        "NoSuchKey",
        "a deleted object's tagging read names the key as missing, got {:?}",
        gone.error_code()
    );
}

#[tokio::test]
async fn a_tagging_parameter_this_emulator_cannot_serve_is_refused_not_guessed() {
    let router = router();
    bucket(&router, "tag-unserved").await;
    object(&router, "tag-unserved", "versioned").await;
    let stored = vec![tag("current", "object")];
    let put = put_object_tagging(&router, "tag-unserved", "versioned", &stored).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        text(&put)
    );

    // `versionId` names ONE version of an object, and a tag set belongs to that version:
    // "to retrieve tags of any other version, use the versionId query parameter"
    // (`GetObjectTagging`). The ids below are the shapes a client sends: the literal `null`,
    // which names the version a write made while the bucket was not versioning-enabled —
    // this object's — and a generated-looking id no version of the key carries, which is
    // that version's own 404.
    let read = get_object_tagging(&router, "tag-unserved", "versioned", "&versionId=null").await;
    assert_eq!(
        (read.status, tag_set_of(&read)),
        (200, stored.clone()),
        "case \"null\": the null version is this object, so its tag set is the one stored, \
         got {} with body {:?}",
        read.status,
        text(&read)
    );
    for version in ["3HL4kqtJlcpXroDTDmjVBH40Nrjfkd", "NoSuchVersionAtAll00000000000000"] {
        let read = get_object_tagging(
            &router,
            "tag-unserved",
            "versioned",
            &format!("&versionId={version}"),
        )
        .await;
        assert_eq!(
            (read.status, read.error_code()),
            (404, "NoSuchVersion".to_string()),
            "case {version:?}: GetObjectTagging naming a version the key does not carry is \
             that version's 404, got {} with body {:?}",
            read.status,
            text(&read)
        );

        let written = send(
            &router,
            "PUT",
            &format!("/tag-unserved/versioned?tagging&versionId={version}"),
            &[],
            &tagging_payload(&[tag("version", version)]),
        )
        .await;
        assert_eq!(
            (written.status, written.error_code()),
            (404, "NoSuchVersion".to_string()),
            "case {version:?}: PutObjectTagging naming a version the key does not carry is \
             that version's 404, got {} with body {:?}",
            written.status,
            text(&written)
        );
    }

    // A request the emulator refused changed nothing: the object still reports the tag set
    // it was given.
    let read = get_object_tagging(&router, "tag-unserved", "versioned", "").await;
    assert_eq!(
        tag_set_of(&read),
        stored,
        "a refused versioned call must leave the current object's tag set untouched, got \
         {:?}",
        tag_set_of(&read)
    );

    // An `x-amz-expected-bucket-owner` that is not this emulator's account is "403
    // Forbidden (access denied)" for both halves, and changes nothing either.
    assert_ne!(
        NOT_THE_OWNER,
        engine::emulator_account_id(),
        "the test's non-owner account id must not be the account this emulator serves"
    );
    for (operation, method, body) in [
        ("GetObjectTagging", "GET", String::new()),
        (
            "PutObjectTagging",
            "PUT",
            tagging_payload(&[tag("owner", "mismatch")]),
        ),
    ] {
        let denied = send(
            &router,
            method,
            "/tag-unserved/versioned?tagging",
            &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
            &body,
        )
        .await;
        assert_eq!(
            denied.status, 403,
            "case {operation}: an expected bucket owner that is not the owner is denied, \
             got {} with body {:?}",
            denied.status,
            text(&denied)
        );
        assert_eq!(
            denied.error_code(),
            "AccessDenied",
            "case {operation}: expected the documented code, got {:?}",
            denied.error_code()
        );
    }
    let read = get_object_tagging(&router, "tag-unserved", "versioned", "").await;
    assert_eq!(
        tag_set_of(&read),
        stored,
        "a denied call must leave the tag set untouched, got {:?}",
        tag_set_of(&read)
    );
}
