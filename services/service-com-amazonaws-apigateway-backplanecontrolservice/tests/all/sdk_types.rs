// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SDK type collection: `GetSdkTypes` and `GetSdkType`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`] and friends are this crate's one
//! wire-test harness, reused from `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs rather than one recorded call:
//!
//! * the collection is the service's published catalog of generators, so EVERY entry it
//!   publishes is listed, in the catalog's order, with the service's own text for the
//!   entry and for every configuration property, and nothing else is listed — the rows
//!   span both generators, every property of each (those the issue cited none of
//!   included), a property with a default stated in its prose and one without, and a
//!   required as well as an optional property;
//! * every published entry reads back member for member through the single reader, and an
//!   id the catalog does not publish — including the retired generators an older model
//!   sentence still names — is the modelled 404;
//! * a page is the slice `position` and `limit` name, over page sizes across the
//!   documented `1..=500` range, each published bound and one past each, and over every
//!   cursor the sequence holds; a cursor the sequence does not hold is the modelled 400;
//! * both reads are reporting operations: repeating them changes nothing a later caller
//!   sees.

use serde_json::Value;

use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};

/// One expected configuration property of a catalog entry:
/// `(name, friendlyName, description, required)`.
type PropertyRow = (&'static str, &'static str, &'static str, bool);

/// One expected catalog entry: `(id, friendlyName, description, configurationProperties)`.
type CatalogRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static [PropertyRow],
);

/// The SDK types API Gateway publishes a generator for, written out here rather than read
/// from the service, so this test checks the catalog instead of restating it.
///
/// Source: the catalog is reference data the service authors — this service's model
/// declares no shape enumerating the types and constrains neither the set nor the prose
/// (`smithy_query(query="shape", shape="SdkType,SdkTypes")`: `SdkType` is
/// id/friendlyName/description/configurationProperties with no constraint trait, and
/// `SdkTypes` is a plain list) — so what is pinned below is the service's own published
/// text for the two generators it offers, down to each property's description, which a
/// console or generator front-end renders to a user verbatim.
///
/// `GetSdkRequest$sdkType`'s documentation (`smithy_query(query="doc",
/// shape="GetSdkRequest", member="sdkType")`: "Currently `java`, `javascript`, `android`,
/// `objectivec` (for iOS), `swift` (for iOS), and `ruby` are supported") names four more
/// ids, but the catalog holds none of them: that sentence is older than the catalog, and
/// those ids are asserted below to resolve to nothing (see [`RETIRED_SDK_TYPE_IDS`]).
///
/// The implementation's counterpart is `SDK_TYPES` in `src/state.rs`. This table is written
/// out independently of it ON PURPOSE: a published string re-worded on one side has to be
/// re-worded here too, which is the check. Keep the two in step.
const CATALOG: &[CatalogRow] = &[
    (
        "java",
        "Java SDK",
        "Java SDK generator for API Gateway APIs",
        &[
            (
                "service.name",
                "Service Name",
                "Name of the service which is used to derive the Java interface name for your client",
                true,
            ),
            (
                "java.package-name",
                "Java Package Name",
                "Name of the Java package your code will be generated under",
                true,
            ),
            (
                "java.build-system",
                "Java Build System",
                "Build system to setup for project; Currently supported: maven, gradle",
                false,
            ),
            (
                "java.group-id",
                "Java Group Id",
                "Group id for your Maven or Gradle project. Defaults to package name",
                false,
            ),
            (
                "java.artifact-id",
                "Java Artifact Id",
                "Artifact Id for your Maven project or project name for your Gradle project. Defaults to service name",
                false,
            ),
            (
                "java.artifact-version",
                "Java Artifact Version",
                "Version of your Maven or Gradle project. Defaults to 1.0-SNAPSHOT",
                false,
            ),
            (
                "java.license-text",
                "Source Code License Text",
                "Customer provided license to inject into source file headers",
                false,
            ),
        ],
    ),
    (
        "ruby",
        "Ruby",
        "Ruby SDK generator for API Gateway APIs",
        &[
            (
                "service.name",
                "Service Name",
                "Name of the service which is used to derive the name for your client",
                true,
            ),
            (
                "ruby.gem-name",
                "Ruby Gem Name",
                "Name of the Ruby gem your code will be generated under",
                false,
            ),
            (
                "ruby.gem-version",
                "Ruby Gem Version",
                "Version number for your service gem. Defaults to 1.0.0",
                false,
            ),
        ],
    ),
];

/// The generator ids an older `GetSdkRequest$sdkType` sentence still names and the catalog
/// does not publish: the service offers no generator under them, so neither read resolves
/// one.
const RETIRED_SDK_TYPE_IDS: &[&str] = &["javascript", "android", "objectivec", "swift"];

/// Every catalog id, in the catalog's order: the sequence a page is cut out of.
fn catalog_ids() -> Vec<&'static str> {
    CATALOG.iter().map(|(id, ..)| *id).collect()
}

/// `GetSdkTypes` with the query string `query` (empty for none).
async fn get_sdk_types(gateway: &Gateway, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/sdktypes".to_string()
    } else {
        format!("/sdktypes?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetSdkType` for `id`.
async fn get_sdk_type(gateway: &Gateway, id: &str) -> Answer {
    gateway.call("GET", &format!("/sdktypes/{id}"), None).await
}

/// The ids of a listed page, in the order the page reports them.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// Assert one listed or read SDK type is the catalog row `expected`, member for member.
fn assert_catalog_entry(entry: &Value, expected: &CatalogRow, case: &str) {
    let (id, friendly_name, description, properties) = expected;
    assert_eq!(
        entry["id"].as_str(),
        Some(*id),
        "case {case}: expected the id {id:?}, got {entry:?}"
    );
    // An entry is the GENERATOR its id names, spelled as the service publishes it: the
    // friendly name is the label the generator is picked by and the description names the
    // language it generates a client for. Both are rendered to a user verbatim, so both
    // are pinned rather than matched by form.
    assert_eq!(
        entry["friendlyName"].as_str(),
        Some(*friendly_name),
        "case {case}: expected the friendly name {friendly_name:?}, got {entry:?}"
    );
    assert_eq!(
        entry["description"].as_str(),
        Some(*description),
        "case {case}: expected the description {description:?}, got {entry:?}"
    );

    // `configurationProperties` is always reported: it says what a `GetSdk` call may set
    // for this generator, each property with the service's own label and sentence.
    let reported = entry["configurationProperties"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected a configurationProperties array, got {entry:?}")
        });
    assert_eq!(
        reported.len(),
        properties.len(),
        "case {case}: expected {} configuration properties, got {reported:?}",
        properties.len()
    );
    for (property, (name, property_friendly_name, property_description, required)) in
        reported.iter().zip(properties.iter())
    {
        assert_eq!(
            property["name"].as_str(),
            Some(*name),
            "case {case}: expected the configuration property {name:?}, got {property:?}"
        );
        assert_eq!(
            property["required"].as_bool(),
            Some(*required),
            "case {case}: expected required={required} on {name:?}, got {property:?}"
        );
        assert_eq!(
            property["friendlyName"].as_str(),
            Some(*property_friendly_name),
            "case {case}: expected the friendly name {property_friendly_name:?} on {name:?}, \
             got {property:?}"
        );
        assert_eq!(
            property["description"].as_str(),
            Some(*property_description),
            "case {case}: expected the description {property_description:?} on {name:?}, \
             got {property:?}"
        );
    }
}

#[tokio::test]
async fn every_sdk_type_of_the_catalog_is_listed_and_reads_back_through_get_sdk_type() {
    let gateway = Gateway::new();

    // The whole collection fits in one page: the documented default page size is 25 and
    // the catalog is smaller, so a request carrying no `limit` lists all of it.
    let all = get_sdk_types(&gateway, "").await;
    assert_eq!(
        all.status, 200,
        "case GetSdkTypes(no limit): expected 200, got {} with {:?}",
        all.status, all.body
    );
    let items = page_items(&all.body, "GetSdkTypes(no limit)");
    assert_eq!(
        items.len(),
        CATALOG.len(),
        "case GetSdkTypes(no limit): expected the whole catalog of {} types, got {:?}",
        CATALOG.len(),
        all.body
    );
    assert_eq!(
        listed_ids(&all.body, "GetSdkTypes(no limit)"),
        catalog_ids(),
        "case GetSdkTypes(no limit): expected the catalog's order, got {:?}",
        all.body
    );

    for (row, entry) in CATALOG.iter().zip(items.iter()) {
        let (id, ..) = row;
        let case = format!("GetSdkTypes entry {id}");
        assert_catalog_entry(entry, row, &case);

        // Round trip: the single reader reports the listed record, member for member.
        let read = get_sdk_type(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case GetSdkType({id}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_catalog_entry(&read.body, row, &format!("GetSdkType({id})"));
        assert_eq!(
            &read.body, entry,
            "case GetSdkType({id}): expected the listed record {entry:?}, got {:?}",
            read.body
        );
    }

    // Reading the collection does not change what a later caller sees: the listing and
    // the single reads above leave the same catalog behind.
    let again = get_sdk_types(&gateway, "").await;
    assert_eq!(
        again.body, all.body,
        "case GetSdkTypes repeated: a read must not change the collection, got {:?} after {:?}",
        again.body, all.body
    );
}

#[tokio::test]
async fn a_page_of_sdk_types_is_the_slice_the_position_and_limit_name() {
    let gateway = Gateway::new();
    let ids = catalog_ids();

    // Rows span the documented page-size range (default 25, maximum 500), each published
    // bound and one past each — a size below 1 and a size above 500 are clamped into the
    // range rather than refused — crossed with every cursor the sequence holds, which
    // includes no cursor at all and the last id.
    let limits = [0, 1, 2, 3, 6, 7, 25, 500, 501, 10_000];
    for limit in limits {
        let effective =
            usize::try_from(limit.clamp(1, 500)).expect("the clamped limit is positive");
        for (index, cursor) in std::iter::once(None)
            .chain(ids.iter().map(|id| Some(*id)))
            .enumerate()
        {
            // No cursor starts at the first id; a cursor is the id of the first entry
            // the previous page did NOT hand out, so its page resumes AT that entry.
            let start = index.saturating_sub(1);
            let expected: Vec<&str> = ids.iter().skip(start).take(effective).copied().collect();
            let query = match cursor {
                None => format!("limit={limit}"),
                Some(id) => format!("limit={limit}&position={id}"),
            };
            let case = format!("GetSdkTypes({query})");
            let answer = get_sdk_types(&gateway, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_ids(&answer.body, &case),
                expected,
                "case {case}: expected the page {expected:?}, got {:?}",
                answer.body
            );
        }
    }

    // The collection carries no cursor member of its own: the modelled `SdkTypes` output
    // declares `items` alone, so a page reports no `position` even when more types follow
    // it.
    let first = get_sdk_types(&gateway, "limit=1").await;
    assert!(
        first.body["position"].is_null(),
        "case GetSdkTypes(limit=1): this collection reports no position, got {:?}",
        first.body["position"]
    );

    // Walking the catalog one page at a time with the caller's own cursor reassembles the
    // catalog's order from each cursor on, with no entry listed twice: a page resumes AT
    // the cursor it is given.
    let mut seen: Vec<String> = Vec::new();
    for chunk in ids.chunks(2) {
        let query = format!("limit=2&position={}", chunk[0]);
        let case = format!("GetSdkTypes({query})");
        let answer = get_sdk_types(&gateway, &query).await;
        seen.extend(listed_ids(&answer.body, &case));
    }
    let expected: Vec<String> = ids
        .chunks(2)
        .flat_map(|chunk| {
            let at = ids
                .iter()
                .position(|id| *id == chunk[0])
                .expect("the chunk's first id is the catalog's");
            ids.iter().skip(at).take(2).map(|id| (*id).to_string())
        })
        .collect();
    assert_eq!(
        seen, expected,
        "case GetSdkTypes paged in twos: expected {expected:?}, got {seen:?}"
    );
}

#[tokio::test]
async fn an_id_the_catalog_does_not_hold_is_a_not_found_and_an_unknown_cursor_is_a_bad_request() {
    let gateway = Gateway::new();

    // The ways an id can miss the catalog: a generator the service no longer publishes
    // (every retired id), a published id in the wrong case, one with a trailing
    // character, a plausible other spelling of a published id, a language the service
    // never generated, and an id shaped like one of this service's minted resource ids.
    let unknown: Vec<&str> = RETIRED_SDK_TYPE_IDS
        .iter()
        .copied()
        .chain([
            "JAVA",
            "java-",
            "ios-swift",
            "typescript",
            "zzzzzzzzzz",
            "objective-c",
            "Ruby",
        ])
        .collect();
    for id in unknown {
        let case = format!("GetSdkType({id})");
        let answer = get_sdk_type(&gateway, id).await;
        assert_failure(&answer, 404, "NotFoundException", &case);
        // Fixed text naming the generator the service has none of, with the id taken from
        // the request's own path label.
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!("No SDK generator for '{id}'"),
            "case {case}: expected the generator refusal naming {id:?}, got {message:?}"
        );
    }

    // A retired generator is not listed either: a client enumerating the catalog to
    // decide which SDKs it may request sees only the generators the service publishes.
    let listed = get_sdk_types(&gateway, "limit=500").await;
    let listed_now = listed_ids(&listed.body, "GetSdkTypes(limit=500)");
    for id in RETIRED_SDK_TYPE_IDS {
        assert!(
            !listed_now.iter().any(|listed_id| listed_id == id),
            "case GetSdkTypes(limit=500): expected no entry for the retired {id:?}, got \
             {listed_now:?}"
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page — in
    // the ways a cursor can miss: an unknown word, a published id in the wrong case, a
    // published id with a trailing character, and a retired generator's id.
    for cursor in ["nosuchcursor", "JAVA", "java-", "javascript"] {
        let query = format!("position={cursor}");
        assert_failure(
            &get_sdk_types(&gateway, &query).await,
            400,
            "BadRequestException",
            &format!("GetSdkTypes({query})"),
        );
    }

    // None of the refusals changed the collection.
    let listed = get_sdk_types(&gateway, "").await;
    assert_eq!(
        listed_ids(&listed.body, "GetSdkTypes after the refusals"),
        catalog_ids(),
        "case GetSdkTypes after the refusals: expected the catalog intact, got {:?}",
        listed.body
    );
}
