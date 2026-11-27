// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the whole contract of a `CreateCatalog` whose `Name` is already
//! taken: the sentence the refusal carries, that the rejected create writes nothing, and
//! that the identical create goes through once the name is free again.
//!
//! The rule under test: a refusal carries the sentence the service words THAT condition on
//! THAT resource surface with, reproduced character for character — its terminal
//! punctuation included — and interpolates nothing the sentence does not name, so each
//! already-exists surface keeps its own wording and none of them is derived from another's.
//! Every one of them names its resource kind and withholds the requested name, and each closes
//! on the character its OWN sentence closes on — the catalog's on the last letter of its last
//! word, the four sibling surfaces' on a full stop; the suite below drives five surfaces beside
//! each other so the wording is pinned per surface rather than blanket-derived.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! requested name's value, length, casing and character set, and how the second create
//! differs from the first (identically, with another definition, with other tags, in
//! another casing). The expected strings are written out here as literals rather than read
//! from the service's own constants, so the suite pins the wording instead of restating it.
//! Every call goes over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::{assert_refusal, assert_withholds};
use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The modelled error a create for a name already taken answers ("a resource to be
/// created or added already exists", `smithy_query operation=CreateCatalog`).
const ALREADY_EXISTS: &str = "AlreadyExistsException";

/// The whole `Message` a `CreateCatalog` for a name the account already holds answers:
/// fixed text, closing on the last letter of its last word.
const CATALOG_ALREADY_EXISTS: &str = "Catalog already exists";

/// The whole `Message` the catalog read and delete answer for an id holding no catalog.
const CATALOG_NOT_FOUND: &str = "Catalog not found.";

/// A `CreateCatalog` body for `name` carrying `description`, naming a source outside the
/// Glue Data Catalog — the only catalog this operation registers.
///
/// Shared with the suite that pins the terminal punctuation of each catalog-surface
/// sentence ([`crate::catalog_refusal_terminal_punctuation`]), so both drive the create
/// with one request shape.
pub(crate) fn create_body(name: &str, description: &str) -> Value {
    json!({
        "Name": name,
        "CatalogInput": {
            "Description": description,
            "FederatedCatalog": {
                "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
                "ConnectionName": "aws:s3tables",
            },
        },
    })
}

/// `(row, Name)` pairs spanning the axis the rule generalises over: the name's value,
/// length, casing and punctuation. Every spelling stays inside `CatalogNameString`
/// ("len 1..64", pattern `^(?!(.*[.\/\\]|aws:)).*$` —
/// `smithy_query operation=CreateCatalog`), so no row is refused at the request boundary
/// instead of by the handler. The first row is the shape of the recorded behavioural case
/// and the last sits at the modelled ceiling.
///
/// Shared with [`crate::catalog_refusal_terminal_punctuation`], so the conditions of the
/// catalog surface are driven over one name axis.
pub(crate) fn catalog_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "recorded behavioural shape",
            "sim2real-probe-catalog-9a546660".to_string(),
        ),
        ("a single character", "k".to_string()),
        ("mixed case with punctuation", "Lake_Catalog-Two2026".to_string()),
        ("at the 64 ceiling", "k".repeat(64)),
    ]
}

/// The `Description` a `GetCatalog` for `catalog_id` reports.
async fn description_of(router: &MultiServiceRouter, catalog_id: &str) -> Option<String> {
    let read = ok(router, "GetCatalog", &json!({ "CatalogId": catalog_id })).await;
    read["Catalog"]["Description"]
        .as_str()
        .map(str::to_string)
}

/// A second `CreateCatalog` for a name already taken is refused with the catalog surface's
/// own fixed sentence — which names nothing of the request —
/// whatever the name is spelled like and however the second create differs from the first,
/// and the stored catalog keeps the definition the first create wrote.
#[tokio::test]
async fn a_duplicate_catalog_name_answers_the_catalog_sentence_character_for_character() {
    for (row, name) in catalog_names() {
        // A fresh emulator per row, so a name at the modelled ceiling can be taken in
        // every row without the store filling up across rows.
        let router = emulator();
        ok(&router, "CreateCatalog", &create_body(&name, "the first definition")).await;

        let mut other_tags = create_body(&name, "the first definition");
        other_tags["Tags"] = json!({ "attempt": "second", "owner": "platform" });
        let repeats = [
            ("the identical request", create_body(&name, "the first definition")),
            ("another definition", create_body(&name, "a second definition")),
            ("other tags on the same definition", other_tags),
            (
                "the name in another casing",
                create_body(&name.to_uppercase(), "a third definition"),
            ),
        ];

        for (repeat, body) in repeats {
            let case = format!("{row} / {repeat}");
            let answered = assert_refusal(
                &router,
                &case,
                "CreateCatalog",
                &body,
                ALREADY_EXISTS,
                CATALOG_ALREADY_EXISTS,
            )
            .await;
            let message = error_message(&answered);
            // The sentence is reproduced character for character, terminal punctuation
            // included, so a client matching the catalog surface's text matches.
            assert_eq!(
                message, CATALOG_ALREADY_EXISTS,
                "case {case}: the catalog duplicate sentence is reproduced whole, got \
                 {message:?}"
            );
            assert_withholds(&case, &message, &name);
        }

        // The refused creates wrote nothing: the stored catalog is still the first
        // create's, not any of the definitions the repeats carried.
        let stored = description_of(&router, &name).await;
        assert_eq!(
            stored.as_deref(),
            Some("the first definition"),
            "case {row}: a refused create must not replace the stored definition, got {stored:?}"
        );
    }
}

/// The refusal is about the NAME being taken and nothing else: once the stored catalog is
/// deleted the id is a miss — worded with the catalog surface's own miss sentence — and the
/// create that was refused a moment ago goes through and is readable with the definition it
/// carried.
///
/// The rows differ along the name's spelling, so the lifecycle is driven over the same axis
/// as the refusal above.
#[tokio::test]
async fn the_identical_create_goes_through_once_the_name_conflict_is_lifted() {
    for (row, name) in catalog_names() {
        let router = emulator();
        ok(&router, "CreateCatalog", &create_body(&name, "the first definition")).await;

        let refused = create_body(&name, "the definition that was refused");
        assert_refusal(
            &router,
            &format!("{row} / before the delete"),
            "CreateCatalog",
            &refused,
            ALREADY_EXISTS,
            CATALOG_ALREADY_EXISTS,
        )
        .await;

        ok(&router, "DeleteCatalog", &json!({ "CatalogId": &name })).await;

        let (status, body) = call(&router, "GetCatalog", &json!({ "CatalogId": &name })).await;
        assert_eq!(
            status, 400,
            "case {row}: the deleted catalog must be a miss, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {row}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            CATALOG_NOT_FOUND,
            "case {row}: expected the catalog miss sentence, got {body}"
        );

        // The create refused above is now the create of a free name.
        ok(&router, "CreateCatalog", &refused).await;
        let stored = description_of(&router, &name).await;
        assert_eq!(
            stored.as_deref(),
            Some("the definition that was refused"),
            "case {row}: the re-create must be served from its own definition, got {stored:?}"
        );
    }
}

/// One row of the per-surface wording table: the create that takes a name and the whole
/// sentence ITS duplicate answers.
struct Surface {
    case: &'static str,
    operation: &'static str,
    body: fn(&str) -> Value,
    message: &'static str,
}

fn database_body(name: &str) -> Value {
    json!({ "DatabaseInput": { "Name": name } })
}

fn connection_body(name: &str) -> Value {
    json!({
        "ConnectionInput": {
            "Name": name,
            "ConnectionType": "JDBC",
            "ConnectionProperties": {
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
            },
        },
    })
}

/// A minimal `CreateCrawler` body: the three required members, naming one S3 crawl target.
fn crawler_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": format!("arn:aws:iam::{ACCOUNT_ID}:role/service-role/AWSGlueServiceRole-test"),
        "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
    })
}

fn registry_body(name: &str) -> Value {
    json!({ "RegistryName": name })
}

/// The bound of the rule: a duplicate name is worded by the surface it was asked of, so each
/// of these sentences names its own resource kind — the catalog's, the database's, the
/// connection's, the crawler's and the registry's — on the same request and the same error
/// code, and each closes on the character its OWN sentence closes on: the catalog's on the
/// last letter of its last word, the other four on a full stop. The closing character is a
/// property of the one condition that is worded and is never normalised across the surfaces.
///
/// The five surfaces are the axis this suite adds to the name table above: a wording that has
/// lost or gained its closing character on any one of them fails here.
#[tokio::test]
async fn each_already_exists_surface_keeps_its_own_sentence() {
    let surfaces = [
        Surface {
            case: "catalog",
            operation: "CreateCatalog",
            body: |name| create_body(name, "the stored definition"),
            message: CATALOG_ALREADY_EXISTS,
        },
        Surface {
            case: "database",
            operation: "CreateDatabase",
            body: database_body,
            message: "Database already exists.",
        },
        Surface {
            case: "connection",
            operation: "CreateConnection",
            body: connection_body,
            message: "Connection already exists.",
        },
        Surface {
            case: "crawler",
            operation: "CreateCrawler",
            body: crawler_body,
            message: "Crawler already exists.",
        },
        Surface {
            case: "registry",
            operation: "CreateRegistry",
            body: registry_body,
            message: "Registry already exists.",
        },
    ];

    // No two surfaces may be served one sentence: each names its own resource kind.
    for (at, surface) in surfaces.iter().enumerate() {
        assert!(
            !surfaces
                .iter()
                .skip(at + 1)
                .any(|other| other.message == surface.message),
            "case {:?}: expected a sentence of its own, got the one another surface answers: \
             {:?}",
            surface.case,
            surface.message
        );
    }

    for surface in surfaces {
        for (row, name) in catalog_names() {
            let router = emulator();
            let case = format!("{} / {row}", surface.case);
            ok(&router, surface.operation, &(surface.body)(&name)).await;
            let answered = assert_refusal(
                &router,
                &case,
                surface.operation,
                &(surface.body)(&name),
                ALREADY_EXISTS,
                surface.message,
            )
            .await;
            let message = error_message(&answered);
            // The whole sentence is compared above, so the character it CLOSES on is pinned by
            // that comparison: this surface's own, and not one shared across the surfaces.
            assert_withholds(&case, &message, &name);
        }
    }
}
