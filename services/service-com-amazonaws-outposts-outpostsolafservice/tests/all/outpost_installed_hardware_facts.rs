// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two members an Outpost reports ABOUT THE HARDWARE
//! installed on it — `Outpost$Generation` and `Outpost$RackScalingType` — driven over
//! the wire through the same front door as [`crate::site_and_outpost_lifecycle`]
//! (whose harness this suite reuses).
//!
//! Where [`crate::outpost_hardware_installation`] states WHICH units an order's
//! fulfilment installs on an Outpost, this suite states what the Outpost then reports
//! about the configuration those units are. The rules under test, each stated once and
//! then exercised over a SET of inputs:
//!
//! 1. `Generation` is "the Outpost generation" and `RackScalingType` "the rack scaling
//!    type": both are facts of the hardware standing on the Outpost, so an Outpost
//!    reports the generation and the rack scaling type the catalog publishes for the
//!    configuration an order placed on it delivered — the configuration its uncancelled
//!    orders name that is published in the class its form factor names — and reports
//!    neither while nothing is installed on it. A configuration rack scaling does not
//!    apply to publishes no scaling type, so an Outpost installed with it reports none.
//! 2. Every read of an Outpost reports the same pair: `GetOutpost`, `ListOutposts` and
//!    the `UpdateOutpost` that answers the Outpost it updated.
//! 3. The facts come out with the hardware they are facts OF: a form factor that
//!    replaces an Outpost's configuration leaves both absent until the install it owes
//!    falls due, and that install reports the facts of the configuration it placed.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::catalog_item_catalog;

use crate::site_and_outpost_lifecycle::{
    activate_outpost, deliver_orders, orderable_site, segment, text, Outposts,
};

/// Both values the modelled `SupportedHardwareType` enum declares
/// (`smithy_query(query="shape",shape="SupportedHardwareType")`), which are also the
/// two classes the modelled `CatalogItemClass` declares — the form factor an Outpost
/// carries and the class a configuration is published as name the same hardware.
const HARDWARE_TYPES: &[&str] = &["RACK", "SERVER"];

/// The `CatalogItemStatus` a configuration that can still be ordered is published in,
/// from the modelled `CatalogItemStatus` enum.
const ITEM_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The values `Outpost$Generation` documents as valid (`GENERATION_1` for
/// first-generation rack deployments and `GENERATION_2` for second-generation rack
/// deployments), as
/// `smithy_query(query="doc",shape="Outpost",member="Generation")` reports them.
const OUTPOST_GENERATIONS: &[&str] = &["GENERATION_1", "GENERATION_2"];

/// The values `Outpost$RackScalingType` documents as valid (`SINGLE_RACK` for
/// single-rack Outposts and `MULTI_RACK` for multi-rack Outposts that can expand across
/// multiple racks), from the same model.
const RACK_SCALING_TYPES: &[&str] = &["SINGLE_RACK", "MULTI_RACK"];

/// A `SkuCode` of the modelled form (`^OR-[A-Z0-9]{7}$`) this service publishes no
/// configuration under, so an order naming it names hardware facts the service does not
/// hold.
const UNPUBLISHED_SKU: &str = "OR-1A2B3C4";

/// One published hardware configuration, as a case about an Outpost's hardware needs
/// it: the `CatalogItemId` an order names it by, the class it is published as, the rack
/// scaling type it publishes (absent for a configuration rack scaling does not apply
/// to) and the generation of its hardware.
#[derive(Debug, Clone)]
struct Configuration {
    catalog_item_id: String,
    item_class: String,
    rack_scaling_type: Option<String>,
    generation: String,
}

/// A string member, or `None` when the wire omits it — which is what distinguishes a
/// member an Outpost reports no value for from one it reports.
fn optional(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

/// The pair one read of an Outpost reports about the hardware installed on it:
/// `(Generation, RackScalingType)`.
fn facts(outpost: &Value) -> (Option<String>, Option<String>) {
    (optional(outpost, "Generation"), optional(outpost, "RackScalingType"))
}

/// Assert that `outpost` reports exactly `wanted` as its hardware facts, and that each
/// value it reports at all is one the model declares.
fn assert_facts(outpost: &Value, wanted: &(Option<String>, Option<String>), case: &str) {
    let reported = facts(outpost);
    assert_eq!(
        &reported, wanted,
        "case {case:?}: an Outpost reports the generation and rack scaling type of the \
         configuration installed on it, expected {wanted:?}, got {outpost}"
    );
    if let Some(generation) = &reported.0 {
        assert!(
            OUTPOST_GENERATIONS.contains(&generation.as_str()),
            "case {case:?}: a reported Generation must be a value the model declares, got \
             {generation:?}"
        );
    }
    if let Some(scaling) = &reported.1 {
        assert!(
            RACK_SCALING_TYPES.contains(&scaling.as_str()),
            "case {case:?}: a reported RackScalingType must be a value the model declares, got \
             {scaling:?}"
        );
    }
}

/// Every hardware configuration this service publishes as still ORDERABLE.
///
/// The id, the class and the rack scaling type are read over the WIRE, from
/// `ListCatalogItems`, so what a case expects of an Outpost is what the catalog
/// publishes for the configuration it was ordered from. The generation is read from the
/// service's own catalog table, because the modelled `CatalogItem` carries no
/// generation member at all — the same seam
/// [`crate::instance_type_catalogs::specifications`] reads the specification table
/// every expectation about an instance SIZE is derived from.
async fn orderable_configurations(outposts: &Outposts) -> Vec<Configuration> {
    let generations: BTreeMap<String, String> = catalog_item_catalog()
        .into_iter()
        .filter_map(|record| {
            record
                .item
                .catalog_item_id
                .clone()
                .map(|catalog_item_id| (catalog_item_id, record.generation.to_string()))
        })
        .collect();
    let published = outposts
        .ok("GET", "/catalog/items?MaxResults=1000", None)
        .await;
    let items = published
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let configurations: Vec<Configuration> = items
        .iter()
        .filter(|item| text(item, "ItemStatus") == ITEM_STATUS_AVAILABLE)
        .map(|item| {
            let catalog_item_id = text(item, "CatalogItemId");
            let generation = generations
                .get(&catalog_item_id)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "the published item {catalog_item_id:?} must be one of the service's \
                         own configurations"
                    )
                });
            Configuration {
                item_class: text(item, "ItemClass"),
                rack_scaling_type: optional(item, "RackScalingType"),
                generation,
                catalog_item_id,
            }
        })
        .collect();

    // The rows below span the class only if the catalog does: both generations, both
    // scaling types, and a configuration rack scaling does not apply to.
    let spanned: BTreeSet<(String, String, Option<String>)> = configurations
        .iter()
        .map(|config| {
            (
                config.item_class.clone(),
                config.generation.clone(),
                config.rack_scaling_type.clone(),
            )
        })
        .collect();
    for generation in OUTPOST_GENERATIONS {
        assert!(
            spanned
                .iter()
                .any(|(_, published, _)| published == generation),
            "the catalog must publish an orderable configuration of every modelled \
             generation for these rows to span the class, got {spanned:?}"
        );
    }
    for scaling in RACK_SCALING_TYPES {
        assert!(
            spanned
                .iter()
                .any(|(_, _, published)| published.as_deref() == Some(scaling)),
            "the catalog must publish an orderable configuration of every modelled rack \
             scaling type, got {spanned:?}"
        );
    }
    assert!(
        spanned.iter().any(|(_, _, published)| published.is_none()),
        "the catalog must publish a configuration rack scaling does not apply to, got \
         {spanned:?}"
    );
    configurations
}

/// One site and one Outpost on it carrying `form_factor` (or none at all), as that
/// Outpost's `OutpostId`. An Outpost belongs to a site, so the site comes first.
async fn an_outpost(outposts: &Outposts, name: &str, form_factor: Option<&str>) -> String {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(form_factor) = form_factor {
        request["SupportedHardwareType"] = json!(form_factor);
    }
    let outpost = outposts.create_outpost(&request).await;
    text(&outpost, "OutpostId")
}

/// `CreateOrder` for one unit of `catalog_item_id` on `outpost_id`, requiring HTTP 200.
async fn place_order(outposts: &Outposts, outpost_id: &str, catalog_item_id: &str) {
    outposts
        .ok(
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": catalog_item_id, "Quantity": 1 }],
            })),
        )
        .await;
}

/// The Outpost `outpost_id` as `ListOutposts` reports it in this region's inventory.
async fn listed_outpost(outposts: &Outposts, outpost_id: &str) -> Value {
    let answer = outposts.list_outposts("?MaxResults=1000").await;
    answer
        .get("Outposts")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| text(item, "OutpostId") == outpost_id)
                .cloned()
        })
        .unwrap_or_else(|| panic!("ListOutposts must report the Outpost {outpost_id}: {answer}"))
}

/// `UpdateOutpost` recording `request` on `outpost_id`, as the Outpost it answered with.
async fn update_outpost(outposts: &Outposts, outpost_id: &str, request: &Value) -> Value {
    outposts
        .ok(
            "PATCH",
            &format!("/outposts/{}", segment(outpost_id)),
            Some(request),
        )
        .await["Outpost"]
        .clone()
}

/// The hardware one Outpost holds, as `ListAssets` reports it.
async fn installed(outposts: &Outposts, outpost_id: &str) -> Vec<Value> {
    outposts
        .ok(
            "GET",
            &format!("/outposts/{}/assets", segment(outpost_id)),
            None,
        )
        .await["Assets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Rules 1 and 2: over every orderable configuration the catalog publishes, and both
/// form factors an Outpost can carry, what the Outpost reports about its hardware
/// before an order is placed, while that order is in progress, and once the hardware it
/// delivered is installed — through each of the three reads that answer an Outpost.
///
/// The rows differ along everything the rule generalises over: the generation of the
/// configuration ordered, its rack scaling type (both modelled values and the
/// configuration rack scaling does not apply to), the class it is published as, and
/// whether the form factor of the Outpost it was ordered for names that class at all.
#[tokio::test]
async fn an_outpost_reports_the_published_facts_of_the_configuration_installed_on_it() {
    let outposts = Outposts::new();
    let catalog = orderable_configurations(&outposts).await;

    /// One row: the Outpost, and what it must report once its order is installed.
    struct Row {
        case: String,
        outpost_id: String,
        wanted: (Option<String>, Option<String>),
    }

    let mut rows: Vec<Row> = Vec::new();
    for config in &catalog {
        for form_factor in HARDWARE_TYPES {
            let case = format!(
                "a {form_factor} Outpost ordered from the {} configuration {} ({}, {:?})",
                config.item_class,
                config.catalog_item_id,
                config.generation,
                config.rack_scaling_type,
            );
            let outpost_id = an_outpost(
                &outposts,
                &format!("facts-{}-{form_factor}", config.catalog_item_id),
                Some(form_factor),
            )
            .await;
            // Nothing is installed yet, so there is no hardware to report the facts of.
            assert_facts(
                &outposts.get_outpost(&outpost_id).await,
                &(None, None),
                &format!("{case}, before the order"),
            );
            place_order(&outposts, &outpost_id, &config.catalog_item_id).await;
            // The order is placed; the hardware has not reached the site.
            assert_facts(
                &outposts.get_outpost(&outpost_id).await,
                &(None, None),
                &format!("{case}, with the order still in progress"),
            );
            // The configuration describes the hardware the Outpost holds only when it
            // is published in the class that Outpost's form factor names.
            let installed_here = *form_factor == config.item_class;
            rows.push(Row {
                case,
                outpost_id,
                wanted: match installed_here {
                    true => (
                        Some(config.generation.clone()),
                        config.rack_scaling_type.clone(),
                    ),
                    false => (None, None),
                },
            });
        }
    }
    assert!(
        rows.iter().any(|row| row.wanted.0.is_some()),
        "the rows must include a configuration the Outpost that ordered it is installed with"
    );

    deliver_orders(&outposts);

    for row in &rows {
        let case = &row.case;
        let outpost = outposts.get_outpost(&row.outpost_id).await;
        let status = text(&outpost, "LifeCycleStatus");
        assert_eq!(
            status, "ACTIVE",
            "case {case:?}: the Outpost must have settled with its ordered hardware, got \
             {outpost}"
        );
        assert_facts(&outpost, &row.wanted, case);
        // Rule 2: the other two reads of an Outpost answer the same pair.
        assert_facts(
            &listed_outpost(&outposts, &row.outpost_id).await,
            &row.wanted,
            &format!("{case}, as ListOutposts reports it"),
        );
        let patched = update_outpost(
            &outposts,
            &row.outpost_id,
            &json!({ "Description": "installed" }),
        )
        .await;
        assert_facts(
            &patched,
            &row.wanted,
            &format!("{case}, as the update answers it"),
        );
        // Reporting the facts changes nothing a later caller sees.
        assert_facts(
            &outposts.get_outpost(&row.outpost_id).await,
            &row.wanted,
            &format!("{case}, read again"),
        );
    }
}

/// The boundary of rule 1: the facts are the facts of hardware, so neither member ever
/// appears for an Outpost holding none, nor for hardware whose configuration this
/// service publishes no facts for.
///
/// The rows differ along why the Outpost holds no published configuration: nobody
/// ordered anything, the order named a SKU the catalog publishes nothing under, and the
/// Outpost's form factor names no configuration this service installs at all.
#[tokio::test]
async fn an_outpost_holding_no_published_configuration_reports_neither_fact() {
    let outposts = Outposts::new();
    let catalog = orderable_configurations(&outposts).await;
    let racked = catalog
        .iter()
        .find(|config| config.item_class == "RACK")
        .expect("the catalog must publish an orderable rack configuration")
        .clone();

    // An Outpost nobody has ordered hardware for: no install is owed at all.
    let unordered = an_outpost(&outposts, "facts-unordered", Some("RACK")).await;
    // An order for a configuration this service publishes nothing under.
    let unpublished = an_outpost(&outposts, "facts-unpublished-sku", Some("RACK")).await;
    place_order(&outposts, &unpublished, UNPUBLISHED_SKU).await;
    // An Outpost that recorded no form factor names no configuration to install.
    let formless = an_outpost(&outposts, "facts-no-form-factor", None).await;
    place_order(&outposts, &formless, &racked.catalog_item_id).await;

    deliver_orders(&outposts);

    let rows: &[(&str, &String, &str, bool)] = &[
        (
            "an Outpost nobody ordered hardware for",
            &unordered,
            "PENDING",
            false,
        ),
        (
            "an order naming a SkuCode the catalog publishes nothing under",
            &unpublished,
            "ACTIVE",
            true,
        ),
        (
            "an Outpost whose form factor names no configuration",
            &formless,
            "ACTIVE",
            false,
        ),
    ];
    for (case, outpost_id, status, holds_hardware) in rows {
        let outpost = outposts.get_outpost(outpost_id).await;
        let reported = text(&outpost, "LifeCycleStatus");
        assert_eq!(
            &reported, status,
            "case {case:?}: expected the lifecycle status {status:?}, got {outpost}"
        );
        let held = installed(&outposts, outpost_id).await;
        assert_eq!(
            !held.is_empty(),
            *holds_hardware,
            "case {case:?}: expected the Outpost to hold hardware: {holds_hardware}, got {held:?}"
        );
        assert_facts(&outpost, &(None, None), case);
        assert_facts(
            &listed_outpost(&outposts, outpost_id).await,
            &(None, None),
            &format!("{case}, as ListOutposts reports it"),
        );
    }
}

/// Rule 3: the facts travel with the hardware they are facts of — a form factor that
/// replaces an Outpost's configuration takes them out with it, and the install that
/// places the newly named configuration reports that configuration's own.
///
/// The rows are the states one Outpost passes through: installed with the rack
/// configuration its order named, given the server form factor (so the rack comes out),
/// settled on the server configuration — which that order names none of, so the Outpost
/// publishes no facts of it — and given the rack form factor again, which installs the
/// configuration the order does name and reports its facts once more.
#[tokio::test]
async fn the_facts_come_out_with_the_hardware_a_new_form_factor_replaces() {
    let outposts = Outposts::new();
    let catalog = orderable_configurations(&outposts).await;
    let racked = catalog
        .iter()
        .find(|config| config.item_class == "RACK" && config.rack_scaling_type.is_some())
        .expect("the catalog must publish an orderable rack configuration")
        .clone();
    let rack_facts = (
        Some(racked.generation.clone()),
        racked.rack_scaling_type.clone(),
    );

    let outpost_id = an_outpost(&outposts, "facts-form-factor-change", Some("RACK")).await;
    place_order(&outposts, &outpost_id, &racked.catalog_item_id).await;
    deliver_orders(&outposts);
    assert_facts(
        &outposts.get_outpost(&outpost_id).await,
        &rack_facts,
        "the rack configuration the order delivered",
    );

    // The server form factor names other hardware, so the rack comes out and its facts
    // come out with it while the install the Outpost is now owed is outstanding.
    let patched = update_outpost(
        &outposts,
        &outpost_id,
        &json!({ "SupportedHardwareType": "SERVER" }),
    )
    .await;
    assert_facts(
        &patched,
        &(None, None),
        "an Outpost whose hardware is being replaced",
    );
    let held = installed(&outposts, &outpost_id).await;
    assert!(
        held.is_empty(),
        "an Outpost whose hardware is being installed holds no asset, got {held:?}"
    );

    // Bring that install due through the store seam the suites that age a stored
    // resource use, rather than waiting the span out.
    activate_outpost(&outposts, &outpost_id);
    let settled = outposts.get_outpost(&outpost_id).await;
    assert!(
        !installed(&outposts, &outpost_id).await.is_empty(),
        "the install the new form factor owed must have placed its hardware, got {settled}"
    );
    assert_facts(
        &settled,
        &(None, None),
        "a server configuration the Outpost's orders name none of",
    );

    // Naming the rack form factor again installs the configuration the order DOES name,
    // so the Outpost reports that configuration's facts once more.
    let patched = update_outpost(
        &outposts,
        &outpost_id,
        &json!({ "SupportedHardwareType": "RACK" }),
    )
    .await;
    assert_facts(
        &patched,
        &(None, None),
        "an Outpost whose hardware is being replaced again",
    );
    activate_outpost(&outposts, &outpost_id);
    assert_facts(
        &outposts.get_outpost(&outpost_id).await,
        &rack_facts,
        "the rack configuration installed a second time",
    );
}
