// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the GRAMMAR the `x-amz-tagging` header a write carries has to be in.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_tagging`] already own), so what is pinned is the wire behaviour — status,
//! error code, the error document's members, and what the object holds afterwards — rather
//! than any handler's internals.
//!
//! The rule under test, asserted over a SET of headers rather than one request:
//!
//! > "The tag-set for the object. The tag-set must be encoded as URL Query parameters. (For
//! > example, "Key1=Value1")" (`PutObjectRequest$Tagging`, and the same sentence on
//! > `CopyObjectRequest$Tagging` and `CreateMultipartUploadRequest$Tagging`) — so each pair of
//! > the header NAMES a tag and gives its value as the URL-ENCODED form of that tag's value.
//! > A pair whose value half is not the encoding of any tag value — an escape that is not one,
//! > escapes whose bytes are not UTF-8, or text outside the class a tag value may carry — and
//! > a header naming one tag twice encode no tag set at all, and are refused for the HEADER:
//! > `InvalidArgument` (400) with the sentence stating that grammar, `<ArgumentName>` naming
//! > the header and `<ArgumentValue>` echoing it as sent. A pair's NAME is the tag's key as it
//! > stands, so a key the published restrictions refuse is the other refusal — `InvalidTag`
//! > about that tag — and a header whose pairs break neither is stored.
//!
//! The rows differ along what the grammar generalises over: how many pairs the header
//! carries, how a value is escaped (percent escapes, `+` for a space, escaped reserved
//! characters, an escaped non-ASCII pair), the degenerate forms a query string allows (an
//! empty value, a pair with no `=`), each way a value half can be outside the grammar — a
//! character no tag value may carry, an escape that is cut short or is not hexadecimal, and
//! the same tag name twice — and, on the other side of that line, a NAME carrying the same
//! character, which is the tag's key and is refused as one.
//!
//! Each row is driven through BOTH writes that take a tag set in that header — `PutObject`
//! and the `CopyObject` that replaces the destination's tag set — so one grammar decides
//! both, and a refused write is shown to have stored nothing.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::bucket_tagging::tag_set_of;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_tagging::get_object_tagging;

/// The bytes every object in this suite is written with.
const BODY: &[u8] = b"a,b\n1,2\n";

/// The sentence S3 states the grammar in — fixed text naming the header and the whole
/// requirement.
const ENCODING_MESSAGE: &str = "The header 'x-amz-tagging' shall be encoded as UTF-8 then \
                                URLEncoded URL query parameters without tag name duplicates.";

/// What a header earns: stored as the tag set it encodes, refused for the HEADER, or refused
/// for a TAG the header did encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The header encodes a storable tag set.
    Stored,
    /// The header is outside the grammar, so it encodes no tag set at all.
    HeaderOutsideTheGrammar,
    /// The header encodes a tag set whose own tag breaks a published restriction.
    TagNotStorable,
}

/// One row of the header table: the case, the header as sent, and what it earns.
type HeaderRow = (&'static str, &'static str, Outcome);

/// The headers every write delivering a tag set is judged over — see the rows' own classes in
/// this file's header.
const HEADER_ROWS: [HeaderRow; 15] = [
    ("the example the member's documentation gives", "Key1=Value1", Outcome::Stored),
    ("several pairs", "zeta=3&alpha=1&middle=2", Outcome::Stored),
    ("a pair with an empty value", "emptied=", Outcome::Stored),
    ("a pair with no = at all", "novalue", Outcome::Stored),
    ("a plus for a space", "project=two+words", Outcome::Stored),
    ("percent escapes", "project=two%20words", Outcome::Stored),
    (
        "escaped reserved characters",
        "expression=a%20%2B%20b%20%3D%20c%2Fd",
        Outcome::Stored,
    ),
    ("an escaped non-ASCII pair", "k%C3%A9y=v%C3%A4lue", Outcome::Stored),
    (
        "punctuation a tag value may carry",
        "path=logs/2026:01@host",
        Outcome::Stored,
    ),
    (
        "a space, which a tag value may carry",
        "summary=quarterly results",
        Outcome::Stored,
    ),
    (
        "a character no tag may carry in the value",
        "summary=quarterly(results)",
        Outcome::HeaderOutsideTheGrammar,
    ),
    (
        "an escape cut short",
        "project=two%2",
        Outcome::HeaderOutsideTheGrammar,
    ),
    (
        "an escape that is not hexadecimal",
        "project=two%zzwords",
        Outcome::HeaderOutsideTheGrammar,
    ),
    (
        "the same tag name twice",
        "project=first&project=second",
        Outcome::HeaderOutsideTheGrammar,
    ),
    (
        "a character no tag may carry in the NAME",
        "summary(q3)=yes",
        Outcome::TagNotStorable,
    ),
];

/// `PUT /<bucket>/<key>` carrying `tagging` in `x-amz-tagging`.
async fn put_object_tagged(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    tagging: &str,
) -> Answer {
    put_object(router, name, key, BODY, &[("x-amz-tagging", tagging)]).await
}

/// `PUT /<bucket>/<destination>` copying `source` and REPLACING its tag set with `tagging`.
async fn copy_object_tagged(
    router: &MultiServiceRouter,
    name: &str,
    source: &str,
    destination: &str,
    tagging: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{name}/{destination}"),
        &[
            ("x-amz-copy-source", &format!("/{name}/{source}")),
            ("x-amz-tagging-directive", "REPLACE"),
            ("x-amz-tagging", tagging),
        ],
        b"",
    )
    .await
}

/// Assert `answer` is the refusal the OUTCOME names, for the header `tagging` as sent.
fn assert_outcome(case: &str, tagging: &str, outcome: Outcome, answer: &Answer) {
    match outcome {
        Outcome::Stored => assert_eq!(
            answer.status,
            200,
            "case {case:?}: a header in the grammar whose tags are storable is served, got {} \
             with body {:?}",
            answer.status,
            text(answer)
        ),
        Outcome::HeaderOutsideTheGrammar => {
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(answer)
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case:?}: a header outside the grammar is a refused ARGUMENT, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                ENCODING_MESSAGE,
                "case {case:?}: expected the sentence S3 states the grammar in, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                answer.element("ArgumentName").as_deref(),
                Some("x-amz-tagging"),
                "case {case:?}: the document names the header it rejected, got {:?}",
                answer.element("ArgumentName")
            );
            assert_eq!(
                answer.element("ArgumentValue").as_deref(),
                Some(tagging),
                "case {case:?}: the document echoes the header as sent, got {:?}",
                answer.element("ArgumentValue")
            );
        }
        Outcome::TagNotStorable => {
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(answer)
            );
            assert_eq!(
                answer.error_code(),
                "InvalidTag",
                "case {case:?}: a tag the header DID encode is refused about the TAG, got {:?}",
                answer.error_code()
            );
        }
    }
}

#[tokio::test]
async fn a_write_judges_its_tagging_header_against_the_grammar_that_header_is_declared_in() {
    let router = router();
    let name = "inline-tagging-grammar";
    bucket(&router, name).await;
    // The object every copy row copies, and the one no refusal may touch.
    let source = put_object_tagged(&router, name, "source.csv", "keep=me").await;
    assert_eq!(
        source.status,
        200,
        "the setup write must be accepted, got {} with body {:?}",
        source.status,
        text(&source)
    );

    for (index, (case, tagging, outcome)) in HEADER_ROWS.iter().enumerate() {
        // One key per row, so a row that stores a tag set cannot decide another's read-back.
        let written = format!("written-{index}.csv");
        let answer = put_object_tagged(&router, name, &written, tagging).await;
        assert_outcome(&format!("PutObject: {case}"), tagging, *outcome, &answer);

        let copied = format!("copied-{index}.csv");
        let copy = copy_object_tagged(&router, name, "source.csv", &copied, tagging).await;
        assert_outcome(&format!("CopyObject: {case}"), tagging, *outcome, &copy);

        // A refused write stores nothing; an accepted one stores the tag set the header
        // encoded, which both paths then report alike.
        for key in [&written, &copied] {
            let read = get_object(&router, name, key, "", &[]).await;
            match outcome {
                Outcome::Stored => assert_eq!(
                    read.status,
                    200,
                    "case {case:?}: the object {key:?} must be stored, got {} with body {:?}",
                    read.status,
                    text(&read)
                ),
                _ => assert_eq!(
                    read.status,
                    404,
                    "case {case:?}: a refused write must store nothing at {key:?}, got {} with \
                     body {:?}",
                    read.status,
                    text(&read)
                ),
            }
        }
        if *outcome == Outcome::Stored {
            let from_write = get_object_tagging(&router, name, &written, "").await;
            let from_copy = get_object_tagging(&router, name, &copied, "").await;
            assert_eq!(
                tag_set_of(&from_write),
                tag_set_of(&from_copy),
                "case {case:?}: one grammar decides both writes, so the two tag sets must be \
                 the same, got {:?} and {:?}",
                tag_set_of(&from_write),
                tag_set_of(&from_copy)
            );
            assert!(
                !tag_set_of(&from_write).is_empty(),
                "case {case:?}: the header named a pair, so a tag must be stored, got {:?}",
                tag_set_of(&from_write)
            );
        }
    }

    // And the object the refusals copied from is exactly as it was.
    let untouched = get_object_tagging(&router, name, "source.csv", "").await;
    assert_eq!(
        tag_set_of(&untouched),
        vec![("keep".to_string(), "me".to_string())],
        "a refused write must leave the source object's tag set untouched, got {:?}",
        tag_set_of(&untouched)
    );
}
