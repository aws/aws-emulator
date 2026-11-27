// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the HARDWARE SPECIFICATION a published catalog item reports —
//! which members it carries, and that it carries no other — driven over the wire
//! through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs (the
//! WHOLE published catalog, which spans both modelled item classes and both statuses):
//!
//! 1. A read of the catalog projects the modelled `CatalogItem` and nothing else: every
//!    member an item reports is one the model declares for that shape
//!    ([`MODELLED_MEMBERS`]), and the members that describe hardware every published
//!    configuration has an answer for — its id, its status, its power draw, its weight,
//!    the uplink speeds it supports, the storage it supports and its EC2 capacity — are
//!    reported by every item.
//! 2. The class a configuration is published as is NOT one of those members: it is a
//!    fact of the catalog that `ItemClassFilter` selects on, so the single-class answers
//!    partition the catalog while no item reports a class of its own.
//! 3. `EC2Capacities` splits an instance type between its two members: `Family` names
//!    the family and `MaxSize` the SIZE SUFFIX alone, so neither repeats the other and
//!    the two join into an instance type `ListOrderableInstanceTypes` publishes.
//! 4. The single-item read answers the same specification as the listing, member for
//!    member, for every SKU the catalog publishes — a client that read a SKU out of the
//!    list reads the same hardware facts back by it.
//!
//! What a catalog item reports about its power, weight, storage and scaling, and how
//! the catalog is filtered and paged, is [`crate::catalog_items`]; the single-item read
//! itself is [`crate::get_catalog_item`].

use std::collections::BTreeSet;

use serde_json::Value;

use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// The modelled `CatalogItemClass` values (`RACK` | `SERVER`), as
/// `smithy_query(query="shape", shape="CatalogItemClass")` reports them — what
/// `ListCatalogItems$ItemClassFilter` accepts.
const ITEM_CLASSES: &[&str] = &["RACK", "SERVER"];

/// Every member the modelled `CatalogItem` declares, as
/// `smithy_query(query="shape", shape="CatalogItem")` reports them. A read of the
/// catalog may report no member outside this set.
const MODELLED_MEMBERS: &[&str] = &[
    "CatalogItemId",
    "ItemStatus",
    "EC2Capacities",
    "PowerKva",
    "WeightLbs",
    "SupportedUplinkGbps",
    "SupportedStorage",
    "RackScalingType",
];

/// The members of that set every published configuration has an answer for, so every
/// item reports them.
///
/// `RackScalingType` is the one member the whole catalog does not share: rack scaling
/// does not apply to a single server, which is [`crate::catalog_items`]'s subject.
const PUBLISHED_FOR_EVERY_ITEM: &[&str] = &[
    "CatalogItemId",
    "ItemStatus",
    "EC2Capacities",
    "PowerKva",
    "WeightLbs",
    "SupportedUplinkGbps",
    "SupportedStorage",
];

/// The `CatalogItems` of one `ListCatalogItems` answer.
fn items(output: &Value) -> Vec<Value> {
    output
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The whole published catalog, in the order `ListCatalogItems` reports it — the page
/// size is the modelled `MaxResults` maximum, so one call exhausts it.
async fn catalog(outposts: &Outposts) -> Vec<Value> {
    items(
        &outposts
            .ok("GET", "/catalog/items?MaxResults=1000", None)
            .await,
    )
}

/// The catalog narrowed to one modelled `CatalogItemClass`.
async fn catalog_of_class(outposts: &Outposts, class: &str) -> Vec<Value> {
    items(
        &outposts
            .ok(
                "GET",
                &format!("/catalog/items?MaxResults=1000&ItemClassFilter={class}"),
                None,
            )
            .await,
    )
}

/// Every instance type this service holds a specification of.
///
/// The wider of its two instance-type lists: a configuration's published capacity
/// names the whole host of a family its hardware runs, including a family offered on
/// SERVER hardware alone, which the catalog `ListOrderableInstanceTypes` publishes no
/// entry for (see [`crate::instance_type_catalogs`]).
fn specified_instance_types() -> BTreeSet<String> {
    crate::instance_type_catalogs::specifications()
        .iter()
        .map(|entry| text(entry, "InstanceType"))
        .collect()
}

/// The member names one reported item carries.
fn members(item: &Value) -> BTreeSet<String> {
    item.as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// A reported list-of-integers member, empty when the wire omits it.
fn integers(item: &Value, key: &str) -> Vec<i64> {
    item.get(key)
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// Rule 1: every published item projects the modelled `CatalogItem` and nothing else.
#[tokio::test]
async fn every_catalog_item_reports_the_modelled_specification_of_its_hardware() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;
    assert!(
        catalog.len() > 1,
        "the service publishes a catalog of configurations, got {catalog:?}"
    );

    let modelled: BTreeSet<&str> = MODELLED_MEMBERS.iter().copied().collect();
    let mut statuses: BTreeSet<String> = BTreeSet::new();
    for item in &catalog {
        let case = text(item, "CatalogItemId");
        let reported = members(item);
        let unmodelled: Vec<&String> = reported
            .iter()
            .filter(|member| !modelled.contains(member.as_str()))
            .collect();
        assert!(
            unmodelled.is_empty(),
            "case {case:?}: an item reports only members the model declares, got \
             {unmodelled:?} in {item:?}"
        );
        for member in PUBLISHED_FOR_EVERY_ITEM {
            assert!(
                reported.contains(*member),
                "case {case:?}: every published configuration reports {member}, got {item:?}"
            );
        }
        let uplink_gbps = integers(item, "SupportedUplinkGbps");
        assert!(
            !uplink_gbps.is_empty(),
            "case {case:?}: an item names the uplink speeds it supports, got {item:?}"
        );
        statuses.insert(text(item, "ItemStatus"));
    }

    // The catalog spans what the rule generalises over, so no clause above passed for
    // want of a case: both modelled statuses, and an item of each modelled class (which
    // rule 2 reads through the filter, the class being no member of an item).
    assert!(
        statuses.len() > 1,
        "the catalog must publish items of more than one status, got {statuses:?}"
    );
    for class in ITEM_CLASSES {
        assert!(
            !catalog_of_class(&outposts, class).await.is_empty(),
            "case {class:?}: the catalog must publish an item of every modelled class"
        );
    }
}

/// Rule 2: the class is a fact of the catalog the filter selects on rather than a
/// member an item reports, so the single-class answers partition the catalog.
#[tokio::test]
async fn the_class_filter_selects_a_fact_no_item_reports() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;

    let mut union: Vec<Value> = Vec::new();
    for class in ITEM_CLASSES {
        let selected = catalog_of_class(&outposts, class).await;
        assert!(
            !selected.is_empty(),
            "case {class:?}: the catalog must publish something of every modelled class"
        );
        for item in &selected {
            let case = text(item, "CatalogItemId");
            assert!(
                !members(item).contains("ItemClass"),
                "case {case:?}: the class is no member of the modelled CatalogItem, got {item:?}"
            );
            assert!(
                catalog.contains(item),
                "case {case:?}: a filtered read must report the item the catalog publishes, \
                 whole, got {item:?}"
            );
            assert!(
                !union.contains(item),
                "case {case:?}: an item is published in one class only, got {item:?}"
            );
        }
        union.extend(selected);
    }

    // Every published item is of one of the modelled classes, so the two single-class
    // answers together are the whole catalog, in its own order.
    let together: Vec<Value> = catalog
        .iter()
        .filter(|item| union.contains(item))
        .cloned()
        .collect();
    assert_eq!(
        together, catalog,
        "the single-class answers together must be the whole catalog, got {union:?}"
    );
}

/// Rule 3: `Family` and `MaxSize` split an instance type between them, over every EC2
/// capacity of every published item.
#[tokio::test]
async fn an_ec2_capacity_names_its_family_and_the_bare_size_of_it() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;
    let published = specified_instance_types();
    let mut checked = 0usize;

    for item in &catalog {
        let case = text(item, "CatalogItemId");
        let capacities = item
            .get("EC2Capacities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !capacities.is_empty(),
            "case {case:?}: a configuration carries EC2 capacity, got {item:?}"
        );
        for capacity in &capacities {
            let family = text(capacity, "Family");
            let max_size = text(capacity, "MaxSize");
            assert!(
                !max_size.is_empty() && !max_size.contains('.'),
                "case {case:?}: MaxSize must be the bare size suffix, got {capacity:?}"
            );
            assert!(
                !family.is_empty() && !family.contains('.'),
                "case {case:?}: Family must be the bare family prefix, got {capacity:?}"
            );
            let instance_type = format!("{family}.{max_size}");
            assert!(
                published.contains(&instance_type),
                "case {case:?}: Family and MaxSize must join into an instance type this \
                 service holds a specification of, got {instance_type:?} for {capacity:?}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > ITEM_CLASSES.len(),
        "the catalog must publish capacities to check, got {checked}"
    );
}

/// Rule 4: the single-item read answers the same specification as the listing, for
/// EVERY SKU the catalog publishes.
#[tokio::test]
async fn the_single_item_read_answers_the_same_specification() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;

    for item in &catalog {
        let sku = text(item, "CatalogItemId");
        let read = outposts
            .ok("GET", &format!("/catalog/item/{}", segment(&sku)), None)
            .await["CatalogItem"]
            .clone();
        assert_eq!(
            &read, item,
            "case {sku:?}: the single-item read must answer the item the listing reports, \
             member for member"
        );
        for member in PUBLISHED_FOR_EVERY_ITEM {
            assert!(
                read.get(member).is_some(),
                "case {sku:?}: the read must report {member}, got {read:?}"
            );
        }
    }
}
