// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the account's own Data Catalog under `DeleteCatalog`: the
//! catalog every account HAS, which holds the catalogs a `CreateCatalog` registers and
//! is not one of them.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test is that this catalog is a protected singleton: a delete that
//! addresses it is refused with the operation's modelled `InvalidInputException` stating
//! the rule a deletable catalog's name satisfies — an input-validation refusal, not a
//! missing entity — the refusal persists nothing, and the delete path is otherwise
//! unaffected by it. The suites are tables: each row is one input along the axis the rule
//! generalises over (what the store holds around the protected catalog, how many refusals
//! precede the next delete, how a deletable catalog's id is spelled).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The refusal a delete of the account's own Data Catalog earns, written here
/// independently of the handler so the suite pins the wording: whole fixed text for this
/// error class, interpolating nothing from the request or from the store.
const NOT_DELETABLE: &str =
    "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name";

/// An account id that is not the one this emulator serves, so the catalogs it names
/// belong to another account's estate.
const OTHER_ACCOUNT_ID: &str = "123456789012";

/// A `CreateCatalog` request body for `name`: every create names a source outside the
/// Glue Data Catalog, which is the only catalog a create registers.
fn create_request(name: &str) -> Value {
    json!({
        "Name": name,
        "CatalogInput": {
            "FederatedCatalog": {
                "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
                "ConnectionName": "aws:s3tables",
            },
        },
    })
}

/// The status and body of a `DeleteCatalog` for `catalog_id`.
async fn delete(router: &MultiServiceRouter, catalog_id: &str) -> (u16, Value) {
    call(router, "DeleteCatalog", &json!({ "CatalogId": catalog_id })).await
}

/// The status and body of a `GetCatalog` for `catalog_id`.
async fn get(router: &MultiServiceRouter, catalog_id: &str) -> (u16, Value) {
    call(router, "GetCatalog", &json!({ "CatalogId": catalog_id })).await
}

/// The `Catalog` the read of the account's own Data Catalog reports, which must serve.
async fn root_catalog(case: &str, router: &MultiServiceRouter) -> Value {
    let (status, body) = get(router, ACCOUNT_ID).await;
    assert_eq!(
        status, 200,
        "case {case}: the account's own Data Catalog must read back, got {status} with {body}"
    );
    body.get("Catalog")
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: GetCatalog must report a Catalog, got {body}"))
}

/// Assert that one delete of the account's own Data Catalog was refused as invalid input,
/// with the catalog-name rule as its whole wording.
fn assert_protected(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: deleting the account's own Data Catalog must be refused, got {status} \
         with {body}"
    );
    assert!(
        error_code(body).ends_with("InvalidInputException"),
        "case {case}: expected InvalidInputException, got {} in {body}",
        error_code(body)
    );
    assert_eq!(
        error_message(body),
        NOT_DELETABLE,
        "case {case}: expected the catalog-name rule as the refusal, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The protected singleton
// ---------------------------------------------------------------------------

/// Deleting the account's own Data Catalog is refused as invalid input however often it
/// is attempted, and the refusal persists nothing: the read that serves that catalog
/// reports the very same `Catalog` afterwards, and everything stored inside it is still
/// there.
///
/// The rows differ along what the store holds around the protected catalog (nothing, a
/// catalog a create registered, several of them, a database of the account's own catalog,
/// a catalog that was created and then deleted) and along how many deletes in a row
/// address it, so what is refused is the catalog's own protection and not an artefact of
/// one store shape or of the first attempt.
#[tokio::test]
async fn the_accounts_own_data_catalog_is_never_deletable() {
    let cases: Vec<(&str, Vec<&str>, Vec<&str>, usize)> = vec![
        ("an empty store, one attempt", vec![], vec![], 1),
        ("an empty store, three attempts", vec![], vec![], 3),
        ("one registered catalog", vec!["guarded-one"], vec![], 2),
        (
            "several registered catalogs",
            vec!["guarded-two", "guarded-three"],
            vec![],
            2,
        ),
        (
            "a database of the account's own catalog",
            vec![],
            vec!["guarded_database"],
            2,
        ),
        (
            "catalogs and databases together",
            vec!["guarded-four"],
            vec!["guarded_other_database"],
            3,
        ),
    ];

    for (case, catalogs, databases, attempts) in &cases {
        // One store per row: what each row varies is what the store holds.
        let router = emulator();
        for name in catalogs {
            ok(&router, "CreateCatalog", &create_request(name)).await;
        }
        for name in databases {
            ok(
                &router,
                "CreateDatabase",
                &json!({ "DatabaseInput": { "Name": name } }),
            )
            .await;
        }

        let before = root_catalog(case, &router).await;

        for attempt in 1..=*attempts {
            let (status, body) = delete(&router, ACCOUNT_ID).await;
            assert_protected(&format!("{case}, attempt {attempt}"), status, &body);

            // Nothing was written for the refusal: the same read still reports the same
            // catalog, member for member.
            let after = root_catalog(case, &router).await;
            assert_eq!(
                after, before,
                "case {case}, attempt {attempt}: the refused delete must leave the catalog \
                 unchanged; expected {before}, got {after}"
            );
        }

        // Everything the protected catalog holds survived the refusals.
        for name in catalogs {
            let (status, body) = get(&router, name).await;
            assert_eq!(
                status, 200,
                "case {case}: the catalog {name} must survive the refusals, got {status} with \
                 {body}"
            );
        }
        for name in databases {
            let (status, body) = call(&router, "GetDatabase", &json!({ "Name": name })).await;
            assert_eq!(
                status, 200,
                "case {case}: the database {name} must survive the refusals, got {status} with \
                 {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The refusal is told apart from the delete path's other outcomes
// ---------------------------------------------------------------------------

/// The protection is the account's own Data Catalog's alone: after any number of refused
/// deletes of it, a catalog a create registered still deletes and then reads back as a
/// miss, an id the account holds no catalog under is still the modelled missing entity,
/// and a catalog of another account is still refused as unauthorized — each with its own
/// error class, so a client can tell a protected catalog from a missing one.
///
/// The rows differ along how many refusals precede the call and along how the id is
/// spelled (a bare name, the account and the name, a name in another casing).
#[tokio::test]
async fn the_protection_leaves_the_rest_of_the_delete_path_alone() {
    for (case, refusals, created, delete_id) in [
        ("a bare name, no refusal first", 0usize, "plain-catalog", "plain-catalog".to_string()),
        ("a bare name, after one refusal", 1, "after-one", "after-one".to_string()),
        (
            "the account and the name, after two refusals",
            2,
            "qualified-after-two",
            format!("{ACCOUNT_ID}:qualified-after-two"),
        ),
        (
            "the name in another casing, after three refusals",
            3,
            "CasedAfterThree",
            "casedafterthree".to_string(),
        ),
    ] {
        let router = emulator();
        ok(&router, "CreateCatalog", &create_request(created)).await;

        for attempt in 1..=refusals {
            let (status, body) = delete(&router, ACCOUNT_ID).await;
            assert_protected(&format!("{case}, refusal {attempt}"), status, &body);
        }

        // The registered catalog is deletable, and its read is the modelled miss after.
        let (status, body) = delete(&router, &delete_id).await;
        assert_eq!(
            status, 200,
            "case {case}: deleting {delete_id:?} must answer 200, got {status} with {body}"
        );
        let (status, body) = get(&router, created).await;
        assert_eq!(
            status, 400,
            "case {case}: the deleted catalog must not read back, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException after the delete, got {body}"
        );

        // An id this account holds no catalog under is a missing entity, NOT the
        // protection's invalid input.
        let (status, body) = delete(&router, "never-created-catalog").await;
        assert_eq!(
            status, 400,
            "case {case}: deleting an unheld id must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: an unheld id is a missing entity, got {body}"
        );

        // Another account's own Data Catalog is refused as unauthorized, not as the
        // protection of this account's catalog.
        let (status, body) = delete(&router, OTHER_ACCOUNT_ID).await;
        assert_eq!(
            status, 400,
            "case {case}: deleting another account's catalog must be refused, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("AccessDeniedException"),
            "case {case}: another account's catalog is unauthorized, got {body}"
        );

        // And the protected catalog itself is still protected, and still readable.
        let (status, body) = delete(&router, ACCOUNT_ID).await;
        assert_protected(&format!("{case}, after the other outcomes"), status, &body);
        root_catalog(case, &router).await;
    }
}
