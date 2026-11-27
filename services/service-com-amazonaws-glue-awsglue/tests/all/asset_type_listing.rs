// SPDX-License-Identifier: CC0-1.0

//! `ListAssetTypes`: the asset types this account holds.
//!
//! RULE: the operation "lists the asset types defined in Glue Data Catalog", so it reports
//! every asset type record this account holds — the records `PutAssetType` wrote and
//! `GetAssetType` reads back — projected to the two members `AssetTypeItem` declares, one
//! `MaxResults`-sized page at a time in asset-type-name order with a `NextToken` that
//! resumes the walk, and nothing the store no longer holds.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many asset types are stored,
//! the page size asked for (including the published bounds 1 and 1000, a size smaller than
//! the stored count, one equal to it and one past it), the form and length of the stored
//! name, how many forms each record carries, and whether a record was registered, updated
//! or deleted between two listings.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, ok, qualified_asset_type_id};

/// The longest `Name` the model accepts: "len 1..128" on `AssetTypeName`
/// (`smithy_query operation=PutAssetType`), whose pattern
/// `^(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$` is what every name below keeps to.
const MAX_NAME_LENGTH: usize = 128;

/// Register a form type and return the `FormTypeId` the catalog assigned it, so an asset
/// type's form entries reference a form type this account really holds.
async fn form_type(router: &MultiServiceRouter, name: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": format!("structure {name} {{ a: String }}") }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutFormType must report an Id, got {stored}"))
        .to_string()
}

/// A `Forms` map sending every `(form name, form type id)` pair, in the modelled
/// `AssetTypeFormReference` shape.
fn forms(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (form_name, form_type_id) in entries {
        map.insert(
            (*form_name).to_string(),
            json!({ "FormTypeIdentifier": form_type_id }),
        );
    }
    Value::Object(map)
}

/// Register the asset type `name`, with the form entries the row asks for.
async fn given_asset_type(router: &MultiServiceRouter, name: &str, entries: &[(&str, &str)]) {
    ok(
        router,
        "PutAssetType",
        &json!({ "Name": name, "Forms": forms(entries) }),
    )
    .await;
}

/// The entries of one answer, as a list.
fn items(page: &Value) -> Vec<Value> {
    page["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("Items must be a list, got {page}"))
        .clone()
}

/// The `Name` of every entry in one answer, in the order the service returned them.
fn listed_names(page: &Value) -> Vec<String> {
    items(page)
        .iter()
        .map(|item| {
            item["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("AssetTypeItem.Name must be a string, got {item}"))
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out, following its own `NextToken`, and return the
/// entries in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListAssetTypes", &request).await;
        let entries = items(&page);
        if let Some(size) = page_size {
            assert!(
                entries.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {entries:?}"
            );
        }
        listed.extend(entries);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= limit,
            "case {case:?}: the walk must terminate, got {listed:?} for {limit} asset types"
        );
    }
    listed
}

/// One row of the summary-member table: the case it stands for, the name the asset type
/// is registered under, and the form entries its record carries.
struct SummaryRow<'a> {
    case: &'a str,
    name: String,
    forms: Vec<(&'a str, &'a str)>,
}

/// A name of exactly `length` characters that satisfies `AssetTypeName`'s pattern.
fn name_of_length(length: usize) -> String {
    let mut name = String::from("Z");
    name.push_str(&"x".repeat(length - 1));
    name
}

#[tokio::test]
async fn every_stored_asset_type_is_listed_and_paged() {
    // (case, how many asset types are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no asset types at all", 0, None),
        ("one asset type, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page exactly the stored count", 4, Some(4)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 6, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        let order_form = form_type(&router, "OrderForm").await;

        // Registered out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("AssetType{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_asset_type(&router, name, &[("Details", &order_form)]).await;
        }

        let listed = walk(&router, case, *page_size, *stored).await;
        let names: Vec<String> = listed
            .iter()
            .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored asset type in name order, got {names:?}"
        );

        // An asset type the listing reports is one `GetAssetType` resolves, and the same
        // record: one record, two reads.
        for item in &listed {
            let id = item["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the entry must carry an Id, got {item}"));
            let read = ok(&router, "GetAssetType", &json!({ "Identifier": id })).await;
            assert_eq!(
                read.get("Name"),
                item.get("Name"),
                "case {case:?}: GetAssetType({id}) must report the listed asset type, got {read} \
                 for {item}"
            );
            assert_eq!(
                read.get("Id").and_then(Value::as_str),
                Some(id),
                "case {case:?}: the listed Id must be the one a read reports, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_asset_type_carries_the_summary_members_the_put_stored() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let audit_form = form_type(&router, "AuditForm").await;

    let long_name = name_of_length(MAX_NAME_LENGTH);
    // The rows differ along the form and length of the name and along how many forms the
    // record carries: every one of them is a summary of the same two members.
    let rows: Vec<SummaryRow<'_>> = vec![
        SummaryRow {
            case: "a one-character name",
            name: "A".to_string(),
            forms: vec![("Details", order_form.as_str())],
        },
        SummaryRow {
            case: "a name carrying digits and underscores",
            name: "Order_Asset_2".to_string(),
            forms: vec![("details_2", order_form.as_str())],
        },
        SummaryRow {
            case: "a name starting with an underscore",
            name: "_internalAsset".to_string(),
            forms: vec![("Details", audit_form.as_str())],
        },
        SummaryRow {
            case: "two form entries",
            name: "AuditedOrderAsset".to_string(),
            forms: vec![
                ("Details", order_form.as_str()),
                ("Audit", audit_form.as_str()),
            ],
        },
        SummaryRow {
            case: "a name at the modelled length bound",
            name: long_name.clone(),
            forms: vec![("Details", order_form.as_str())],
        },
    ];

    for row in &rows {
        given_asset_type(&router, &row.name, &row.forms).await;
    }

    let listed = walk(&router, "summary members", None, rows.len()).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case summary members: expected {} asset types, got {listed:?}",
        rows.len()
    );

    for SummaryRow { case, name, forms: entries } in &rows {
        let item = listed
            .iter()
            .find(|item| item["Name"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {name} must be listed, got {listed:?}"));
        // `AssetTypeItem` is a summary of two members (`smithy_query shape=AssetTypeItem`):
        // the forms an asset type is made of are reported by `GetAssetType` and not here.
        assert!(
            item.get("Forms").is_none(),
            "case {case:?}: a listed asset type must report no Forms, got {item}"
        );
        assert_eq!(
            item.get("Id").and_then(Value::as_str),
            Some(qualified_asset_type_id(name).as_str()),
            "case {case:?}: expected the identifier a read reports, got {item}"
        );

        // The identifier the listing reports addresses the record, and the read carries the
        // forms the listing leaves out.
        let read = ok(
            &router,
            "GetAssetType",
            &json!({ "Identifier": item["Id"].as_str().unwrap_or_default() }),
        )
        .await;
        assert_eq!(
            read.get("Forms"),
            Some(&forms(entries)),
            "case {case:?}: GetAssetType must report the forms the put stored, got {read}"
        );
    }
}

#[tokio::test]
async fn the_listing_reflects_what_the_store_holds_now() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let order_form = form_type(&router, "OrderForm").await;
    let audit_form = form_type(&router, "AuditForm").await;

    // Nothing stored yet: the listing is empty rather than a refusal.
    let empty = ok(&router, "ListAssetTypes", &json!({})).await;
    assert_eq!(
        listed_names(&empty),
        Vec::<String>::new(),
        "case nothing stored: expected an empty listing, got {empty}"
    );

    for name in ["KeptOne", "KeptTwo", "Removed"] {
        given_asset_type(&router, name, &[("Details", &order_form)]).await;
    }

    let before = listed_names(&ok(&router, "ListAssetTypes", &json!({})).await);
    assert_eq!(
        before,
        vec![
            "KeptOne".to_string(),
            "KeptTwo".to_string(),
            "Removed".to_string()
        ],
        "case after the puts: expected every stored asset type, got {before:?}"
    );

    // Listing is reporting only: two listings in a row agree, so a read changed nothing a
    // later caller sees.
    let again = listed_names(&ok(&router, "ListAssetTypes", &json!({})).await);
    assert_eq!(
        again, before,
        "case listed twice: a listing must change nothing, got {again:?} then {before:?}"
    );

    // An update replaces a record's forms and not its identity, so the listing — a summary
    // of id and name — is unchanged by it.
    given_asset_type(&router, "KeptOne", &[("Audit", &audit_form)]).await;
    let updated = listed_names(&ok(&router, "ListAssetTypes", &json!({})).await);
    assert_eq!(
        updated, before,
        "case after an update: an update adds no record, got {updated:?}"
    );

    // An asset type the store no longer holds is not listed. The record is dropped through
    // `DeleteAssetType`, the operation that owns that removal.
    ok(
        &router,
        "DeleteAssetType",
        &json!({ "Identifier": "Removed" }),
    )
    .await;
    assert!(
        !engine
            .state::<ComAmazonawsGlueAwsglueState>()
            .lock()
            .asset_types
            .contains_key("Removed"),
        "case after the delete: the store must no longer hold Removed"
    );

    let after = listed_names(&ok(&router, "ListAssetTypes", &json!({})).await);
    assert_eq!(
        after,
        vec!["KeptOne".to_string(), "KeptTwo".to_string()],
        "case after the record is dropped: an asset type the store no longer holds must not \
         be listed, got {after:?}"
    );

    // A record registered after a listing is reported by the next one.
    given_asset_type(&router, "AddedLater", &[("Details", &order_form)]).await;
    let grown = listed_names(&ok(&router, "ListAssetTypes", &json!({})).await);
    assert_eq!(
        grown,
        vec![
            "AddedLater".to_string(),
            "KeptOne".to_string(),
            "KeptTwo".to_string()
        ],
        "case after a later put: expected the new record in name order, got {grown:?}"
    );
}

#[tokio::test]
async fn a_page_resumes_at_the_entry_after_the_token() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let names = ["Alpha", "Bravo", "Charlie", "Delta"];
    for name in names {
        given_asset_type(&router, name, &[("Details", &order_form)]).await;
    }

    // (case, the page size, the names of the first page, whether a token must follow it)
    let rows: &[(&str, i32, &[&str], bool)] = &[
        ("one at a time", 1, &["Alpha"], true),
        ("two at a time", 2, &["Alpha", "Bravo"], true),
        ("three at a time", 3, &["Alpha", "Bravo", "Charlie"], true),
        (
            "every asset type in one page",
            4,
            &["Alpha", "Bravo", "Charlie", "Delta"],
            false,
        ),
    ];

    for (case, page_size, first, continues) in rows {
        let page = ok(
            &router,
            "ListAssetTypes",
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            first
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the first {page_size} asset types in name order, got {page}"
        );
        let token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            token.is_some(),
            *continues,
            "case {case:?}: expected a continuation token {continues}, got {page}"
        );

        // Continuing from the token reports the REST of the asset types, each exactly once.
        if let Some(cursor) = &token {
            let next = ok(
                &router,
                "ListAssetTypes",
                &json!({ "NextToken": cursor }),
            )
            .await;
            let rest: Vec<String> = names
                .iter()
                .filter(|name| !first.contains(name))
                .map(|name| (*name).to_string())
                .collect();
            assert_eq!(
                listed_names(&next),
                rest,
                "case {case:?}: expected the asset types after the first page, got {next}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    given_asset_type(&router, "BoundedAsset", &[("Details", &order_form)]).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused
    // by the modelled constraint rather than served as some other page size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(
            &router,
            "ListAssetTypes",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }

    // Each published bound itself is served.
    for (case, max_results) in [("the minimum", 1), ("the maximum", 1_000)] {
        let page = ok(
            &router,
            "ListAssetTypes",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            vec!["BoundedAsset".to_string()],
            "case {case:?}: MaxResults={max_results} must be served, got {page}"
        );
    }
}
