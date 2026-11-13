// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFormType`, the delete of the Glue metadata form type slice
//! `PutFormType` writes and `GetFormType` reads.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a delete removes the ONE form type its `Identifier` resolves to —
//! a form type's identifier being the name it was registered under, which is also the `Id`
//! its put reports — so a later read of that form type misses while every other form type
//! is untouched; it is HELD UP while an ASSET TYPE still references that form type ("a
//! form type cannot be deleted if it is still referenced by an asset type"), whether by
//! declaring a form of it among its own forms or through an asset of that type carrying one,
//! earning the modelled `ConflictException` that names every such asset type and removing
//! nothing, and is served again once those records are gone; and it
//! reports no miss, because the operation models no not-found error
//! (`smithy_query operation=DeleteFormType` lists `AccessDeniedException`,
//! `ConcurrentModificationException`, `ConflictException`,
//! `InternalServiceException`, `InvalidInputException` and `ThrottlingException` and no
//! `EntityNotFoundException`, unlike `GetFormType`).
//!
//! The suites are tables whose rows differ along what the rule generalises over: the form
//! and length of the form type's name, the size of its stored schema, how an identifier
//! misses, how many forms an asset type holds and how many assets use the form type.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The longest `Name` the model accepts: "len 1..128" on `FormTypeName`
/// (`smithy_query operation=PutFormType`), whose pattern `^[A-Z]\w*$` is what the rows
/// below keep to.
const MAX_NAME_LENGTH: usize = 128;

/// The longest `Schema` the model accepts: "len 1..100000" on `FormTypeSchema`
/// (`smithy_query operation=PutFormType`). A row at this bound proves a form type is
/// removed whole whatever it holds.
const MAX_SCHEMA_LENGTH: usize = 100_000;

/// A name of exactly `length` characters that satisfies `FormTypeName`'s `^[A-Z]\w*$`.
fn name_of_length(length: usize) -> String {
    let mut name = String::from("D");
    name.push_str(&"e".repeat(length - 1));
    name
}

/// Register a form type and return the `FormTypeId` the catalog assigned it.
async fn put_form_type(router: &MultiServiceRouter, name: &str, schema: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": schema }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutFormType must report an Id, got {stored}"))
        .to_string()
}

/// Delete the form type `identifier` names, asserting the modelled 200.
async fn delete(router: &MultiServiceRouter, case: &str, identifier: &str) -> Value {
    let (status, body) = call(
        router,
        "DeleteFormType",
        &json!({ "Identifier": identifier }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteFormType of {identifier:?} must answer 200, got {status} with \
         {body}"
    );
    body
}

/// Assert that `identifier` resolves to no form type: 400 `EntityNotFoundException`, the
/// miss `GetFormType` models.
async fn assert_read_misses(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(router, "GetFormType", &json!({ "Identifier": identifier })).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted form type {identifier:?}, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
    );
    // The refusal's `Message` member, pinned exactly: the metadata form type surface's
    // refusal text is a JSON document carrying the sentence under a `Message` key, with the
    // identifier from the request as its one variable part.
    assert_eq!(
        error_message(&body),
        format!(r#"{{"Message":"Metadata form type {identifier} not found."}}"#),
        "case {case}: unexpected refusal wording for {identifier:?}, got {body}"
    );
}

/// The schema `GetFormType` reports for `identifier`, which must still resolve.
async fn read_schema(router: &MultiServiceRouter, identifier: &str) -> String {
    let read = ok(router, "GetFormType", &json!({ "Identifier": identifier })).await;
    read.get("Schema")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetFormType must report a Schema, got {read}"))
        .to_string()
}

/// A `Forms` map sending every `(form name, form type identifier)` pair, in the modelled
/// `AssetTypeFormReference` shape.
fn forms(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (form_name, form_type_identifier) in entries {
        map.insert(
            (*form_name).to_string(),
            json!({ "FormTypeIdentifier": form_type_identifier }),
        );
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// The delete removes the one form type it names
// ---------------------------------------------------------------------------

/// A delete removes the form type its `Identifier` resolves to and nothing else: a read of
/// the deleted form type misses, every other stored form type still reports its own schema,
/// and a put under the deleted name stores the form type afresh.
#[tokio::test]
async fn deleting_a_form_type_removes_only_the_one_it_names() {
    let router = emulator();

    let long_name = name_of_length(MAX_NAME_LENGTH);
    let max_schema = format!(
        "structure Big {{ {}}}",
        "a".repeat(MAX_SCHEMA_LENGTH - "structure Big { }".len())
    );
    // (case, name, schema)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the shape a lifecycle client registers",
            "OrderForm".to_string(),
            "structure OrderForm { orderId: String }".to_string(),
        ),
        (
            "a one-character name and a one-character schema",
            "F".to_string(),
            "x".to_string(),
        ),
        (
            "a name carrying digits and underscores",
            "Order_Form_2".to_string(),
            "structure Order_Form_2 { total: Integer }".to_string(),
        ),
        (
            "a name at the modelled length bound",
            long_name,
            "structure Long { a: String }".to_string(),
        ),
        (
            "a schema at the modelled length bound",
            "MaximalSchema".to_string(),
            max_schema,
        ),
    ];

    // A bystander, to prove the delete is scoped to the form type it names.
    let bystander_schema = "structure Bystander { a: String }";
    let bystander = put_form_type(&router, "Bystander", bystander_schema).await;

    // Every row is stored FIRST, so each delete runs against a store holding several form
    // types and has to reach exactly one of them.
    let mut stored: Vec<(&str, String, String, String)> = Vec::new();
    for (case, name, schema) in rows {
        let id = put_form_type(&router, &name, &schema).await;
        assert_eq!(
            id, name,
            "case {case}: the identifier a put reports is the form type's name"
        );
        stored.push((case, name, schema, id));
    }

    for (case, name, schema, id) in &stored {
        // The record is there to begin with, so the miss asserted below is the delete's
        // doing.
        assert_eq!(
            &read_schema(&router, id).await,
            schema,
            "case {case}: the form type must be readable before the delete"
        );

        let body = delete(&router, case, id).await;
        assert!(
            body.is_null() || body.as_object().is_some_and(Map::is_empty),
            "case {case}: DeleteFormTypeResponse models no members, got {body}"
        );

        // The record is gone, by the identifier the delete named and by the name the put
        // registered it under.
        assert_read_misses(&router, case, id).await;
        assert_read_misses(&router, case, name).await;
    }

    // Nothing a delete did reached the form type it did not name.
    assert_eq!(
        read_schema(&router, &bystander).await,
        bystander_schema,
        "case bystander: a delete must leave every other form type in place"
    );

    // A put under a deleted name stores the form type afresh: the delete freed the name,
    // it did not latch it.
    for (case, name, schema, _) in &stored {
        let new_id = put_form_type(&router, name, schema).await;
        assert_eq!(
            &new_id, name,
            "case {case}: a put after the delete must report the form type's name as its id"
        );
        assert_eq!(
            &read_schema(&router, &new_id).await,
            schema,
            "case {case}: the re-registered form type must read back"
        );
    }
}

// ---------------------------------------------------------------------------
// A form type an asset still uses
// ---------------------------------------------------------------------------

/// A delete is REFUSED for a form type an ASSET still carries a form of: "a form type cannot
/// be deleted if it is still referenced by an asset type" (`DeleteFormType`), so the request
/// earns the operation's modelled `ConflictException` naming every dependency that holds it
/// up — the asset type of each using asset — the form type is still readable afterwards and
/// no asset type is rewritten; and once the using assets are gone the same delete is served.
///
/// The rows differ along how many forms the asset type declares, whether two of its entries
/// share the doomed form type, how many assets (under how many asset types) use it, and
/// whether the asset types keep a reference to a form type the delete never names.
#[tokio::test]
async fn deleting_a_form_type_an_asset_uses_is_refused_while_that_use_stands() {
    let router = emulator();

    /// One row: the asset types whose assets carry a form of the form type the delete names.
    struct Row {
        case: &'static str,
        /// The form type this row's delete names.
        doomed_name: &'static str,
        /// A second form type no delete names, which must be left alone throughout.
        survivor_name: &'static str,
        /// Every asset type one of this row's assets is an instance of.
        asset_types: &'static [&'static str],
        /// How many further entries each asset type holds, all naming the survivor.
        extra_entries: usize,
        /// Whether a second entry of each asset type names the doomed form type too.
        duplicate_entry: bool,
    }

    let rows = vec![
        Row {
            case: "the one form of a single-form asset type",
            doomed_name: "SoloForm",
            survivor_name: "SoloSurvivor",
            asset_types: &["SoloAsset"],
            extra_entries: 0,
            duplicate_entry: false,
        },
        Row {
            case: "one of two forms",
            doomed_name: "NamedForm",
            survivor_name: "NamedSurvivor",
            asset_types: &["NamedAsset"],
            extra_entries: 1,
            duplicate_entry: false,
        },
        Row {
            case: "one of four forms",
            doomed_name: "SharedForm",
            survivor_name: "SharedSurvivor",
            asset_types: &["SharedAsset"],
            extra_entries: 3,
            duplicate_entry: false,
        },
        Row {
            case: "two entries of one asset type referencing the form type",
            doomed_name: "TwiceForm",
            survivor_name: "TwiceSurvivor",
            asset_types: &["TwiceAsset"],
            extra_entries: 1,
            duplicate_entry: true,
        },
        Row {
            case: "three asset types referencing the one form type",
            doomed_name: "CommonForm",
            survivor_name: "CommonSurvivor",
            // Named so the dependency list below is in this namespace's own (name) order.
            asset_types: &["CommonAssetA", "CommonAssetB", "CommonAssetC"],
            extra_entries: 1,
            duplicate_entry: false,
        },
    ];

    for row in &rows {
        let doomed_id = put_form_type(
            &router,
            row.doomed_name,
            &format!("structure {} {{ a: String }}", row.doomed_name),
        )
        .await;
        let doomed_schema = format!("structure {} {{ a: String }}", row.doomed_name);
        let survivor_schema = format!("structure {} {{ b: String }}", row.survivor_name);
        let survivor_id = put_form_type(&router, row.survivor_name, &survivor_schema).await;

        let mut entries: Vec<(&str, &str)> = vec![("primary", doomed_id.as_str())];
        if row.duplicate_entry {
            entries.push(("alias", doomed_id.as_str()));
        }
        for index in 0..row.extra_entries {
            // Every extra entry references the surviving form type, so the asset type's
            // other references are there to be compared after the delete too.
            let name: &'static str = match index {
                0 => "extra0",
                1 => "extra1",
                _ => "extra2",
            };
            entries.push((name, survivor_id.as_str()));
        }

        let mut forms_before: Vec<Value> = Vec::new();
        for asset_type in row.asset_types {
            let put = ok(
                &router,
                "PutAssetType",
                &json!({ "Name": asset_type, "Forms": forms(&entries) }),
            )
            .await;
            forms_before.push(
                put.get("Forms")
                    .cloned()
                    .unwrap_or_else(|| panic!("case {}: PutAssetType must report Forms", row.case)),
            );
        }

        // A DECLARATION alone already holds the delete up. One asset per asset type, each
        // carrying a form of the doomed form type, references it through that same asset
        // type, so the dependency list below names each asset type once however many
        // references it carries.
        let assets: Vec<String> = row
            .asset_types
            .iter()
            .map(|asset_type| format!("{}-asset", asset_type.to_lowercase()))
            .collect();
        for (asset_type, asset) in row.asset_types.iter().zip(assets.iter()) {
            ok(
                &router,
                "PutAsset",
                &json!({
                    "Identifier": asset,
                    "AssetTypeId": asset_type,
                    "Name": asset,
                    "Forms": { "primary": { "FormTypeId": &doomed_id, "Content": "{}" } },
                }),
            )
            .await;
        }

        // The form type is readable to begin with, so the refusal below is about the
        // reference and not about a form type that is not there.
        assert_eq!(
            read_schema(&router, &doomed_id).await,
            doomed_schema,
            "case {}: the form type must be readable before the delete",
            row.case
        );

        // The delete is held up while the assets stand — by either address the form
        // type answers to — and the refusal names every asset type that holds it up.
        for identifier in [doomed_id.as_str(), row.doomed_name] {
            assert_delete_is_refused(
                &router,
                row.case,
                identifier,
                row.doomed_name,
                row.asset_types,
            )
            .await;
        }
        // The refused delete removed nothing: the form type still reports its schema.
        assert_eq!(
            read_schema(&router, &doomed_id).await,
            doomed_schema,
            "case {}: a refused delete must leave the form type in place",
            row.case
        );

        // And it rewrote nothing else: every asset type still reports the `Forms` its put
        // wrote.
        for (asset_type, before) in row.asset_types.iter().zip(forms_before.iter()) {
            let read = ok(
                &router,
                "GetAssetType",
                &json!({ "Identifier": asset_type }),
            )
            .await;
            assert_eq!(
                read.get("Forms"),
                Some(before),
                "case {}: the refused delete must rewrite no asset type, got {read}",
                row.case
            );
        }

        // Dropping the using ASSETS is not enough: each asset type still declares the form
        // type, which is a reference of its own, so the identical delete is still refused
        // and names the same dependencies.
        for asset in &assets {
            ok(&router, "DeleteAsset", &json!({ "Identifier": asset })).await;
        }
        for identifier in [doomed_id.as_str(), row.doomed_name] {
            assert_delete_is_refused(
                &router,
                row.case,
                identifier,
                row.doomed_name,
                row.asset_types,
            )
            .await;
        }

        // Once the asset types are gone too, nothing references the form type any more and
        // the very same delete is served, by either address.
        for asset_type in row.asset_types {
            ok(
                &router,
                "DeleteAssetType",
                &json!({ "Identifier": asset_type }),
            )
            .await;
        }
        delete(&router, row.case, &doomed_id).await;
        assert_read_misses(&router, row.case, &doomed_id).await;
        assert_read_misses(&router, row.case, row.doomed_name).await;

        // The form type the asset types' other entries named was never the delete's
        // target and is untouched throughout.
        assert_eq!(
            read_schema(&router, &survivor_id).await,
            survivor_schema,
            "case {}: a delete must leave the other referenced form types in place",
            row.case
        );
    }
}

/// The dependency a refusal names an asset type by: the kind of dependency it is, and the
/// asset type's address inside the catalog that holds it. Written here independently of the
/// service, so the suite pins the wording and not the code.
fn dependency_of(asset_type: &str) -> String {
    format!("gluetype glueAsset#glue_{ACCOUNT_ID}#{asset_type}")
}

/// An asset type that only DECLARES a form type among its forms still REFERENCES it: the
/// delete is refused while that asset type stands, naming it as the dependency that holds the
/// delete up, and the same delete is served once the asset type itself is gone.
///
/// The rows differ along how many asset types declare the form type, how many forms each
/// declares, and whether an asset of that asset type exists at all — carrying a form of
/// ANOTHER form type, so the asset is there without using the one being deleted.
#[tokio::test]
async fn deleting_a_form_type_an_asset_type_declares_is_refused_until_that_type_is_gone() {
    let router = emulator();

    // (case, the form type the delete names, the asset types declaring it, whether an
    // asset of each asset type exists)
    let rows: Vec<(&str, &str, &[&str], bool)> = vec![
        ("one declaring asset type", "DanglingOne", &["DanglingTypeA"], false),
        (
            "two declaring asset types",
            "DanglingTwo",
            &["DanglingTypeB", "DanglingTypeC"],
            false,
        ),
        (
            "a declaring asset type with an asset that uses another form type",
            "DanglingThree",
            &["DanglingTypeD"],
            true,
        ),
    ];

    // The form type every "other" asset carries a form of, so such an asset never uses the
    // form type a row deletes.
    let other = put_form_type(&router, "DanglingOther", "structure DanglingOther { a: String }").await;

    for (case, doomed_name, asset_types, with_asset) in rows {
        let doomed_schema = format!("structure {doomed_name} {{ a: String }}");
        let doomed_id = put_form_type(&router, doomed_name, &doomed_schema).await;

        let mut assets: Vec<String> = Vec::new();
        for asset_type in asset_types {
            ok(
                &router,
                "PutAssetType",
                &json!({
                    "Name": asset_type,
                    "Forms": forms(&[("primary", doomed_id.as_str()), ("other", other.as_str())]),
                }),
            )
            .await;
            if with_asset {
                let asset = format!("{}-other-asset", asset_type.to_lowercase());
                ok(
                    &router,
                    "PutAsset",
                    &json!({
                        "Identifier": &asset,
                        "AssetTypeId": asset_type,
                        "Name": asset_type,
                        "Forms": { "other": { "FormTypeId": &other, "Content": "{}" } },
                    }),
                )
                .await;
                assets.push(asset);
            }
        }

        // The declaration is a reference, so it holds the delete up by either address the
        // form type answers to, and the form type stays readable.
        for identifier in [doomed_id.as_str(), doomed_name] {
            assert_delete_is_refused(&router, case, identifier, doomed_name, asset_types).await;
        }
        assert_eq!(
            read_schema(&router, &doomed_id).await,
            doomed_schema,
            "case {case}: a refused delete must leave the form type in place"
        );

        // Once the declaring asset types are gone, nothing references the form type and the
        // identical delete is served.
        for asset in &assets {
            ok(&router, "DeleteAsset", &json!({ "Identifier": asset })).await;
        }
        for asset_type in asset_types {
            ok(
                &router,
                "DeleteAssetType",
                &json!({ "Identifier": asset_type }),
            )
            .await;
        }
        delete(&router, case, &doomed_id).await;
        assert_read_misses(&router, case, &doomed_id).await;
    }

    // The form type no row ever named is untouched: the deletes above were refused and
    // served for the references they named, not for any reference at all.
    assert_eq!(
        read_schema(&router, &other).await,
        "structure DanglingOther { a: String }",
        "case the other form type: no delete above ever named it"
    );
}

/// Assert that the delete of `identifier` is refused with the modelled `ConflictException`
/// whose `Message` is the business catalog's JSON document naming the form type and every
/// dependency holding the delete up.
async fn assert_delete_is_refused(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    form_type: &str,
    asset_types: &[&str],
) {
    let (status, body) = call(
        router,
        "DeleteFormType",
        &json!({ "Identifier": identifier }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: DeleteFormType of {identifier:?} must be refused while an asset uses \
         it, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ConflictException"),
        "case {case}: expected ConflictException for {identifier:?}, got {body}"
    );
    let dependencies: Vec<String> = asset_types.iter().copied().map(dependency_of).collect();
    assert_eq!(
        error_message(&body),
        format!(
            r#"{{"Message":"Form type {form_type} cannot be deleted because of the following dependencies: {}"}}"#,
            dependencies.join(", ")
        ),
        "case {case}: unexpected refusal wording for {identifier:?}, got {body}"
    );
}

/// A NEW reference to a deleted form type is a different matter from one that already
/// stands: a put whose form entry names a form type the catalog no longer holds resolves to
/// nothing and is refused with the same miss the form type's own reader answers, worded the
/// same way, so nothing is stored.
///
/// The rows differ along whether the dead reference is the asset type's only entry or one
/// of several, and along whether the identifier was deleted or was never registered.
#[tokio::test]
async fn putting_an_asset_type_that_references_a_deleted_form_type_is_refused() {
    let router = emulator();

    let live_schema = "structure LiveForm { a: String }";
    let live = put_form_type(&router, "LiveForm", live_schema).await;
    let deleted = put_form_type(&router, "DeletedForm", "structure DeletedForm { a: String }").await;
    delete(&router, "the form type the put will reference", &deleted).await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the only entry names a deleted form type",
            deleted.as_str(),
            forms(&[("Details", deleted.as_str())]),
        ),
        (
            "one of two entries names a deleted form type",
            deleted.as_str(),
            forms(&[("Live", live.as_str()), ("Dead", deleted.as_str())]),
        ),
        (
            "the only entry names a form type never registered",
            "NeverRegisteredForm",
            forms(&[("Details", "NeverRegisteredForm")]),
        ),
    ];

    for (case, missing, entries) in &rows {
        let (status, body) = call(
            &router,
            "PutAssetType",
            &json!({ "Name": "DeadRefAsset", "Forms": entries }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for a reference to {missing:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(r#"{{"Message":"Metadata form type {missing} not found."}}"#),
            "case {case}: the refusal must be worded as the form type surface words its \
             miss, got {body}"
        );

        // Nothing was stored: the asset type the refused put named does not exist.
        let (status, body) = call(
            &router,
            "GetAssetType",
            &json!({ "Identifier": "DeadRefAsset" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused put must store no asset type, got {status} with {body}"
        );
    }

    // The form type that is still registered is unaffected by those refusals.
    assert_eq!(
        read_schema(&router, &live).await,
        live_schema,
        "case the live form type: a refused put must change nothing"
    );
}

// ---------------------------------------------------------------------------
// An identifier that resolves to nothing
// ---------------------------------------------------------------------------

/// A delete for an identifier this account holds no form type under is SERVED: this
/// operation models no not-found error, so the caller that asks for a state the store is
/// already in is answered as though the delete had happened, and nothing is removed.
///
/// The rows differ along how the identifier misses: never registered, a prefix of a stored
/// identifier, a stored identifier with one more character, a stored identifier recased, a
/// form type that was already deleted, an identifier at the modelled length bound, and one
/// no form type name could ever take.
#[tokio::test]
async fn deleting_a_form_type_the_store_does_not_hold_is_served() {
    let router = emulator();

    let kept_schema = "structure KeptForm { a: String }";
    let kept = put_form_type(&router, "KeptForm", kept_schema).await;
    // A form type registered and then deleted, so one row names an address that WAS live.
    let gone = put_form_type(&router, "GoneForm", "structure GoneForm { a: String }").await;
    delete(&router, "the form type deleted up front", &gone).await;

    let truncated = kept[..kept.len() - 1].to_string();
    let extended = format!("{kept}0");
    let recased = kept.to_uppercase();
    // "len 1..256" on `FormTypeId` (`smithy_query operation=DeleteFormType`): the widest
    // identifier the model accepts, which the handler must resolve rather than reject.
    let widest = "F".repeat(256);
    let rows: Vec<(&str, String)> = vec![
        ("never registered", "NoSuchFormType".to_string()),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", extended),
        ("a stored identifier recased", recased),
        ("an already deleted form type", gone.clone()),
        ("an identifier at the modelled length bound", widest),
        (
            "an identifier no form type name could take",
            "no-such-form-type".to_string(),
        ),
    ];
    // No row may name the one form type the store holds, or it would assert a delete that
    // happened rather than one that resolved to nothing.
    for (case, identifier) in &rows {
        assert_ne!(
            identifier, &kept,
            "case {case}: the row must name an identifier the store does not hold"
        );
    }

    for (case, identifier) in &rows {
        delete(&router, case, identifier).await;
        // Repeating it answers the same way: the delete of something absent is not a
        // state change that a second call could fail on.
        delete(&router, case, identifier).await;
    }

    // Nothing was removed by any of those calls: the form type the store does hold is
    // still readable, with the schema its put stored.
    assert_eq!(
        read_schema(&router, &kept).await,
        kept_schema,
        "case the stored form type: a delete that resolved to nothing must remove nothing"
    );
}
