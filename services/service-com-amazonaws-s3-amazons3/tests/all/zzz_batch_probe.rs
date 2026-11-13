// SPDX-License-Identifier: CC0-1.0

//! The PROBE a client makes with the TEXT of a request document's element, end to end — the
//! batch delete that carries a boolean and an object key, and the configuration write that
//! carries four booleans.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::bucket_public_access_block`] already own), so what is pinned is the wire
//! behaviour — status, the result document, what the bucket reports afterwards — rather than
//! any codec's internals.
//!
//! The two rules under test, each asserted over a SET of spellings rather than one request:
//!
//! > A boolean element of a request document is read in the lexical space the schema S3
//! > publishes for it gives a boolean: `true`, `false`, `1` and `0`, with surrounding
//! > whitespace allowed — so a document spelling one the NUMERIC way states the same value as
//! > the word, and text in neither spelling states no boolean at all and is refused.
//!
//! > The text of an element that carries an OBJECT KEY is the key itself, whitespace and all:
//! > the key a batch delete names is the key it takes away, and the key a tag set states is
//! > the key it stores, whether that key is composed of whitespace, begins or ends with it, or
//! > carries it inside.
//!
//! Their sources: a request document is validated against the published schema, where a boolean
//! element is an `xsd:boolean`, whose lexical space is exactly `{true, false, 1, 0}` (W3C XML
//! Schema Part 2: Datatypes, §3.2.2.1) — the "0" and "1" being the numeric spellings of the
//! other two; and `DeleteObjects` names a whitespace key in its own documentation of
//! `Delete$Objects` ("an object that's composed entirely of whitespace characters is not
//! supported by the `DeleteObjects` API operation" for a DIRECTORY bucket, which is the bucket
//! kind that does not support it). The documents driven here are the two kinds that carry a
//! boolean — `Delete$Quiet` on `DeleteObjects` ("element to enable quiet mode for the request")
//! and the four `PublicAccessBlockConfiguration` settings on `PutPublicAccessBlock` — each read
//! back through the operation that reports it.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_s3_amazons3::service::wire::{
    XML_BOOLEAN_DOCUMENT_ELEMENTS, XML_OBJECT_KEY_DOCUMENT_ELEMENTS,
};

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_public_access_block::{get_public_access_block, reported_flags};
use super::bucket_tagging::tag_set_of;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_storage::{list_objects_v2, listing_of};
use super::object_tagging::{get_object_tagging, put_object_tagging};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// `POST /<bucket>?delete` with one identifier and the `Quiet` element spelled exactly as
/// `quiet` gives it (and no `Quiet` element at all for `None`).
async fn delete_objects(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    quiet: Option<&str>,
) -> Answer {
    let mut payload = format!("<Delete xmlns=\"{XMLNS}\"><Object><Key>{key}</Key></Object>");
    if let Some(quiet) = quiet {
        payload.push_str(&format!("<Quiet>{quiet}</Quiet>"));
    }
    payload.push_str("</Delete>");
    send(router, "POST", &format!("/{name}?delete"), &[], &payload).await
}

/// `PUT /<bucket>?publicAccessBlock` with every setting spelled exactly as `spelling` gives
/// it.
async fn put_public_access_block_spelled(
    router: &MultiServiceRouter,
    name: &str,
    spelling: &str,
) -> Answer {
    let mut payload = format!("<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">");
    for setting in SETTINGS {
        payload.push_str(&format!("<{setting}>{spelling}</{setting}>"));
    }
    payload.push_str("</PublicAccessBlockConfiguration>");
    send(
        router,
        "PUT",
        &format!("/{name}?publicAccessBlock"),
        &[],
        &payload,
    )
    .await
}

/// Every member `PublicAccessBlockConfiguration` publishes, in the model's own order
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`), each targeting the
/// boolean `Setting`.
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// One row of the spelling table: the case, the text the element carries, and the boolean it
/// states — `None` for text in neither spelling, which states no boolean at all.
type SpellingRow = (&'static str, &'static str, Option<bool>);

/// Every spelling the published lexical space admits, beside text outside it.
///
/// The rows differ along what the rule generalises over: which of the two words, which of the
/// two numeric spellings, and the surrounding whitespace the space allows — plus, for the
/// refusal rows, a word in the wrong case, a number outside the space and text that is no
/// boolean in any reading.
const SPELLINGS: [SpellingRow; 10] = [
    ("the word true", "true", Some(true)),
    ("the word false", "false", Some(false)),
    ("the numeric spelling of true", "1", Some(true)),
    ("the numeric spelling of false", "0", Some(false)),
    ("a word with surrounding whitespace", " true ", Some(true)),
    ("a numeric spelling with surrounding whitespace", " 0 ", Some(false)),
    ("a word in another case", "True", None),
    ("a number outside the space", "2", None),
    ("a word that is not one of the two", "yes", None),
    ("no text at all", "", None),
];

/// This service's Smithy model's shapes, parsed.
///
/// The one reading of the model in this suite, so the two catalogs below are derived from the
/// same source.
fn model_shapes() -> serde_json::Map<String, serde_json::Value> {
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object")
        .clone()
}

/// The shapes every request DOCUMENT of this service is rooted at: the structured
/// `@httpPayload` members of the operation inputs. A member bound to a header, a query
/// parameter or a URI label is not in a document at all, and a scalar payload IS the body
/// rather than a document.
fn request_document_shapes(shapes: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let mut rooted: Vec<String> = Vec::new();
    for (_, shape) in shapes.iter().filter(|(_, shape)| shape["type"] == "operation") {
        let Some(input) = shape["input"]["target"].as_str() else {
            continue;
        };
        let Some(members) = shapes.get(input).and_then(|shape| shape["members"].as_object()) else {
            continue;
        };
        for (_, member) in members {
            if member["traits"].get("smithy.api#httpPayload").is_none() {
                continue;
            }
            let Some(target) = member["target"].as_str() else {
                continue;
            };
            let kind = shapes
                .get(target)
                .and_then(|shape| shape["type"].as_str())
                .unwrap_or_default();
            if matches!(kind, "structure" | "union" | "list" | "map") {
                rooted.push(target.to_string());
            }
        }
    }
    rooted
}

/// Every element a request document carries a member of `kind` in — walked from the shapes
/// those documents are rooted at and following each member's target, under the element name
/// restXml writes each with (its `@xmlName`, else the member's own name).
///
/// `kind` is read as the member's TARGET: its shape `type` for [`element_type`] and its shape
/// id for a named shape, which is what tells an object key from any other string.
fn document_elements(by_target: &dyn Fn(&str, &serde_json::Value) -> bool) -> BTreeSet<String> {
    let shapes = model_shapes();
    let mut pending: Vec<String> = request_document_shapes(&shapes);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    while let Some(shape_id) = pending.pop() {
        if !seen.insert(shape_id.clone()) {
            continue;
        }
        let Some(shape) = shapes.get(&shape_id) else {
            continue;
        };
        // A list or a map carries its element shape rather than members.
        for target in [
            shape["member"]["target"].as_str(),
            shape["value"]["target"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            pending.push(target.to_string());
        }
        let Some(members) = shape["members"].as_object() else {
            continue;
        };
        for (name, member) in members {
            let Some(target) = member["target"].as_str() else {
                continue;
            };
            let targeted = shapes.get(target).cloned().unwrap_or_default();
            if by_target(target, &targeted) {
                found.insert(
                    member["traits"]["smithy.api#xmlName"]
                        .as_str()
                        .unwrap_or(name)
                        .to_string(),
                );
                continue;
            }
            pending.push(target.to_string());
        }
    }
    found
}

#[test]
fn the_boolean_element_catalog_matches_the_model() {
    // Every element a request document of this service binds a BOOLEAN member from, read from
    // the model itself rather than from a second hand-written list.
    let derived = document_elements(&|_, targeted| targeted["type"] == "boolean");
    let declared: BTreeSet<String> = XML_BOOLEAN_DOCUMENT_ELEMENTS
        .iter()
        .map(|element| (*element).to_string())
        .collect();
    assert_eq!(
        declared, derived,
        "XML_BOOLEAN_DOCUMENT_ELEMENTS must name exactly the elements a request document of \
         this service binds a boolean from; declared {declared:?}, the model binds {derived:?}"
    );
}

#[test]
fn the_object_key_element_catalog_matches_the_model() {
    // And every element such a document carries an OBJECT KEY in: a member whose target is
    // the `ObjectKey` shape, which is the shape every member carrying a key targets.
    let derived = document_elements(&|target, _| target == OBJECT_KEY_SHAPE);
    let declared: BTreeSet<String> = XML_OBJECT_KEY_DOCUMENT_ELEMENTS
        .iter()
        .map(|element| (*element).to_string())
        .collect();
    assert_eq!(
        declared, derived,
        "XML_OBJECT_KEY_DOCUMENT_ELEMENTS must name exactly the elements a request document of \
         this service carries an object key in; declared {declared:?}, the model carries \
         {derived:?}"
    );
}

/// The model shape an object KEY is: the one every member carrying a key targets.
const OBJECT_KEY_SHAPE: &str = "com.amazonaws.s3#ObjectKey";

#[tokio::test]
async fn a_batch_delete_addresses_the_key_its_document_spells_whitespace_and_all() {
    let router = router();
    let name = "batch-delete-keys";
    bucket(&router, name).await;

    // (case, the key, the path it is written at). The rows differ along WHERE the whitespace
    // of a key falls — a key that is nothing but whitespace, one that begins or ends with it,
    // one that carries it inside, and one with none at all — because that is what the
    // trimming a codec does to an element's text generalises over. The whitespace-only key is
    // the one `DeleteObjects` names in its own documentation ("an object that's composed
    // entirely of whitespace characters"), and every one of them is a key S3 stores.
    let rows: [(&str, &str, &str); 6] = [
        ("one space", " ", "%20"),
        ("two spaces", "  ", "%20%20"),
        ("a tab", "\t", "%09"),
        ("a leading space", " leading.csv", "%20leading.csv"),
        ("a trailing space", "trailing.csv ", "trailing.csv%20"),
        ("whitespace inside", "two words.csv", "two%20words.csv"),
    ];
    for (case, key, path) in rows {
        let written = put_object(&router, name, path, b"a,b\n", &[]).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the setup write of {key:?} must be accepted, got {} with body {:?}",
            written.status,
            text(&written)
        );
    }
    let listed = listing_of(&list_objects_v2(&router, name, "").await).keys();
    for (case, key, _) in rows {
        assert!(
            listed.contains(&key.to_string()),
            "case {case:?}: the bucket must hold the key {key:?}, got {listed:?}"
        );
    }

    // Each key is then deleted by a batch that spells it in the document verbatim: the entry
    // the result reports names that key, and the key — and only that key — is gone.
    let mut left: Vec<String> = rows.iter().map(|(_, key, _)| (*key).to_string()).collect();
    for (case, key, _) in rows {
        let payload =
            format!("<Delete xmlns=\"{XMLNS}\"><Object><Key>{key}</Key></Object></Delete>");
        let answer = send(&router, "POST", &format!("/{name}?delete"), &[], &payload).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a batch naming {key:?} is served, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let body = String::from_utf8_lossy(&answer.body).into_owned();
        assert!(
            body.contains(&format!("<Deleted><Key>{key}</Key></Deleted>")),
            "case {case:?}: the result must report the key as the document spelled it, got \
             {body:?}"
        );

        left.retain(|held| held != key);
        let mut expected = left.clone();
        expected.sort();
        let after = listing_of(&list_objects_v2(&router, name, "").await).keys();
        assert_eq!(
            after, expected,
            "case {case:?}: the delete must take {key:?} alone away, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_delete_reads_quiet_in_the_published_boolean_space() {
    for (case, spelled, states) in SPELLINGS {
        // A fresh store per row, so one row's delete cannot decide another's.
        let router = router();
        let name = "batch-delete-quiet";
        bucket(&router, name).await;
        let key = "data/one.csv";
        let written = put_object(&router, name, key, b"a,b\n", &[]).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the setup write must be accepted, got {} with body {:?}",
            written.status,
            text(&written)
        );

        let answer = delete_objects(&router, name, key, Some(spelled)).await;
        let Some(quiet) = states else {
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: text in neither spelling states no boolean, so the document is \
                 refused, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            continue;
        };
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a boolean the published space admits is served, got {} with body \
             {:?}",
            answer.status,
            text(&answer)
        );
        // "In quiet mode the response includes only keys where the delete operation
        // encountered an error" (this operation's own documentation), so the value the
        // element states is what decides whether the deleted key is reported.
        let body = String::from_utf8_lossy(&answer.body).into_owned();
        assert_eq!(
            body.contains("<Deleted>"),
            !quiet,
            "case {case:?}: expected Quiet to state {quiet}, so the result must {} report the \
             deleted key, got {body:?}",
            match quiet {
                true => "not",
                false => "",
            }
        );
    }

    // A payload naming no `Quiet` at all states nothing, which is the verbose default the
    // member's absence means — so the rows above are about the element's TEXT and not about
    // its presence.
    let router = router();
    let name = "batch-delete-quiet-absent";
    bucket(&router, name).await;
    put_object(&router, name, "data/one.csv", b"a,b\n", &[]).await;
    let answer = delete_objects(&router, name, "data/one.csv", None).await;
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    assert!(
        answer.status == 200 && body.contains("<Deleted>"),
        "case \"no Quiet element at all\": expected the verbose default, got {} with body \
         {body:?}",
        answer.status
    );
}

#[tokio::test]
async fn a_configuration_write_reads_every_setting_in_the_published_boolean_space() {
    for (case, spelled, states) in SPELLINGS {
        // A fresh store per row: every bucket is created with all four switches on, so a
        // row's own write is the only thing that can have changed them.
        let router = router();
        let name = "public-access-block-spelling";
        bucket(&router, name).await;

        let answer = put_public_access_block_spelled(&router, name, spelled).await;
        let Some(setting) = states else {
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: text in neither spelling states no boolean, so the document is \
                 refused, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            continue;
        };
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a boolean the published space admits is served, got {} with body \
             {:?}",
            answer.status,
            text(&answer)
        );

        // And the value each setting states is what the bucket reports afterwards, read back
        // through the operation that reports the stored configuration.
        let read = get_public_access_block(&router, name, &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the stored configuration must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            reported_flags(&read),
            [Some(setting); 4],
            "case {case:?}: expected every setting to report {setting}, got {:?}",
            reported_flags(&read)
        );
    }
}

#[tokio::test]
async fn a_tag_set_keeps_the_whitespace_the_key_its_document_spells_carries() {
    // `Tag$Key` is the OTHER member the model targets at `ObjectKey` — that shape documents
    // it as "name of the object key" — and it travels in an element of the same name, so the
    // text of a tag key is kept exactly as the document spelled it too. Whitespace is a
    // character a tag key may carry ("letters, whitespace, and numbers representable in
    // UTF-8", the published tag restrictions), so a key stated with it is a key with it.
    let router = router();
    let name = "tag-key-whitespace";
    bucket(&router, name).await;
    let key = "data/one.csv";
    put_object(&router, name, key, b"a,b\n", &[]).await;

    // The rows differ along where the whitespace of a tag key falls, as the delete rows above
    // do for an object key.
    let rows: [(&str, &str); 4] = [
        ("a leading space", " leading"),
        ("a trailing space", "trailing "),
        ("whitespace inside", "two words"),
        ("no whitespace at all", "plain"),
    ];
    for (case, tag_key) in rows {
        let written = put_object_tagging(
            &router,
            name,
            key,
            &[(tag_key.to_string(), "value".to_string())],
        )
        .await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the tag set must be stored, got {} with body {:?}",
            written.status,
            text(&written)
        );
        let read = get_object_tagging(&router, name, key, "").await;
        assert_eq!(
            tag_set_of(&read),
            vec![(tag_key.to_string(), "value".to_string())],
            "case {case:?}: the stored key must be the one the document spelled, got {:?}",
            tag_set_of(&read)
        );
    }
}
