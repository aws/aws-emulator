// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseBlueprints` — the managed-database blueprint catalog,
//! driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRelationalDatabaseBlueprints` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over the WHOLE catalog and over a set of page
//! tokens rather than one recorded call:
//!
//! * a blueprint's id is its engine and major engine version joined with
//!   underscores (`mysql` + `8.0` -> `mysql_8_0`), its `engineVersion` is the full
//!   version within that major version, and its `engineVersionDescription` names
//!   the engine beside that version;
//! * `engine` is an engine the service serves managed databases for — MySQL and
//!   PostgreSQL, the engines the recorded catalog carries, though the modelled
//!   `RelationalDatabaseEngine` enum still names only `mysql` — and exactly one
//!   blueprint per engine is that engine's default;
//! * a page is the catalog from the entry `pageToken` names onward, bounded by the
//!   page size, with `nextPageToken` present only while entries remain — so the
//!   walk reports the catalog in order, exactly once each;
//! * a `pageToken` that names no blueprint is a modelled 400
//!   `InvalidInputException`; and
//! * the operation reports, never mutates: identical calls answer identically.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The operation under test.
const OPERATION: &str = "GetRelationalDatabaseBlueprints";

/// The page size the service publishes for this operation, read from the crate
/// rather than restated here, so a page that grows stays bounded by it.
const PAGE_SIZE: usize =
    service_com_amazonaws_lightsail_lightsail_20161128::state::RELATIONAL_DATABASE_BLUEPRINT_PAGE_SIZE;

/// The database engines Lightsail serves managed databases for, and so the
/// `engine` values a blueprint may report.
///
/// The modelled `RelationalDatabaseEngine` enum
/// (`smithy_query(query="shape",shape="RelationalDatabaseEngine")`) still names
/// only `mysql`, which the service has outgrown: Lightsail also serves
/// PostgreSQL managed databases, and the recorded
/// `GetRelationalDatabaseBlueprints` answer carries `postgres` blueprints beside
/// the MySQL one.
const SERVED_ENGINES: &[&str] = &["mysql", "postgres"];

/// One expected catalog entry.
///
/// Source: the engines and major versions the Lightsail user guide's "Database
/// engines in Amazon Lightsail" lists for a managed database — MySQL 8.4 and
/// PostgreSQL 13 through 18 — each at the patch release the service serves that
/// series at, with `mysql_8_4` the recorded first entry and MySQL's default
/// version. A managed PostgreSQL release is published as a revision of its
/// community version, so its version description ends with that revision label,
/// which a MySQL release carries none of. `postgres_18` is an entry the issue
/// cited nothing about.
struct Expected {
    blueprint_id: &'static str,
    engine: &'static str,
    major_version: &'static str,
    engine_version: &'static str,
    engine_version_description: &'static str,
    is_engine_default: bool,
}

const EXPECTED_CATALOG: &[Expected] = &[
    Expected {
        blueprint_id: "mysql_8_4",
        engine: "mysql",
        major_version: "8.4",
        engine_version: "8.4.11",
        engine_version_description: "MySQL 8.4.11",
        is_engine_default: true,
    },
    Expected {
        blueprint_id: "postgres_13",
        engine: "postgres",
        major_version: "13",
        engine_version: "13.23-rds.20260514",
        engine_version_description: "PostgreSQL 13.23-rds.20260514-R1",
        is_engine_default: false,
    },
    Expected {
        blueprint_id: "postgres_14",
        engine: "postgres",
        major_version: "14",
        engine_version: "14.24",
        engine_version_description: "PostgreSQL 14.24-R1",
        is_engine_default: false,
    },
    Expected {
        blueprint_id: "postgres_15",
        engine: "postgres",
        major_version: "15",
        engine_version: "15.19",
        engine_version_description: "PostgreSQL 15.19-R1",
        is_engine_default: false,
    },
    Expected {
        blueprint_id: "postgres_16",
        engine: "postgres",
        major_version: "16",
        engine_version: "16.15",
        engine_version_description: "PostgreSQL 16.15-R1",
        is_engine_default: false,
    },
    Expected {
        blueprint_id: "postgres_17",
        engine: "postgres",
        major_version: "17",
        engine_version: "17.11",
        engine_version_description: "PostgreSQL 17.11-R1",
        is_engine_default: false,
    },
    Expected {
        blueprint_id: "postgres_18",
        engine: "postgres",
        major_version: "18",
        engine_version: "18.6",
        engine_version_description: "PostgreSQL 18.6-R1",
        is_engine_default: true,
    },
];

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "case {operation}: must succeed, got {parsed:?}");
        parsed
    }

    /// One page of the catalog: the page's blueprints and its `nextPageToken`.
    async fn page(&self, request: Value) -> (Vec<Value>, Option<String>) {
        let case = request.clone();
        let body = self.ok(OPERATION, request).await;
        let blueprints = body["blueprints"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a blueprints array, got {body:?}")
            })
            .clone();
        let next = body["nextPageToken"].as_str().map(str::to_string);
        (blueprints, next)
    }

    /// Every blueprint the walk from `request` reports, in page order, together
    /// with the size of each page it took.
    async fn walk(&self, request: Value) -> (Vec<Value>, Vec<usize>) {
        let mut blueprints = Vec::new();
        let mut page_sizes = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = request.clone();
            if let Some(token) = &token {
                body["pageToken"] = json!(token);
            }
            let (page, next) = self.page(body).await;
            page_sizes.push(page.len());
            blueprints.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                page_sizes.len() < 100,
                "case {request:?}: the walk did not terminate after {} pages",
                page_sizes.len()
            );
        }
        (blueprints, page_sizes)
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A member of a blueprint, as a string, or a panic naming the case.
fn member(blueprint: &Value, name: &str) -> String {
    blueprint[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {blueprint:?}: expected a string {name}"))
        .to_string()
}

#[tokio::test]
async fn every_blueprint_derives_its_id_and_descriptions_from_its_engine_and_version() {
    let emulator = Emulator::new();
    let (blueprints, _) = emulator.walk(json!({})).await;

    assert!(
        !blueprints.is_empty(),
        "case whole catalog: expected at least one blueprint, got none"
    );

    for blueprint in &blueprints {
        let case = member(blueprint, "blueprintId");
        let engine = member(blueprint, "engine");
        let engine_version = member(blueprint, "engineVersion");
        let engine_description = member(blueprint, "engineDescription");
        let engine_version_description = member(blueprint, "engineVersionDescription");

        assert!(
            SERVED_ENGINES.contains(&engine.as_str()),
            "case {case:?}: engine must be an engine this service serves \
             ({SERVED_ENGINES:?}), got {engine:?}"
        );

        // The id names the engine and the MAJOR version, dots written as
        // underscores; `engineVersion` is the full version inside that major
        // version. How many components the major version has is the engine's
        // business — MySQL's is `8.4`, PostgreSQL's is `17` — so the major
        // version is read off the id rather than assumed.
        let major_version = case
            .strip_prefix(&format!("{engine}_"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: blueprintId must begin with its engine {engine:?}")
            })
            .replace('_', ".");
        assert!(
            !major_version.is_empty(),
            "case {case:?}: blueprintId must name a major version after its engine"
        );
        assert!(
            engine_version.starts_with(&format!("{major_version}.")),
            "case {case:?}: expected engineVersion inside major version \
             {major_version:?}, got {engine_version:?}"
        );

        assert!(
            !engine_description.is_empty(),
            "case {case:?}: expected a non-empty engineDescription, got {engine_description:?}"
        );
        assert!(
            engine_version_description.contains(&engine_version)
                && engine_version_description.len() > engine_version.len(),
            "case {case:?}: expected engineVersionDescription to name the engine beside \
             {engine_version:?}, got {engine_version_description:?}"
        );
        assert!(
            blueprint["isEngineDefault"].is_boolean(),
            "case {case:?}: expected a boolean isEngineDefault, got {blueprint:?}"
        );
    }
}

#[tokio::test]
async fn exactly_one_blueprint_per_engine_is_the_engine_default() {
    let emulator = Emulator::new();
    let (blueprints, _) = emulator.walk(json!({})).await;

    for engine in SERVED_ENGINES {
        let defaults: Vec<String> = blueprints
            .iter()
            .filter(|blueprint| member(blueprint, "engine") == *engine)
            .filter(|blueprint| blueprint["isEngineDefault"] == json!(true))
            .map(|blueprint| member(blueprint, "blueprintId"))
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "case engine {engine:?}: exactly one blueprint must be the engine default, got {defaults:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_reports_every_published_blueprint_once() {
    let emulator = Emulator::new();
    let (blueprints, page_sizes) = emulator.walk(json!({})).await;

    let reported: Vec<String> = blueprints
        .iter()
        .map(|blueprint| member(blueprint, "blueprintId"))
        .collect();
    let unique: BTreeSet<&String> = reported.iter().collect();
    assert_eq!(
        unique.len(),
        reported.len(),
        "case whole catalog: the walk reported a blueprint twice: {reported:?} over pages {page_sizes:?}"
    );

    // The catalog is exactly the published entries, in the order AWS reports
    // them: the MySQL blueprint first, then PostgreSQL by ascending major
    // version.
    let expected_ids: Vec<&str> = EXPECTED_CATALOG
        .iter()
        .map(|want| want.blueprint_id)
        .collect();
    assert_eq!(
        reported, expected_ids,
        "case whole catalog: expected the published catalog in report order, got {reported:?}"
    );

    for want in EXPECTED_CATALOG {
        let found = blueprints
            .iter()
            .find(|blueprint| member(blueprint, "blueprintId") == want.blueprint_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the catalog to carry it, got {reported:?}",
                    want.blueprint_id
                )
            });
        let case = want.blueprint_id;

        // The id derivation itself, asserted from the row's own (engine, major
        // version) rather than read back off the id: `mysql` + `8.4` ->
        // `mysql_8_4`, `postgres` + `13` -> `postgres_13`.
        let want_id = format!("{}_{}", want.engine, want.major_version.replace('.', "_"));
        assert_eq!(
            case, want_id,
            "case {case:?}: the id must be the engine and major version joined with underscores"
        );
        assert_eq!(
            member(found, "engine"),
            want.engine,
            "case {case:?}: wrong engine, got {found:?}"
        );
        assert_eq!(
            member(found, "engineVersion"),
            want.engine_version,
            "case {case:?}: wrong engineVersion, got {found:?}"
        );
        assert_eq!(
            member(found, "engineVersionDescription"),
            want.engine_version_description,
            "case {case:?}: wrong engineVersionDescription, got {found:?}"
        );
        assert_eq!(
            found["isEngineDefault"],
            json!(want.is_engine_default),
            "case {case:?}: wrong isEngineDefault, got {found:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_resumes_the_catalog_at_the_blueprint_it_names() {
    let emulator = Emulator::new();
    let (whole, page_sizes) = emulator.walk(json!({})).await;
    let ids: Vec<String> = whole
        .iter()
        .map(|blueprint| member(blueprint, "blueprintId"))
        .collect();

    // Each page is bounded by the published page size, and `nextPageToken` is
    // present exactly while entries remain: so every page but the last is full.
    for (index, size) in page_sizes.iter().enumerate() {
        assert!(
            *size <= PAGE_SIZE,
            "case page {index}: a page carries at most {PAGE_SIZE} blueprints, got {size}"
        );
        let is_last = index + 1 == page_sizes.len();
        if !is_last {
            assert_eq!(
                *size, PAGE_SIZE,
                "case page {index}: a page followed by another must be full, got {size}"
            );
        }
    }
    assert_eq!(
        ids.len(),
        page_sizes.iter().sum::<usize>(),
        "case paging: the pages must account for every blueprint, got {page_sizes:?} for {ids:?}"
    );

    // Every blueprint id is a token, including the first and the last: the page it
    // opens is the catalog suffix from there.
    for (index, token) in ids.iter().enumerate() {
        let (suffix, _) = emulator.walk(json!({"pageToken": token})).await;
        let suffix_ids: Vec<String> = suffix
            .iter()
            .map(|blueprint| member(blueprint, "blueprintId"))
            .collect();
        assert_eq!(
            suffix_ids,
            ids[index..],
            "case pageToken {token:?}: expected the catalog from there on, got {suffix_ids:?}"
        );
    }

    // An absent token and an empty one both ask for the first page.
    for request in [json!({}), json!({"pageToken": ""})] {
        let (page, _) = emulator.page(request.clone()).await;
        let page_ids: Vec<String> = page
            .iter()
            .map(|blueprint| member(blueprint, "blueprintId"))
            .collect();
        assert_eq!(
            page_ids,
            ids[..page_sizes[0]],
            "case {request:?}: expected the first page, got {page_ids:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_that_names_no_blueprint_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a token can fail to name a catalog entry: an
    // engine that is not served, a major version that is not, a full version where
    // a major one belongs, a dotted id, and an opaque cursor from elsewhere.
    for token in [
        "mariadb_11",
        "mysql_9_9",
        "postgres_9_6",
        "mysql_8_4_11",
        "mysql.8.4",
        "AAAAAAAA",
        " ",
    ] {
        let (status, body) = emulator.call(OPERATION, json!({"pageToken": token})).await;
        assert_eq!(
            status, 400,
            "case pageToken {token:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case pageToken {token:?}: expected the modelled InvalidInputException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let first = emulator.ok(OPERATION, json!({})).await;
    let second = emulator.ok(OPERATION, json!({})).await;
    let after_token_walk = {
        let _ = emulator.walk(json!({})).await;
        emulator.ok(OPERATION, json!({})).await
    };

    assert_eq!(
        first, second,
        "case repeated read: the catalog must answer identically, got {first:?} then {second:?}"
    );
    assert_eq!(
        first, after_token_walk,
        "case read after a paged walk: the catalog must answer identically, got {first:?} then {after_token_walk:?}"
    );
}
