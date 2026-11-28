// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the four reads over the catalog of modules a flow can
//! attach: `ListAvailableModules`, `GetAvailableModule`,
//! `ListAvailableModuleVersions` and `GetAvailableModuleVersion`.
//!
//! The rules under test are about the catalog, not about one recorded request.
//! The four reads answer from ONE catalog, so every module a listing reports
//! resolves through its own reader with the same members, and so does every
//! version; a listing reports the modules newest first by `createdAt` and a
//! module's versions highest version first counted NUMERICALLY rather than
//! lexically; only `ACTIVE` and `DEPRECATED` versions are reported, so a status
//! filter naming any other member of the modelled `ModuleVersionStatus` reports
//! none; a module or a version the catalog does not hold is not found, in this
//! service's one not-found wording; and both listings page by the shared page
//! walk, reporting every entry exactly once.
//!
//! The tables span what those rules generalise over: every entry of the catalog
//! rather than one of them, every member of the modelled `SubscriptionType` and
//! of `ModuleVersionStatus` as a filter, each published bound of the modelled
//! `maxResults` range and a value above it, and both kinds of missing address.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_rtbfabric_rtbfabric::state::{
    listed_modules, AvailableModule, MODULE_VERSION_STATUS_ACTIVE,
    MODULE_VERSION_STATUS_DEPRECATED, RESOURCE_NOT_FOUND_MESSAGE, SUBSCRIPTION_TYPE_AWS_BUILT_IN,
};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The members of the modelled `ModuleVersionStatus`
/// (`smithy_query(query="shape", shape="ModuleVersionStatus")`), which is what a
/// status filter may name.
const MODULE_VERSION_STATUS_VALUES: &[&str] =
    &["CREATING", "CREATE_FAILED", "ACTIVE", "DEPRECATED"];

/// The members of the modelled `SubscriptionType`
/// (`smithy_query(query="shape", shape="SubscriptionType")`).
const SUBSCRIPTION_TYPE_VALUES: &[&str] = &["MARKETPLACE", "PRIVATE", "AWS_BUILT_IN"];

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// The entries of a reported list member.
fn entries(body: &Value, member: &str) -> Vec<Value> {
    body[member].as_array().cloned().unwrap_or_default()
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate ->
/// handle -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    /// Send one restJson1 GET and return `(status, the error code, parsed body)`.
    async fn get(&self, path: &str) -> (u16, String, Value) {
        let request = ::http::Request::builder()
            .method("GET")
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::new())
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, code, parsed)
    }

    /// Walk a paginated listing to exhaustion, `per_page` entries at a time, and
    /// answer with every entry it reported in the order it reported them.
    async fn walk(&self, case: &str, path: &str, member: &str, per_page: usize) -> Vec<Value> {
        let joiner = if path.contains('?') { '&' } else { '?' };
        let mut reported: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let page_path = match &next_token {
                Some(token) => format!("{path}{joiner}maxResults={per_page}&nextToken={token}"),
                None => format!("{path}{joiner}maxResults={per_page}"),
            };
            let (status, _, body) = self.get(&page_path).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {page_path:?} to answer 200, got {status} {body:?}"
            );
            let page = entries(&body, member);
            assert!(
                page.len() <= per_page,
                "case {case:?}: expected at most {per_page} entries on a page, got {page:?}"
            );
            reported.extend(page);
            pages += 1;
            assert!(
                pages <= 64,
                "case {case:?}: the walk of {path:?} did not terminate, {reported:?} so far"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => return reported,
            }
        }
    }
}

/// The ARN the modelled `ModuleArn` pattern spells for a catalog module in the
/// Region the caller addressed: no account field, since a catalog module belongs
/// to no account.
fn module_arn(module: &AvailableModule) -> String {
    format!("arn:aws:rtbfabric:{REGION}::modules/{}", module.module_id)
}

/// The ARN the modelled `ModuleVersionArn` pattern spells for one version.
fn module_version_arn(module: &AvailableModule, version: &str) -> String {
    format!(
        "arn:aws:rtbfabric:{REGION}::modules/{}/versions/{version}",
        module.module_id
    )
}

/// Every module the catalog reports resolves through its own reader with the same
/// members, newest first, and the subscription-type filter reports exactly the
/// modules held that way.
#[tokio::test]
async fn every_listed_module_resolves_through_its_own_reader() {
    let fabric = Fabric::new();
    let catalog = listed_modules();
    assert!(
        !catalog.is_empty(),
        "expected the catalog to hold at least one module"
    );

    let (status, _, body) = fabric.get("/available-modules").await;
    assert_eq!(status, 200, "ListAvailableModules: {status} {body:?}");
    let listed = entries(&body, "modules");
    assert_eq!(
        listed.len(),
        catalog.len(),
        "expected the listing to report every module of the catalog, got {listed:?}"
    );

    for (reported, module) in listed.iter().zip(catalog.iter()) {
        let case = module.module_id;
        assert_eq!(
            text(reported, "moduleId"),
            module.module_id,
            "case {case:?}: expected the listing to report the modules newest first, got {listed:?}"
        );
        assert_eq!(
            text(reported, "moduleArn"),
            module_arn(module),
            "case {case:?}: expected the modelled ModuleArn spelling, got {reported:?}"
        );
        assert_eq!(
            text(reported, "subscriptionType"),
            module.subscription_type,
            "case {case:?}: expected the listing to report the module's subscription type"
        );

        // The same module through its own reader: one catalog, so the members
        // agree, and the single-module reader adds the ones the summary leaves
        // out.
        let path = format!("/available-modules/{}", module.module_id);
        let (status, _, read) = fabric.get(&path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetAvailableModule on {path:?} to answer 200, got {status} {read:?}"
        );
        for member in [
            "moduleId",
            "moduleArn",
            "moduleName",
            "subscriptionType",
            "subscriptionStatus",
        ] {
            assert_eq!(
                read[member], reported[member],
                "case {case:?}: expected {member:?} to agree between the listing and the reader"
            );
        }
        assert!(
            read["createdAt"].is_number(),
            "case {case:?}: expected the reader to report a createdAt, got {read:?}"
        );
        assert_eq!(
            read["marketplaceProductId"].is_null(),
            module.marketplace_product_id.is_none(),
            "case {case:?}: a marketplaceProductId belongs to a MARKETPLACE module alone"
        );
        // A read reports the same catalog however often it is taken.
        let (_, _, again) = fabric.get(&path).await;
        assert_eq!(
            again, read,
            "case {case:?}: expected a second read of {path:?} to report the same module"
        );
    }

    for wanted in SUBSCRIPTION_TYPE_VALUES {
        let path = format!("/available-modules?subscriptionType={wanted}");
        let (status, _, body) = fabric.get(&path).await;
        assert_eq!(
            status, 200,
            "case {wanted:?}: expected {path:?} to answer 200, got {status} {body:?}"
        );
        let expected: Vec<String> = catalog
            .iter()
            .filter(|module| module.subscription_type == *wanted)
            .map(|module| module.module_id.to_string())
            .collect();
        let reported: Vec<String> = entries(&body, "modules")
            .iter()
            .map(|module| text(module, "moduleId"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {wanted:?}: expected the filter to report exactly the modules held that way"
        );
        if *wanted != SUBSCRIPTION_TYPE_AWS_BUILT_IN {
            assert!(
                reported.is_empty(),
                "case {wanted:?}: this emulator holds no module of that subscription type, got {reported:?}"
            );
        }
    }
}

/// A module's versions are reported highest version first counted numerically,
/// each resolves through its own reader, and a status filter reports exactly the
/// versions in that status — none at all for a status the readers do not report.
#[tokio::test]
async fn every_listed_version_resolves_through_its_own_reader() {
    let fabric = Fabric::new();

    for module in listed_modules() {
        let case = module.module_id;
        let path = format!("/available-modules/{}/versions", module.module_id);
        let (status, _, body) = fabric.get(&path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {path:?} to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            text(&body, "moduleId"),
            module.module_id,
            "case {case:?}: expected the listing to report the module it was addressed to"
        );
        let expected: Vec<String> = module
            .listed_versions()
            .iter()
            .map(|version| version.version.to_string())
            .collect();
        let reported: Vec<String> = entries(&body, "versions")
            .iter()
            .map(|version| text(version, "version"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the versions highest first, counted numerically"
        );

        for version in module.listed_versions() {
            let case = format!("{case}@{}", version.version);
            let path = format!(
                "/available-modules/{}/versions/{}",
                module.module_id, version.version
            );
            let (status, _, read) = fabric.get(&path).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {path:?} to answer 200, got {status} {read:?}"
            );
            assert_eq!(
                text(&read, "moduleVersionArn"),
                module_version_arn(module, version.version),
                "case {case:?}: expected the modelled ModuleVersionArn spelling, got {read:?}"
            );
            assert_eq!(
                text(&read, "status"),
                version.status,
                "case {case:?}: expected the reader to report the version's status"
            );
            assert!(
                text(&read, "status") == MODULE_VERSION_STATUS_ACTIVE
                    || text(&read, "status") == MODULE_VERSION_STATUS_DEPRECATED,
                "case {case:?}: only an attachable or deprecated version is reported, got {read:?}"
            );
            let permissions = entries(&read, "permissions");
            let actions = entries(&read, "configurableActions");
            assert!(
                (1..=16).contains(&permissions.len()) && (1..=16).contains(&actions.len()),
                "case {case:?}: expected the modelled 1..16 permissions and actions, got {read:?}"
            );
            assert!(
                read["createdAt"].is_number() && read["updatedAt"].is_number(),
                "case {case:?}: expected both instants to be reported, got {read:?}"
            );
        }

        for wanted in MODULE_VERSION_STATUS_VALUES {
            let path = format!(
                "/available-modules/{}/versions?status={wanted}",
                module.module_id
            );
            let (status, _, body) = fabric.get(&path).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{wanted:?}: expected {path:?} to answer 200, got {status} {body:?}"
            );
            let reported = entries(&body, "versions");
            let expected = module
                .listed_versions()
                .iter()
                .filter(|version| version.status == *wanted)
                .count();
            assert_eq!(
                reported.len(),
                expected,
                "case {case:?}/{wanted:?}: expected the filter to report the versions in that status, got {reported:?}"
            );
            for version in &reported {
                assert_eq!(
                    text(version, "status"),
                    *wanted,
                    "case {case:?}/{wanted:?}: expected every reported version to be in that status"
                );
            }
            if *wanted != MODULE_VERSION_STATUS_ACTIVE
                && *wanted != MODULE_VERSION_STATUS_DEPRECATED
            {
                assert!(
                    reported.is_empty(),
                    "case {case:?}/{wanted:?}: a status the readers do not report has no versions, got {reported:?}"
                );
            }
        }
    }
}

/// Both listings page by the shared walk: every entry is reported exactly once,
/// in the same order, whatever page size the caller asks for — at each published
/// bound of the modelled range and above it.
#[tokio::test]
async fn a_catalog_walk_reports_every_entry_once() {
    let fabric = Fabric::new();
    let catalog = listed_modules();
    let all_modules: Vec<String> = catalog
        .iter()
        .map(|module| module.module_id.to_string())
        .collect();
    let widest = catalog
        .iter()
        .max_by_key(|module| module.listed_versions().len())
        .expect("the catalog holds a module");
    let all_versions: Vec<String> = widest
        .listed_versions()
        .iter()
        .map(|version| version.version.to_string())
        .collect();

    // The modelled `maxResults` range is 1..10, so the rows are its minimum, a
    // value inside it and its maximum; one past that bound is refused before any
    // handler runs, which the row below the loop pins.
    for per_page in [1usize, 2, 10] {
        let case = format!("maxResults={per_page}");
        let reported: Vec<String> = fabric
            .walk(&case, "/available-modules", "modules", per_page)
            .await
            .iter()
            .map(|module| text(module, "moduleId"))
            .collect();
        assert_eq!(
            reported, all_modules,
            "case {case:?}: expected the walk to report every module once, in order"
        );

        let path = format!("/available-modules/{}/versions", widest.module_id);
        let reported: Vec<String> = fabric
            .walk(&case, &path, "versions", per_page)
            .await
            .iter()
            .map(|version| text(version, "version"))
            .collect();
        assert_eq!(
            reported, all_versions,
            "case {case:?}: expected the walk to report every version once, in order"
        );
    }

    // One past the modelled maximum is refused rather than paged, for both
    // listings, since the range is enforced before either handler runs.
    for path in [
        "/available-modules?maxResults=11".to_string(),
        format!(
            "/available-modules/{}/versions?maxResults=11",
            widest.module_id
        ),
    ] {
        let (status, code, body) = fabric.get(&path).await;
        assert_eq!(
            status, 400,
            "case {path:?}: expected a page size past the modelled maximum to be refused, got {status} {body:?}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {path:?}: expected the modelled client-input code, got {code:?}"
        );
    }
}

/// A module or a version the catalog does not hold is not found, in this
/// service's one not-found wording — and a version is addressed only through its
/// own module.
#[tokio::test]
async fn an_address_the_catalog_does_not_hold_is_not_found() {
    let fabric = Fabric::new();
    let held = listed_modules()[0];
    let version = held.listed_versions()[0].version;

    let rows: Vec<(&str, String)> = vec![
        (
            "a module id nothing published",
            "/available-modules/module-nothingpublishedthis".to_string(),
        ),
        (
            "the versions of a module nothing published",
            "/available-modules/module-nothingpublishedthis/versions".to_string(),
        ),
        (
            "a version of a module nothing published",
            format!("/available-modules/module-nothingpublishedthis/versions/{version}"),
        ),
        (
            "a version the module does not hold",
            format!("/available-modules/{}/versions/99.99.99", held.module_id),
        ),
    ];

    for (case, path) in rows {
        let (status, code, body) = fabric.get(&path).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {path:?} to answer 404, got {status} {body:?}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found code, got {code:?}"
        );
        assert_eq!(
            text(&body, "message"),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected this service's one not-found wording, got {body:?}"
        );
    }
}
