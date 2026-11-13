// SPDX-License-Identifier: CC0-1.0

//! `DescribePatchProperties`, end to end: the properties of the patches available for one
//! operating system.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call_in_region`]) dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router. Nothing reaches
//! into the state struct, so what a read reports is what the wire reports.
//!
//! The rule under test is that the read reports the values the patch metadata Systems
//! Manager publishes for the operating system a request names carries for the patch
//! property it names — each value once, in ascending order, carrying the product family a
//! product belongs to where that operating system publishes families, paged by
//! `MaxResults` and resumed at the value a page stopped on — and refuses a property that
//! operating system does not publish. The suites are tables whose rows
//! differ along what the rule generalises over: the operating system (every value of the
//! modelled enum), the property (every value of the modelled enum), the patch set, the
//! page size and the Region the endpoint is.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// The `OperatingSystem` values the model's enum declares
/// (`smithy_query(query="shape", shape="OperatingSystem")`).
const OPERATING_SYSTEMS: &[&str] = &[
    "ALMA_LINUX",
    "AMAZON_LINUX",
    "AMAZON_LINUX_2",
    "AMAZON_LINUX_2022",
    "AMAZON_LINUX_2023",
    "CENTOS",
    "DEBIAN",
    "MACOS",
    "ORACLE_LINUX",
    "RASPBIAN",
    "REDHAT_ENTERPRISE_LINUX",
    "ROCKY_LINUX",
    "SUSE",
    "UBUNTU",
    "WINDOWS",
];

/// The `PatchProperty` values the model's enum declares
/// (`smithy_query(query="shape", shape="PatchProperty")`).
const PATCH_PROPERTIES: &[&str] = &[
    "CLASSIFICATION",
    "MSRC_SEVERITY",
    "PRIORITY",
    "PRODUCT",
    "PRODUCT_FAMILY",
    "SEVERITY",
];

/// The property whose values are products, and so the only property whose entries carry a
/// product family.
const PRODUCT: &str = "PRODUCT";

/// The property whose values are product families.
const PRODUCT_FAMILY: &str = "PRODUCT_FAMILY";

/// The properties an RPM-based distribution publishes, from the operation's own doc list
/// (`smithy_query(query="doc", shape="DescribePatchProperties")`: "AMAZON_LINUX_2 Valid
/// properties: PRODUCT | CLASSIFICATION | SEVERITY", and the same three for CENTOS,
/// ORACLE_LINUX, REDHAT_ENTERPRISE_LINUX and SUSE).
const RPM_PROPERTIES: &[&str] = &["CLASSIFICATION", "PRODUCT", "SEVERITY"];

/// The properties a Debian-based distribution publishes, from the same list ("DEBIAN
/// Valid properties: PRODUCT | PRIORITY", "UBUNTU Valid properties: PRODUCT | PRIORITY").
const DEBIAN_PROPERTIES: &[&str] = &["PRIORITY", "PRODUCT"];

/// The properties macOS publishes, from the same list ("MACOS Valid properties: PRODUCT |
/// CLASSIFICATION").
const MACOS_PROPERTIES: &[&str] = &["CLASSIFICATION", "PRODUCT"];

/// The properties Windows publishes, from the same list ("WINDOWS Valid properties:
/// PRODUCT | PRODUCT_FAMILY | CLASSIFICATION | MSRC_SEVERITY").
const WINDOWS_PROPERTIES: &[&str] = &[
    "CLASSIFICATION",
    "MSRC_SEVERITY",
    "PRODUCT",
    "PRODUCT_FAMILY",
];

/// What each operating system publishes. The doc list above names the "major operating
/// system type"s; the four values the modelled enum adds beyond it take the properties of
/// the family they belong to — a Red Hat rebuild publishes what Red Hat Enterprise Linux
/// does, and Raspberry Pi OS what Debian does.
const PUBLISHED_PROPERTIES: &[(&str, &[&str])] = &[
    ("ALMA_LINUX", RPM_PROPERTIES),
    ("AMAZON_LINUX", RPM_PROPERTIES),
    ("AMAZON_LINUX_2", RPM_PROPERTIES),
    ("AMAZON_LINUX_2022", RPM_PROPERTIES),
    ("AMAZON_LINUX_2023", RPM_PROPERTIES),
    ("CENTOS", RPM_PROPERTIES),
    ("DEBIAN", DEBIAN_PROPERTIES),
    ("MACOS", MACOS_PROPERTIES),
    ("ORACLE_LINUX", RPM_PROPERTIES),
    ("RASPBIAN", DEBIAN_PROPERTIES),
    ("REDHAT_ENTERPRISE_LINUX", RPM_PROPERTIES),
    ("ROCKY_LINUX", RPM_PROPERTIES),
    ("SUSE", RPM_PROPERTIES),
    ("UBUNTU", DEBIAN_PROPERTIES),
    ("WINDOWS", WINDOWS_PROPERTIES),
];

/// The key every reported entry carries its value under, as the operation's own example
/// output spells it (`{"Properties": [{"Name": "AmazonLinux2018.03"}]}`).
const NAME_KEY: &str = "Name";

/// The key a reported product entry carries its family under
/// (`{"Name": "Office 2019", "ProductFamily": "Office"}`).
const PRODUCT_FAMILY_KEY: &str = "ProductFamily";

/// The published floor of this read's `MaxResults`
/// (`smithy_query(query="operation", operation="DescribePatchProperties")`: range 1..50).
const MAX_RESULTS_MIN: i64 = 1;

/// The published ceiling of the same member.
const MAX_RESULTS_MAX: i64 = 50;

/// What a `NextToken` this read never issued is refused with: fixed text for this class,
/// interpolating nothing from the request. Spelled out here rather than read from the
/// crate, so the wording itself is what the assertion checks.
const INVALID_NEXT_TOKEN_MESSAGE: &str = "The NextToken value is not valid";

/// What a property an operating system does not publish is refused with: the sentence
/// this service words a member carrying a value outside its legal set with, naming the
/// member and the values legal for the operating system the request carried — which for
/// this member are the properties that operating system publishes, in the order the
/// operation's doc lists them. Spelled out here rather than read from the crate, so the
/// wording itself is what the assertions check.
fn unsupported_property_message(operating_system: &str, property: &str) -> String {
    let published = PUBLISHED_PROPERTIES
        .iter()
        .find(|(name, _)| *name == operating_system)
        .map(|(_, published)| *published)
        .unwrap_or_else(|| panic!("every modelled operating system is in the table"));
    let in_doc_order: Vec<&str> = [
        PRODUCT,
        PRODUCT_FAMILY,
        "CLASSIFICATION",
        "MSRC_SEVERITY",
        "PRIORITY",
        "SEVERITY",
    ]
    .into_iter()
    .filter(|property| published.contains(property))
    .collect();
    format!(
        "Property {property} is invalid, valid values are {}",
        in_doc_order.join(", ")
    )
}

/// How many pages a walk may take before the test gives up: every property below reports
/// fewer values than this at the smallest page size the read serves, so a walk that
/// reaches it is looping rather than paging.
const MAX_PAGES: usize = 64;

/// One value a read reports: the value itself and the product family it carries, where
/// the catalogue publishes one for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    product_family: Option<String>,
}

/// The request body for one read: the two required members, plus the patch set when a row
/// names one.
fn request(operating_system: &str, property: &str, patch_set: Option<&str>) -> Value {
    let mut body = json!({ "OperatingSystem": operating_system, "Property": property });
    if let Some(patch_set) = patch_set {
        body["PatchSet"] = json!(patch_set);
    }
    body
}

/// One read at `region`'s endpoint, returning `(status, body)`.
async fn describe(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribePatchProperties", body).await
}

/// The entries a response reports, asserting the shape the model gives them: a list of
/// attribute maps, each carrying its value under `Name` and nothing beyond the product
/// family, whose values are distinct and ascending.
fn reported_entries(case: &str, response: &Value) -> Vec<Entry> {
    let reported = response["Properties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a read reports a Properties list, got {response}"));
    let mut entries = Vec::new();
    for entry in reported {
        let entry = entry.as_object().unwrap_or_else(|| {
            panic!("case {case}: every entry is a map of attributes, got {response}")
        });
        let name = entry
            .get(NAME_KEY)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case}: every entry carries its value under {NAME_KEY}, got {entry:?}")
            })
            .to_string();
        let product_family = entry.get(PRODUCT_FAMILY_KEY).map(|family| {
            family
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a {PRODUCT_FAMILY_KEY} is a string, got {entry:?}")
                })
                .to_string()
        });
        assert_eq!(
            entry.len(),
            1 + usize::from(product_family.is_some()),
            "case {case}: an entry carries its value and at most its product family, got \
             {entry:?}"
        );
        entries.push(Entry {
            name,
            product_family,
        });
    }
    assert!(
        entries
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name),
        "case {case}: the values are distinct and ascending, got {entries:?}"
    );
    entries
}

/// One read at [`REGION`], asserted to succeed, returning the entries it reports.
async fn entries_of(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Entry> {
    let (status, response) = describe(router, REGION, body).await;
    assert_eq!(
        status, 200,
        "case {case}: the read must succeed, got {status} {response}"
    );
    reported_entries(case, &response)
}

/// The values of those entries.
fn names(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.name.clone()).collect()
}

/// One read at [`REGION`], asserted to succeed, returning the values it reports.
async fn values_of(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<String> {
    names(&entries_of(router, case, body).await)
}

/// Every value the read reports for `body`, walked in pages of `page_size`.
async fn walked_values(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    page_size: i64,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut page_request = body.clone();
        page_request["MaxResults"] = json!(page_size);
        if let Some(token) = &token {
            page_request["NextToken"] = json!(token);
        }
        let (status, response) = describe(router, REGION, &page_request).await;
        assert_eq!(
            status, 200,
            "case {case}: a page must be served, got {status} {response}"
        );
        let page = names(&reported_entries(case, &response));
        assert!(
            i64::try_from(page.len()).expect("a page length fits an i64") <= page_size,
            "case {case}: a page carries at most {page_size} entries, got {page:?}"
        );
        match response["NextToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    i64::try_from(page.len()).expect("a page length fits an i64"),
                    page_size,
                    "case {case}: a page that hands back a token is full, got {page:?}"
                );
                assert!(
                    page.last().is_some_and(|last| last.as_str() < next),
                    "case {case}: the token is a value past the page it closes, got {next} \
                     after {page:?}"
                );
                collected.extend(page);
                token = Some(next.to_string());
            }
            None => {
                collected.extend(page);
                return collected;
            }
        }
    }
    panic!("case {case}: the walk did not reach a last page in {MAX_PAGES} pages")
}

/// A read reports values for exactly the properties its operating system publishes, in
/// the shape the model gives them, and reporting them changes nothing; a property that
/// operating system does not publish is refused.
#[tokio::test]
async fn every_operating_system_reports_the_properties_it_publishes() {
    let router = router();

    for operating_system in OPERATING_SYSTEMS {
        let (_, published) = PUBLISHED_PROPERTIES
            .iter()
            .find(|(name, _)| name == operating_system)
            .unwrap_or_else(|| panic!("every modelled operating system is in the table"));
        for property in PATCH_PROPERTIES {
            let case = format!("{operating_system} {property}");
            let body = request(operating_system, property, None);

            if !published.contains(property) {
                // A property this operating system's patches do not carry names nothing
                // to report, so the read is refused rather than served as nothing.
                let (status, response) = describe(&router, REGION, &body).await;
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, "ValidationException"),
                    "case {case}: {operating_system} publishes no {property}, so the read \
                     is refused, got {status} {response}"
                );
                assert_eq!(
                    error_message(&response),
                    unsupported_property_message(operating_system, property),
                    "case {case}: the refusal names the property and the operating \
                     system, got {response}"
                );
                assert!(
                    response["Properties"].is_null(),
                    "case {case}: a refusal reports no values, got {response}"
                );
                continue;
            }

            let entries = entries_of(&router, &case, &body).await;
            assert!(
                !entries.is_empty(),
                "case {case}: {operating_system} publishes {property}, so the read \
                 reports values for it, got {entries:?}"
            );

            // Only a product carries a product family, and only where the operating
            // system publishes families at all.
            let families_published = *property == PRODUCT && published.contains(&PRODUCT_FAMILY);
            for entry in &entries {
                assert_eq!(
                    entry.product_family.is_some(),
                    families_published,
                    "case {case}: an entry carries a product family only for a product of \
                     an operating system that publishes families, got {entry:?}"
                );
            }

            // A read: the catalogue is published rather than written by a call, so a
            // second read of the same property reports the same values.
            let again = entries_of(&router, &format!("{case} read twice"), &body).await;
            assert_eq!(
                again, entries,
                "case {case}: a second read reports the same values, got {again:?}"
            );
        }
    }
}

/// A product is reported with the family it belongs to, and the families the products
/// carry are exactly the ones the family property reports.
#[tokio::test]
async fn a_product_carries_the_family_it_belongs_to() {
    let router = router();

    for patch_set in [None, Some("OS"), Some("APPLICATION")] {
        let case = format!("WINDOWS products with patch set {patch_set:?}");
        let products = entries_of(&router, &case, &request("WINDOWS", PRODUCT, patch_set)).await;
        let mut carried: Vec<String> = products
            .iter()
            .map(|entry| {
                entry
                    .product_family
                    .clone()
                    .unwrap_or_else(|| panic!("case {case}: every product carries a family"))
            })
            .collect();
        carried.sort();
        carried.dedup();
        let reported = values_of(
            &router,
            &format!("WINDOWS families with patch set {patch_set:?}"),
            &request("WINDOWS", PRODUCT_FAMILY, patch_set),
        )
        .await;
        assert_eq!(
            carried, reported,
            "case {case}: the families the products carry are the ones the family \
             property reports, got {carried:?} against {reported:?}"
        );
    }

    // The family of one product of each patch set: the operating system's own patches
    // belong to the Windows family, and an Office release to the Office family
    // (`smithy_query(query="doc", shape="DescribeAvailablePatchesRequest",
    // member="Filters")`: `PRODUCT_FAMILY` sample values `Windows | Office`).
    for (patch_set, product, family) in [
        (Some("OS"), "WindowsServer2019", "Windows"),
        (Some("APPLICATION"), "Office 2019", "Office"),
    ] {
        let case = format!("{product} in patch set {patch_set:?}");
        let products = entries_of(&router, &case, &request("WINDOWS", PRODUCT, patch_set)).await;
        let entry = products
            .iter()
            .find(|entry| entry.name == product)
            .unwrap_or_else(|| panic!("case {case}: the catalogue publishes it, got {products:?}"));
        assert_eq!(
            entry.product_family.as_deref(),
            Some(family),
            "case {case}: it belongs to the {family} family, got {entry:?}"
        );
    }
}

/// The patch metadata is published in every Region, so the same read at another
/// endpoint reports the same values.
#[tokio::test]
async fn the_catalogue_is_published_at_every_endpoint() {
    let router = router();

    for (operating_system, property) in [
        ("WINDOWS", PRODUCT),
        ("AMAZON_LINUX_2023", "CLASSIFICATION"),
        ("UBUNTU", "PRIORITY"),
    ] {
        let case = format!("{operating_system} {property} in another Region");
        let body = request(operating_system, property, None);
        let here = entries_of(&router, &case, &body).await;
        let (status, response) = describe(&router, "eu-central-1", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: the read must succeed, got {status} {response}"
        );
        let elsewhere = reported_entries(&case, &response);
        assert_eq!(
            elsewhere, here,
            "case {case}: every Region's endpoint reports the same values, got \
             {elsewhere:?} against {here:?}"
        );
    }
}

/// The patch set selects WHICH Windows patches are reported — the operating system's own
/// or the Microsoft applications' — and is "not applicable for the Linux or macOS
/// operating systems".
#[tokio::test]
async fn the_patch_set_selects_the_windows_products_and_families() {
    let router = router();

    // A Windows request that names no patch set is about the operating system's own
    // patches: the set a request has to ASK for is the application one.
    for property in [PRODUCT, PRODUCT_FAMILY] {
        let case = format!("WINDOWS {property} with no patch set");
        let unnamed = values_of(&router, &case, &request("WINDOWS", property, None)).await;
        let operating_system_set = values_of(
            &router,
            &format!("WINDOWS {property} OS"),
            &request("WINDOWS", property, Some("OS")),
        )
        .await;
        let application_set = values_of(
            &router,
            &format!("WINDOWS {property} APPLICATION"),
            &request("WINDOWS", property, Some("APPLICATION")),
        )
        .await;
        assert_eq!(
            unnamed, operating_system_set,
            "case {case}: a request naming no patch set is served the OS set, got \
             {unnamed:?} against {operating_system_set:?}"
        );
        assert_ne!(
            application_set, operating_system_set,
            "case {case}: the application set is not the OS set, got {application_set:?}"
        );
    }

    // The family of the Windows operating-system patches, and the families of the
    // Microsoft applications beside them.
    let families = values_of(
        &router,
        "WINDOWS PRODUCT_FAMILY OS",
        &request("WINDOWS", PRODUCT_FAMILY, Some("OS")),
    )
    .await;
    assert_eq!(
        families,
        vec!["Windows".to_string()],
        "case WINDOWS PRODUCT_FAMILY OS: the operating system's patches are one family, \
         got {families:?}"
    );
    let application_families = values_of(
        &router,
        "WINDOWS PRODUCT_FAMILY APPLICATION",
        &request("WINDOWS", PRODUCT_FAMILY, Some("APPLICATION")),
    )
    .await;
    assert!(
        application_families.contains(&"Office".to_string()),
        "case WINDOWS PRODUCT_FAMILY APPLICATION: the Microsoft applications include the \
         Office family, got {application_families:?}"
    );

    // The patch set is not applicable to a Linux or macOS operating system, so naming one
    // changes nothing there.
    for operating_system in ["AMAZON_LINUX_2023", "UBUNTU", "MACOS"] {
        let case = format!("{operating_system} PRODUCT with a patch set");
        let unnamed = values_of(&router, &case, &request(operating_system, PRODUCT, None)).await;
        for patch_set in ["OS", "APPLICATION"] {
            let named = values_of(
                &router,
                &format!("{case} {patch_set}"),
                &request(operating_system, PRODUCT, Some(patch_set)),
            )
            .await;
            assert_eq!(
                named, unnamed,
                "case {case} {patch_set}: the patch set is not applicable here, got \
                 {named:?} against {unnamed:?}"
            );
        }
    }
}

/// The values a read reports are the ones the published metadata carries for that
/// operating system: products of that operating system, and the classifications,
/// severities and priorities its patches carry.
#[tokio::test]
async fn the_reported_values_are_the_ones_the_metadata_publishes() {
    let router = router();

    // (operating system, property, patch set, values the catalogue publishes)
    //
    // Each value is one the patch documentation names for that operating system: the
    // products from `Patch$Product` ("For example, WindowsServer2016 or
    // AmazonLinux2018.03") and the `DescribeAvailablePatches$Filters` sample values
    // (`PRODUCT`: `WindowsServer2012 | Office 2010 | MicrosoftDefenderAntivirus`;
    // `CLASSIFICATION`: `ServicePacks | SecurityUpdates | DefinitionUpdates`;
    // `MSRC_SEVERITY`: `Important | Moderate`), the severities from `Patch$Severity`
    // ("For example, CRITICAL or MODERATE") and the priorities a Debian-based
    // distribution publishes.
    let rows: &[(&str, &str, Option<&str>, &[&str])] = &[
        (
            "WINDOWS",
            PRODUCT,
            Some("OS"),
            &["WindowsServer2012", "WindowsServer2022"],
        ),
        (
            "WINDOWS",
            PRODUCT,
            Some("APPLICATION"),
            &["MicrosoftDefenderAntivirus", "Office 2019"],
        ),
        (
            "WINDOWS",
            "CLASSIFICATION",
            None,
            &["DefinitionUpdates", "SecurityUpdates", "ServicePacks"],
        ),
        (
            "WINDOWS",
            "MSRC_SEVERITY",
            None,
            &["Critical", "Important", "Moderate"],
        ),
        ("AMAZON_LINUX", PRODUCT, None, &["AmazonLinux2018.03"]),
        ("AMAZON_LINUX_2023", PRODUCT, None, &["AmazonLinux2023"]),
        (
            "REDHAT_ENTERPRISE_LINUX",
            "CLASSIFICATION",
            None,
            &["Bugfix", "Security"],
        ),
        (
            "REDHAT_ENTERPRISE_LINUX",
            "SEVERITY",
            None,
            &["Critical", "Moderate"],
        ),
        ("UBUNTU", "PRIORITY", None, &["Optional", "Required"]),
        ("DEBIAN", "PRIORITY", None, &["Important", "Standard"]),
        ("MACOS", "CLASSIFICATION", None, &["Security"]),
    ];

    for (operating_system, property, patch_set, published) in rows {
        let case = format!("{operating_system} {property} {patch_set:?}");
        let values = values_of(
            &router,
            &case,
            &request(operating_system, property, *patch_set),
        )
        .await;
        for value in *published {
            assert!(
                values.contains(&(*value).to_string()),
                "case {case}: the catalogue publishes {value}, got {values:?}"
            );
        }
    }
}

/// A page carries at most `MaxResults` values and the token resumes the walk, so paging
/// through a property covers every value it reports exactly once.
#[tokio::test]
async fn paging_walks_every_published_value_once() {
    let router = router();

    for (operating_system, property, patch_set) in [
        ("WINDOWS", PRODUCT, Some("OS")),
        ("WINDOWS", "CLASSIFICATION", None),
        ("WINDOWS", PRODUCT, Some("APPLICATION")),
        ("SUSE", "CLASSIFICATION", None),
        ("UBUNTU", "PRIORITY", None),
        ("AMAZON_LINUX_2023", PRODUCT, None),
    ] {
        let body = request(operating_system, property, patch_set);
        let whole = values_of(
            &router,
            &format!("{operating_system} {property} unpaged"),
            &body,
        )
        .await;
        // Both published bounds of `MaxResults` and sizes in between, so a page smaller
        // than, equal to and larger than the catalogue's own length is covered.
        for page_size in [MAX_RESULTS_MIN, 2, 3, MAX_RESULTS_MAX] {
            let case = format!("{operating_system} {property} at MaxResults={page_size}");
            let walked = walked_values(&router, &case, &body, page_size).await;
            assert_eq!(
                walked, whole,
                "case {case}: paging covers the same values in the same order, got \
                 {walked:?} against {whole:?}"
            );
        }
    }

    // The token is the value the next page starts at, so the page it resumes begins there.
    let body = request("WINDOWS", "CLASSIFICATION", None);
    let whole = values_of(&router, "WINDOWS CLASSIFICATION unpaged", &body).await;
    let mut first = body.clone();
    first["MaxResults"] = json!(2);
    let (status, response) = describe(&router, REGION, &first).await;
    assert_eq!(
        status, 200,
        "case the first page: it must be served, got {status} {response}"
    );
    let token = response["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case the first page: it hands back a token, got {response}"))
        .to_string();
    assert_eq!(
        Some(token.as_str()),
        whole.get(2).map(String::as_str),
        "case the first page: the token is the value the next page starts at, got \
         {token} against {whole:?}"
    );
    let mut second = body.clone();
    second["MaxResults"] = json!(2);
    second["NextToken"] = json!(token);
    let resumed = values_of(&router, "the resumed page", &second).await;
    assert_eq!(
        resumed,
        whole[2..4].to_vec(),
        "case the resumed page: it starts at the token's value, got {resumed:?}"
    );
}

/// A `MaxResults` outside the published range, and a `NextToken` this read never issued,
/// are refused — and the refusal reports nothing.
#[tokio::test]
async fn an_unserviceable_page_request_is_refused() {
    let router = router();

    for (case, max_results) in [
        ("one below the published floor", MAX_RESULTS_MIN - 1),
        ("one past the published ceiling", MAX_RESULTS_MAX + 1),
    ] {
        let mut body = request("WINDOWS", PRODUCT, None);
        body["MaxResults"] = json!(max_results);
        let (status, response) = describe(&router, REGION, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case a MaxResults {case}: expected 400 ValidationException, got {status} \
             {response}"
        );
    }

    // Each published bound of `MaxResults` is itself served.
    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let mut body = request("WINDOWS", PRODUCT, None);
        body["MaxResults"] = json!(max_results);
        let (status, response) = describe(&router, REGION, &body).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results} is inside the published range: it must be \
             served, got {status} {response}"
        );
    }

    // A token is the value the next page starts at, so a value the property does not
    // publish — including one another property does — was never issued here.
    for (case, token) in [
        ("a value no property publishes", "not-a-published-value"),
        ("a value of another property", "SecurityUpdates"),
    ] {
        let mut body = request("WINDOWS", PRODUCT, None);
        body["NextToken"] = json!(token);
        let (status, response) = describe(&router, REGION, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case a NextToken that is {case}: expected 400 ValidationException, got \
             {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case a NextToken that is {case}: the refusal reports the token is not valid, \
             got {response}"
        );
        assert!(
            response["Properties"].is_null(),
            "case a NextToken that is {case}: a refusal reports no values, got {response}"
        );
    }
}
