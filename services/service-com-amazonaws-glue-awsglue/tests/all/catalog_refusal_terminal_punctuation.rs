// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the exact TEXT of each CONDITION the catalog surface refuses, down
//! to the character its sentence ends on.
//!
//! The rule under test: a refusal carries the sentence the service words THAT condition on
//! THAT resource surface with, reproduced character for character — its terminal
//! punctuation included — so the punctuation is a property of the single condition and is
//! never normalised across the conditions of one surface. The catalog surface is where that
//! is visible in one table: its refusal of a name outside the character class a catalog name
//! is built from and its duplicate-name sentence end on the last letter of their last word,
//! while its own miss sentence and one of its other input refusals end on a full stop.
//!
//! This suite's axis is the CONDITION, which is what no other suite of this crate spans:
//! the duplicate-name sentence beside the catalog miss asked by each of the three operations
//! that resolve a catalog and beside the create's own input refusals. The duplicate-name
//! contract itself — the repeat forms, the effect-absence, the re-create once the name is
//! free, and the sibling already-exists surfaces — is pinned by
//! [`crate::catalog_duplicate_create_wording`], whose request shape and name table this
//! suite reuses rather than copying. Each row still differs along what the rule generalises
//! over: the requested name's value, length, casing and character set. The expected strings
//! are written out here as literals rather than read from the service's own constants, so the
//! suite pins the wording instead of restating it. Every call goes over the wire through the
//! shared [`crate::wire`] harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::catalog_duplicate_create_wording::{catalog_names, create_body};
use crate::error_messages::{assert_refusal, assert_withholds};
use crate::wire::{emulator, error_message, ok};

/// The modelled error a create for a name already taken answers ("a resource to be created
/// or added already exists", `smithy_query operation=CreateCatalog`).
const ALREADY_EXISTS: &str = "AlreadyExistsException";

/// The modelled error a read, write or delete addressing no stored catalog answers
/// (`smithy_query operation=GetCatalog,DeleteCatalog,UpdateCatalog`).
const ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

/// The modelled error an input the catalog surface refuses answers
/// (`smithy_query operation=CreateCatalog`).
const INVALID_INPUT: &str = "InvalidInputException";

/// The whole `Message` a `CreateCatalog` for a name the account already holds answers: fixed
/// text that names the resource kind, withholds the requested name, and closes on the last
/// letter of its last word — this condition's own punctuation, which the sibling
/// already-exists surfaces of this service do not share.
const CATALOG_ALREADY_EXISTS: &str = "Catalog already exists";

/// The whole `Message` the catalog read, update and delete answer for an id holding no
/// catalog: the same surface, period-terminated too.
const CATALOG_NOT_FOUND: &str = "Catalog not found.";

/// The whole `Message` a `CreateCatalog` whose `Name` carries a character the catalog
/// surface forbids answers: the character class a name may be built from, with no terminal
/// full stop.
const CATALOG_NAME_NOT_ALLOWED: &str =
    "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name";

/// The whole `Message` a `CreateCatalog` whose input names no source outside the Glue Data
/// Catalog answers: period-terminated, on the same operation as the duplicate sentence.
const NATIVE_CATALOG_UNSUPPORTED: &str = "Create glue native catalog is not supported.";

/// The length a catalog's `Name` may not exceed (`CatalogNameString` "len 1..64",
/// `smithy_query operation=CreateCatalog`).
const CATALOG_NAME_CEILING: usize = 64;

/// A name derived from `name` by appending `suffix`, kept inside the modelled ceiling
/// ([`CATALOG_NAME_CEILING`]) by trimming the stem, so a row whose name already sits at that
/// ceiling reaches the handler condition under test instead of the length constraint the
/// request boundary applies first.
fn derived_name(name: &str, suffix: &str) -> String {
    let room = CATALOG_NAME_CEILING - suffix.len();
    format!("{}{suffix}", &name[..name.len().min(room)])
}

/// One row of the catalog-surface wording table: a condition, the operation that asks it,
/// the request that reaches it, and the whole sentence it is refused with.
struct Condition {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
    code: &'static str,
    message: &'static str,
}

/// The conditions of the catalog surface, each with the sentence the service words IT with:
/// the duplicate name, the surface's own miss asked by each of the three operations that
/// resolve a catalog, and the two input refusals of the create itself — the table whose rows
/// end on different characters (the duplicate-name and character-class refusals on their last
/// word, the rest on a full stop).
fn conditions() -> Vec<Condition> {
    vec![
        Condition {
            case: "a name already taken",
            operation: "CreateCatalog",
            request: |name| create_body(name, "a second definition"),
            code: ALREADY_EXISTS,
            message: CATALOG_ALREADY_EXISTS,
        },
        Condition {
            case: "a read of an id holding no catalog",
            operation: "GetCatalog",
            request: |name| json!({ "CatalogId": format!("{name}-absent") }),
            code: ENTITY_NOT_FOUND,
            message: CATALOG_NOT_FOUND,
        },
        Condition {
            case: "a delete of an id holding no catalog",
            operation: "DeleteCatalog",
            request: |name| json!({ "CatalogId": format!("{name}-absent") }),
            code: ENTITY_NOT_FOUND,
            message: CATALOG_NOT_FOUND,
        },
        Condition {
            case: "an update of an id holding no catalog",
            operation: "UpdateCatalog",
            request: |name| {
                json!({
                    "CatalogId": format!("{name}-absent"),
                    "CatalogInput": { "Description": "a definition for no catalog" },
                })
            },
            code: ENTITY_NOT_FOUND,
            message: CATALOG_NOT_FOUND,
        },
        Condition {
            case: "a name outside the catalog character class",
            operation: "CreateCatalog",
            // A dot is one of the separators a catalog name may carry nowhere in it.
            request: |name| create_body(&derived_name(name, ".suffix"), "a refused definition"),
            code: INVALID_INPUT,
            message: CATALOG_NAME_NOT_ALLOWED,
        },
        Condition {
            case: "an input naming no source outside the Data Catalog",
            operation: "CreateCatalog",
            request: |name| {
                json!({
                    "Name": derived_name(name, "-native"),
                    "CatalogInput": { "Description": "a native catalog" },
                })
            },
            code: INVALID_INPUT,
            message: NATIVE_CATALOG_UNSUPPORTED,
        },
    ]
}

/// Store the catalog under `name`, so the conditions below are refused about a catalog that
/// exists rather than about an empty Data Catalog.
async fn given_catalog(router: &MultiServiceRouter, name: &str) {
    ok(router, "CreateCatalog", &create_body(name, "the first definition")).await;
}

/// Every refusal of the catalog surface carries the sentence for its OWN condition, whole
/// and character for character — the duplicate-name and character-class sentences ending on
/// their last word beside the miss sentence ending on a full stop — whatever the requested
/// name is spelled like, and none of the refusals reaches the store.
#[tokio::test]
async fn each_catalog_refusal_reproduces_the_sentence_for_its_own_condition() {
    for (row, name) in catalog_names() {
        // A fresh emulator per row, so a name at the modelled ceiling can be taken in
        // every row without the store filling up across rows.
        let router = emulator();
        given_catalog(&router, &name).await;

        for condition in conditions() {
            let case = format!("{row} / {}", condition.case);
            let answered = assert_refusal(
                &router,
                &case,
                condition.operation,
                &(condition.request)(&name),
                condition.code,
                condition.message,
            )
            .await;
            assert_withholds(&case, &error_message(&answered), &name);
        }

        // Nothing the refusals above carried reached the store: the catalog is still the
        // one the first create wrote.
        let read = ok(&router, "GetCatalog", &json!({ "CatalogId": &name })).await;
        assert_eq!(
            read["Catalog"]["Description"].as_str(),
            Some("the first definition"),
            "case {row}: a refused write must leave the stored definition alone, got {read}"
        );
    }
}
