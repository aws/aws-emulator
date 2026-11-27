// SPDX-License-Identifier: CC0-1.0

//! Retiring a place index resource, end to end over the wire.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: a place index that stops
//! answering after the delete stopped answering because it left PERSISTED state,
//! not because a handler branched on this request.
//!
//! RULE under test: a delete of a named control-plane resource retires it
//! permanently, so every later call that resolves that name — the resource's own
//! read, the reads derived from it, the tag plane, and a second delete — resolves
//! nothing and is refused with `ResourceNotFoundException` (HTTP 404, the
//! `x-amzn-errortype` header naming the shape), while the resources the delete did
//! not name keep everything they held and the retired name is free to create
//! again.
//!
//! The table spans what the rule generalises over: the punctuation and the lengths
//! `ResourceName` admits (`@length 1..100`, `^[-._\w]+$`, so a single character and
//! the 100-character bound are both rows), the whole documented provider set a
//! place index can be created against (the `CreatePlaceIndexRequest$DataSource`
//! doc: "Valid values include: Esri … Grab … Here"), and whether the retired index
//! held tags. The index the issue report replays is one row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, encode, millis_of, Answer, ACCOUNT, REGION};

/// One row: a place index to retire, and the index kept beside it.
struct Case {
    case: &'static str,
    /// The name the row deletes. `String` because one row is built to the
    /// published length bound.
    name: String,
    /// The geospatial data provider the retired index geocodes against.
    data_source: &'static str,
    /// The tags it was created with.
    tags: &'static [(&'static str, &'static str)],
}

/// The table. Each row differs from the others along the name's shape, the data
/// provider the index was created against, and the tag map it held.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "an Esri index with no tags",
            name: "retired".to_string(),
            data_source: "Esri",
            tags: &[],
        },
        Case {
            case: "a tagged Here index whose name carries every admitted punctuation",
            name: "A_b-c.1".to_string(),
            data_source: "Here",
            tags: &[("env", "staging"), ("team", "places")],
        },
        Case {
            case: "a Grab index named by a single character",
            // `ResourceName` is `@length 1..100`: the shallow bound itself.
            name: "x".to_string(),
            data_source: "Grab",
            tags: &[("owner", "places")],
        },
        Case {
            case: "an Esri index named at the longest admitted length",
            // `ResourceName` is `@length 1..100`: the published maximum.
            name: "i".repeat(100),
            data_source: "Esri",
            tags: &[],
        },
    ]
}

/// The place index kept beside the retired one in every row, so a delete is shown
/// to retire the index it names and nothing else.
const BYSTANDER: &str = "kept_index.2";
const BYSTANDER_PROVIDER: &str = "Here";
const BYSTANDER_TAG: (&str, &str) = ("generation", "first");

/// The query every row geocodes, which the provider dataset answers with at least
/// one place: the search is the positive control that the index served reads at
/// all before the delete.
const QUERY: &str = "Space Needle";

/// The status, the wire error CODE and the whole message the shared tagging frontend
/// in front of the ARN-addressed tag operations answers for an ARN that resolves to no
/// resource: its own `NotFoundException` and one fixed sentence, rather than the
/// modelled `ResourceNotFoundException` and the index-naming sentence a call that named
/// the index in its own URI earns.
const TAG_PLANE_NOT_FOUND_CODE: &str = "NotFoundException";
const TAG_PLANE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

fn tags_body(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

async fn create_index(
    router: &MultiServiceRouter,
    name: &str,
    data_source: &str,
    tags: &[(&str, &str)],
) {
    let mut body = json!({ "IndexName": name, "DataSource": data_source });
    if !tags.is_empty() {
        body["Tags"] = tags_body(tags);
    }
    let created = call(router, "POST", "/places/v0/indexes", Some(body)).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}/{data_source:?}: CreatePlaceIndex must succeed, got {} {:?}",
        created.status, created.body
    );
}

async fn describe_index(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/places/v0/indexes/{name}"), None).await
}

async fn delete_index(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/places/v0/indexes/{name}"),
        None,
    )
    .await
}

async fn search_text(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/places/v0/indexes/{name}/search/text"),
        Some(json!({ "Text": QUERY })),
    )
    .await
}

async fn get_place(router: &MultiServiceRouter, name: &str, place_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/places/v0/indexes/{name}/places/{}", encode(place_id)),
        None,
    )
    .await
}

/// The first `PlaceId` the text search reports, when the index's provider mints
/// one on a text search at all.
///
/// The `SearchForTextResult$PlaceId` doc: "For `SearchPlaceIndexForText`
/// operations, the `PlaceId` is returned only by place indexes that use HERE or
/// Grab as a data provider." — so an Esri index answers results with no
/// identifier, and a row against one reads a place by an identifier of its own
/// making instead.
fn first_place_id(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("Results")?
        .as_array()?
        .iter()
        .find_map(|result| result.get("PlaceId")?.as_str().map(str::to_string))
}

/// One 404: the status, the header, the wire code and the sentence the caller expects.
fn assert_refused_as_missing(answer: &Answer, code: &str, message: &str, case: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_type.is_some(),
        "case {case:?}: the refusal must carry the x-amzn-errortype header, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected error code {code}, got {:?} ({:?})",
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        answer.message()
    );
}

/// A refusal's status, wire error code, `x-amzn-errortype` header and message, for
/// a call that named the place index in its own URI: the MODELLED
/// `ResourceNotFoundException`.
fn assert_not_found(answer: &Answer, name: &str, case: &str) {
    assert_refused_as_missing(
        answer,
        "ResourceNotFoundException",
        &format!("Place index not found: {name}."),
        case,
    );
}

/// The same class from the ARN-addressed tag plane, which the shared tagging
/// frontend serves: one route over every taggable resource, so it answers that
/// frontend's own error code and its single fixed sentence instead of naming the
/// index.
fn assert_tag_plane_not_found(answer: &Answer, case: &str) {
    assert_refused_as_missing(
        answer,
        TAG_PLANE_NOT_FOUND_CODE,
        TAG_PLANE_NOT_FOUND_MESSAGE,
        case,
    );
}

/// The `Tags` member of a response, sorted, or `None` when the member is absent.
fn reported_tags(body: &Value) -> Option<Vec<(String, String)>> {
    let mut tags: Vec<(String, String)> = body
        .get("Tags")?
        .as_object()?
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect();
    tags.sort();
    Some(tags)
}

#[tokio::test]
async fn a_delete_retires_the_place_index_so_every_later_read_of_the_name_is_refused() {
    for row in cases() {
        let router = emulator();
        let name = row.name.as_str();
        create_index(&router, name, row.data_source, row.tags).await;
        create_index(&router, BYSTANDER, BYSTANDER_PROVIDER, &[BYSTANDER_TAG]).await;

        // The positive control: before the delete, the index's own read and the
        // search derived from its stored provider are both served.
        let described = describe_index(&router, name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: the index must be readable before the delete, got {} {:?}",
            row.case, described.status, described.body
        );
        let searched = search_text(&router, name).await;
        assert_eq!(
            searched.status, 200,
            "case {:?}: the index must geocode before the delete, got {} {:?}",
            row.case, searched.status, searched.body
        );
        // An identifier the index's provider mints, so the place read below is
        // about a place this index could resolve while it existed. A provider that
        // mints none on a text search is read by an identifier of this test's own
        // making: whichever it is, resolving the INDEX comes first.
        let place_id = first_place_id(&searched).unwrap_or_else(|| "unminted-place".to_string());

        let deleted = delete_index(&router, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: DeletePlaceIndex must succeed, got {} {:?}",
            row.case, deleted.status, deleted.body
        );

        // Every way back into the retired index is refused: its own read, the
        // reads derived from its stored provider, and a second delete of the name.
        assert_not_found(
            &describe_index(&router, name).await,
            name,
            &format!("{}: DescribePlaceIndex after delete", row.case),
        );
        assert_not_found(
            &search_text(&router, name).await,
            name,
            &format!("{}: SearchPlaceIndexForText after delete", row.case),
        );
        assert_not_found(
            &get_place(&router, name, &place_id).await,
            name,
            &format!("{}: GetPlace after delete", row.case),
        );
        assert_not_found(
            &delete_index(&router, name).await,
            name,
            &format!("{}: delete after delete", row.case),
        );

        // The tag plane resolves the same stored index, so the retired index's ARN
        // names nothing there either.
        let arn = format!("arn:aws:geo:{REGION}:{ACCOUNT}:place-index/{name}");
        let encoded_arn = encode(&arn);
        assert_tag_plane_not_found(
            &call(&router, "GET", &format!("/tags/{encoded_arn}"), None).await,
            &format!("{}: ListTagsForResource after delete", row.case),
        );
        assert_tag_plane_not_found(
            &call(
                &router,
                "POST",
                &format!("/tags/{encoded_arn}"),
                Some(json!({ "Tags": { "late": "tag" } })),
            )
            .await,
            &format!("{}: TagResource after delete", row.case),
        );

        // The index the delete did not name keeps everything it held.
        let kept = describe_index(&router, BYSTANDER).await;
        assert_eq!(
            kept.status, 200,
            "case {:?}: an index the delete did not name must still be readable, got {} {:?}",
            row.case, kept.status, kept.body
        );
        assert_eq!(
            reported_tags(&kept.json()),
            Some(vec![(
                BYSTANDER_TAG.0.to_string(),
                BYSTANDER_TAG.1.to_string()
            )]),
            "case {:?}: an index the delete did not name must keep its tags, got {:?}",
            row.case,
            kept.json().get("Tags")
        );
    }
}

#[tokio::test]
async fn deleting_a_name_no_place_index_is_stored_under_is_refused_and_retires_nothing() {
    let router = emulator();
    // One index exists, so the refusals below are about the requested name rather
    // than about an empty store.
    create_index(&router, "Exists", "Esri", &[]).await;

    // Names that satisfy `ResourceName`'s `^[-._\w]+$` but name no index,
    // including case variants of the one that does exist: the lookup is by exact
    // name.
    for name in ["missing", "exists", "EXISTS", "Exists.2", "x", "a_b-c.1"] {
        assert_not_found(
            &delete_index(&router, name).await,
            name,
            &format!("delete {name:?}"),
        );
    }

    // A refused delete retires nothing.
    let still_there = describe_index(&router, "Exists").await;
    assert_eq!(
        still_there.status, 200,
        "a refused delete must leave the stored index readable, got {} {:?}",
        still_there.status, still_there.body
    );
}

#[tokio::test]
async fn a_retired_place_index_name_is_free_again_and_reads_back_as_the_new_resource() {
    let router = emulator();
    create_index(&router, "recycled", "Esri", &[("generation", "first")]).await;
    let first = describe_index(&router, "recycled").await.json();

    let deleted = delete_index(&router, "recycled").await;
    assert_eq!(
        deleted.status, 200,
        "DeletePlaceIndex must succeed, got {} {:?}",
        deleted.status, deleted.body
    );

    // The name is free: the create is accepted rather than refused as a conflict.
    create_index(&router, "recycled", "Grab", &[]).await;

    let again = describe_index(&router, "recycled").await;
    assert_eq!(
        again.status, 200,
        "the re-created index must be readable, got {} {:?}",
        again.status, again.body
    );
    let again = again.json();
    // What the name reads back is the NEW index: nothing the retired one held
    // survived the delete.
    assert_eq!(
        again.get("DataSource").and_then(Value::as_str),
        Some("Grab"),
        "the re-created index must report its own DataSource, got {again}"
    );
    assert_eq!(
        reported_tags(&again),
        Some(Vec::new()),
        "the re-created index must hold none of the retired one's tags, got {:?}",
        again.get("Tags")
    );
    assert!(
        millis_of(&again, "CreateTime") >= millis_of(&first, "CreateTime"),
        "the re-created index's CreateTime must be its own, got {:?} against the retired index's {:?}",
        again.get("CreateTime"),
        first.get("CreateTime")
    );
}
