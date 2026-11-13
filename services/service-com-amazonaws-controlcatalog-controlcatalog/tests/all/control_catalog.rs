// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Control Catalog emulator: every one of the six modeled
//! operations, driven over HTTP through the same `MultiServiceRouter` the production
//! front door serves.
//!
//! The rules under test are catalog-wide rather than per-entry, so each test is a
//! TABLE whose rows are generated from the catalog the service serves (every list
//! operation, every domain, every objective, every control, every mapping type) rather
//! than from one entry picked by hand. Where a bound comes from the Smithy model, the
//! comment beside it names the shape the bound belongs to.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;
use service_com_amazonaws_controlcatalog_controlcatalog::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service: that
/// scope is how the router picks the service which claims a request. The signature
/// itself is never verified here — [`Emulator`] turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/controlcatalog/aws4_request, SignedHeaders=host, Signature=not-verified";

/// The five paginated list operations: route, the response member carrying the items,
/// and the maximum page size that operation's own `MaxResults` range declares
/// (`OPERATIONS.md` / the `MaxList*Results` shapes).
const LIST_OPERATIONS: [(&str, &str, u32); 5] = [
    ("/domains", "Domains", 100),
    ("/objectives", "Objectives", 100),
    ("/common-controls", "CommonControls", 100),
    ("/list-controls", "Controls", 100),
    ("/list-control-mappings", "ControlMappings", 1000),
];

/// A router with only this service mounted — one emulator "session".
///
/// `pub(crate)` so the sibling suites in this test target (see `tests/all.rs`) drive
/// the emulator through the same front door rather than standing up a second harness
/// of their own.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A session over an existing engine, so two sessions can share one catalog.
    pub(crate) fn with_engine(engine: &Arc<engine::InMemoryEngine>) -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(engine)));
        Self { router }
    }

    /// A session over a fresh engine.
    pub(crate) fn new() -> Self {
        Self::with_engine(&Arc::new(engine::InMemoryEngine::new()))
    }

    /// POST `path_and_query` with a JSON body; returns the status and parsed body.
    pub(crate) async fn post(&self, path_and_query: &str, body: &str) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://localhost{path_and_query}"))
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// POST and require HTTP 200.
    pub(crate) async fn ok(&self, path_and_query: &str, body: &str) -> Value {
        let (status, parsed) = self.post(path_and_query, body).await;
        assert_eq!(
            status, 200,
            "case {path_and_query} {body}: expected 200, got {status} with {parsed:?}"
        );
        parsed
    }
}

/// The wire error code of a failure response: the last segment of its `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Walk a list operation to its end, following `NextToken`, and return every item in
/// the order the service returned them.
///
/// Checks the pagination contract as it goes: no page exceeds the `MaxResults` asked
/// for, no item is returned twice, a `NextToken` never accompanies an empty page, and
/// the walk terminates on a page that carries no token.
pub(crate) async fn drain(
    emulator: &Emulator,
    path: &str,
    items_key: &str,
    max_results: Option<u32>,
    body: &str,
) -> Vec<Value> {
    /// More pages than the catalog could ever hold: a token that fails to advance
    /// would otherwise loop forever.
    const PAGE_LIMIT: usize = 500;

    let mut items: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("maxResults={max}"));
        }
        if let Some(token) = token.as_deref() {
            query.push(format!("nextToken={token}"));
        }
        let uri = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{}", query.join("&"))
        };

        let page = emulator.ok(&uri, body).await;
        let page_items = page
            .get(items_key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {uri} {body}: response carries no {items_key} list: {page:?}")
            });

        pages += 1;
        assert!(
            pages <= PAGE_LIMIT,
            "case {uri} {body}: pagination did not terminate within {PAGE_LIMIT} pages"
        );
        if let Some(max) = max_results {
            assert!(
                page_items.len() <= max as usize,
                "case {uri} {body}: page holds {} items, more than maxResults={max}",
                page_items.len()
            );
        }
        for item in &page_items {
            assert!(
                seen.insert(item.to_string()),
                "case {uri} {body}: {item} was returned on more than one page"
            );
        }
        let page_was_empty = page_items.is_empty();
        items.extend(page_items);

        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => {
                assert!(
                    !page_was_empty,
                    "case {uri} {body}: a NextToken came back with an empty page"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }

    items
}

/// The catalog ARN a resource of `kind` — the resource segment a catalog ARN carries
/// (`control`, `domain`, `objective`, `common-control`) — whose published opaque
/// identifier is `identifier` is served under: the global form, which carries neither
/// account nor Region.
///
/// `pub(crate)`: the ONE spelling of a catalog ARN this test target builds, shared with
/// the sibling suites rather than rewritten beside them.
pub(crate) fn catalog_kind_arn(kind: &str, identifier: &str) -> String {
    format!("arn:aws:controlcatalog:::{kind}/{identifier}")
}

/// The catalog ARN a control whose published opaque identifier is `identifier` is served
/// under.
pub(crate) fn catalog_arn(identifier: &str) -> String {
    catalog_kind_arn("control", identifier)
}

/// Sort items by their `Arn`, so two differently ordered sets compare equal.
pub(crate) fn by_arn(mut items: Vec<Value>) -> Vec<Value> {
    items.sort_by_key(|item| {
        item.get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    items
}

/// The string at `path` (a chain of object members) in `item`, or `None` when the
/// chain does not resolve to a string.
///
/// `pub(crate)`: the ONE member-path walk this test target uses, shared with the
/// sibling suites rather than reimplemented beside them.
pub(crate) fn at_path<'a>(item: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = item;
    for key in path {
        cursor = cursor.get(key)?;
    }
    cursor.as_str()
}

/// Every distinct string at `path` (a chain of object members) across `items`.
fn distinct<'a>(items: &'a [Value], path: &[&str]) -> Vec<&'a str> {
    let mut values: Vec<&str> = items
        .iter()
        .filter_map(|item| at_path(item, path))
        .collect();
    values.sort_unstable();
    values.dedup();
    values
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Every list operation walks its whole collection exactly once, whatever page size
/// the caller asks for, and the final page carries no token.
#[tokio::test]
async fn every_list_operation_pages_its_whole_collection_exactly_once() {
    let emulator = Emulator::new();
    for (path, items_key, _modeled_max) in LIST_OPERATIONS {
        let whole = drain(&emulator, path, items_key, None, "{}").await;
        assert!(
            whole.len() >= 3,
            "case {path}: the seeded catalog must hold enough entries to page \
             (holds {})",
            whole.len()
        );
        for page_size in [1u32, 2, 3, 5] {
            let paged = drain(&emulator, path, items_key, Some(page_size), "{}").await;
            assert_eq!(
                paged, whole,
                "case {path} maxResults={page_size}: paging must yield the same items, \
                 in the same order, as one unpaginated page"
            );
        }
    }
}

/// A `NextToken` resumes only the collection and filter that issued it; anything else
/// is refused as invalid input rather than answered from the wrong sequence.
#[tokio::test]
async fn a_next_token_resumes_only_the_sequence_that_issued_it() {
    let emulator = Emulator::new();

    let first_domains = emulator.ok("/domains?maxResults=1", "{}").await;
    let domains_token = first_domains
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("the catalog holds more than one domain, so a token must come back")
        .to_string();

    let first_objectives = emulator.ok("/objectives?maxResults=1", "{}").await;
    let objectives_token = first_objectives
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("the catalog holds more than one objective, so a token must come back")
        .to_string();

    let domain_arn = {
        let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;
        domains[0]["Arn"]
            .as_str()
            .expect("a domain summary carries its ARN")
            .to_string()
    };
    let filtered_objectives =
        format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{domain_arn}"}}]}}}}"#);

    // (case, route, body, token)
    let rejected: [(&str, &str, String, &str); 5] = [
        (
            "a domains token replayed on objectives",
            "/objectives",
            "{}".to_string(),
            domains_token.as_str(),
        ),
        (
            "a domains token replayed on controls",
            "/list-controls",
            "{}".to_string(),
            domains_token.as_str(),
        ),
        (
            "an objectives token replayed under a domain filter",
            "/objectives",
            filtered_objectives,
            objectives_token.as_str(),
        ),
        (
            "a token that is not an issued token",
            "/domains",
            "{}".to_string(),
            "deadbeef",
        ),
        (
            "a token that is not even hex",
            "/domains",
            "{}".to_string(),
            "not-a-token",
        ),
    ];

    for (case, path, body, token) in rejected {
        let (status, response) = emulator
            .post(&format!("{path}?nextToken={token}"), &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected a ValidationException, got {response:?}"
        );
    }

    // The same token DOES resume the sequence that issued it.
    let resumed = emulator
        .ok(&format!("/domains?maxResults=1&nextToken={domains_token}"), "{}")
        .await;
    let resumed_arn = resumed["Domains"][0]["Arn"].as_str();
    assert_ne!(
        resumed_arn, first_domains["Domains"][0]["Arn"].as_str(),
        "case resume: the page after the token must start after the last item returned"
    );
}

/// `MaxResults` is honoured at each published bound and refused one past it.
#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_outside_them() {
    let emulator = Emulator::new();
    for (path, items_key, modeled_max) in LIST_OPERATIONS {
        // Each published bound of the operation's own `MaxResults` range, and one past
        // each end.
        let rows: [(u32, u16); 4] = [
            (0, 400),
            (1, 200),
            (modeled_max, 200),
            (modeled_max + 1, 400),
        ];
        for (max_results, expected) in rows {
            let (status, response) = emulator
                .post(&format!("{path}?maxResults={max_results}"), "{}")
                .await;
            assert_eq!(
                status, expected,
                "case {path} maxResults={max_results}: expected {expected}, got {status} \
                 with {response:?}"
            );
            if expected == 400 {
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {path} maxResults={max_results}: expected a ValidationException, \
                     got {response:?}"
                );
            } else {
                let returned = response
                    .get(items_key)
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
                assert!(
                    returned <= max_results as usize,
                    "case {path} maxResults={max_results}: {returned} items came back"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// Every ARN the catalog hands out satisfies its own modeled ARN shape: the prefix,
/// the identifier character set, and the minimum length.
#[tokio::test]
async fn every_catalog_arn_matches_its_modeled_arn_shape() {
    let emulator = Emulator::new();
    // (route, items member, ARN prefix, the identifier's modeled character class,
    //  the ARN shape's modeled minimum length) — from `DomainArn`, `ObjectiveArn`,
    //  `CommonControlArn` (`[0-9a-z]+`, 33/36/41) and `ControlArn`
    //  (`[0-9a-zA-Z_\-]+`, 34).
    let rows: [(&str, &str, &str, &str, usize); 4] = [
        (
            "/domains",
            "Domains",
            "arn:aws:controlcatalog:::domain/",
            "0123456789abcdefghijklmnopqrstuvwxyz",
            33,
        ),
        (
            "/objectives",
            "Objectives",
            "arn:aws:controlcatalog:::objective/",
            "0123456789abcdefghijklmnopqrstuvwxyz",
            36,
        ),
        (
            "/common-controls",
            "CommonControls",
            "arn:aws:controlcatalog:::common-control/",
            "0123456789abcdefghijklmnopqrstuvwxyz",
            41,
        ),
        (
            "/list-controls",
            "Controls",
            "arn:aws:controlcatalog:::control/",
            "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_-",
            34,
        ),
    ];
    for (path, items_key, prefix, charset, min_length) in rows {
        let items = drain(&emulator, path, items_key, None, "{}").await;
        for item in &items {
            let arn = item["Arn"].as_str().unwrap_or_else(|| {
                panic!("case {path}: an entry came back without an ARN: {item}")
            });
            let identifier = arn.strip_prefix(prefix).unwrap_or_else(|| {
                panic!("case {path}: {arn} does not start with {prefix}")
            });
            assert!(
                !identifier.is_empty() && identifier.chars().all(|c| charset.contains(c)),
                "case {path}: the identifier of {arn} is outside the modeled character class"
            );
            assert!(
                arn.len() >= min_length,
                "case {path}: {arn} is {} characters, under the modeled minimum of \
                 {min_length}",
                arn.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Filtering objectives by a domain returns exactly that domain's objectives, each
/// reporting the domain ListDomains itself returns, and the per-domain filters
/// together partition the unfiltered list.
#[tokio::test]
async fn objectives_are_narrowed_to_the_domain_they_belong_to() {
    let emulator = Emulator::new();
    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;
    let all = drain(&emulator, "/objectives", "Objectives", None, "{}").await;

    let mut union: Vec<Value> = Vec::new();
    for domain in &domains {
        let arn = domain["Arn"].as_str().expect("a domain carries its ARN");
        let name = domain["Name"].as_str().expect("a domain carries its name");
        let body = format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{arn}"}}]}}}}"#);
        // Page size 1, so the filter and the pagination cursor are exercised together.
        let filtered = drain(&emulator, "/objectives", "Objectives", Some(1), &body).await;
        assert!(
            !filtered.is_empty(),
            "case domain {arn}: every seeded domain has objectives under it"
        );
        for objective in &filtered {
            assert_eq!(
                objective["Domain"]["Arn"].as_str(),
                Some(arn),
                "case domain {arn}: objective {} reports a different domain",
                objective["Arn"]
            );
            assert_eq!(
                objective["Domain"]["Name"].as_str(),
                Some(name),
                "case domain {arn}: objective {} reports a domain name ListDomains does \
                 not give",
                objective["Arn"]
            );
        }
        union.extend(filtered);
    }

    assert_eq!(
        by_arn(union),
        by_arn(all),
        "the per-domain filters must partition the unfiltered objective list"
    );
}

/// Filtering common controls by an objective returns exactly that objective's common
/// controls, each reporting the objective and domain the other operations return.
#[tokio::test]
async fn common_controls_are_narrowed_to_the_objective_they_belong_to() {
    let emulator = Emulator::new();
    let objectives = drain(&emulator, "/objectives", "Objectives", None, "{}").await;
    let all = drain(&emulator, "/common-controls", "CommonControls", None, "{}").await;

    let mut union: Vec<Value> = Vec::new();
    for objective in &objectives {
        let arn = objective["Arn"].as_str().expect("an objective carries its ARN");
        let name = objective["Name"].as_str().expect("an objective carries its name");
        let body = format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{arn}"}}]}}}}"#);
        let filtered = drain(&emulator, "/common-controls", "CommonControls", Some(2), &body).await;
        assert!(
            !filtered.is_empty(),
            "case objective {arn}: every seeded objective has common controls under it"
        );
        for common_control in &filtered {
            assert_eq!(
                common_control["Objective"]["Arn"].as_str(),
                Some(arn),
                "case objective {arn}: common control {} reports a different objective",
                common_control["Arn"]
            );
            assert_eq!(
                common_control["Objective"]["Name"].as_str(),
                Some(name),
                "case objective {arn}: common control {} reports an objective name \
                 ListObjectives does not give",
                common_control["Arn"]
            );
            assert_eq!(
                common_control["Domain"]["Arn"].as_str(),
                objective["Domain"]["Arn"].as_str(),
                "case objective {arn}: common control {} reports a domain that is not its \
                 objective's domain",
                common_control["Arn"]
            );
        }
        union.extend(filtered);
    }

    assert_eq!(
        by_arn(union),
        by_arn(all),
        "the per-objective filters must partition the unfiltered common control list"
    );
}

/// A filter that names an entry the catalog does not hold returns no results, and one
/// that names more than one is refused: the model's own documentation says these
/// filters carry a single ARN at a time.
#[tokio::test]
async fn a_single_entry_filter_is_empty_for_an_unknown_arn_and_refused_for_several() {
    let emulator = Emulator::new();
    let absent_domain = format!("arn:aws:controlcatalog:::domain/{}", "z".repeat(25));
    let absent_objective = format!("arn:aws:controlcatalog:::objective/{}", "z".repeat(25));
    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;
    let objectives = drain(&emulator, "/objectives", "Objectives", None, "{}").await;
    let first_domain = domains[0]["Arn"].as_str().expect("a domain carries its ARN");
    let second_domain = domains[1]["Arn"].as_str().expect("a domain carries its ARN");
    let first_objective = objectives[0]["Arn"].as_str().expect("an objective ARN");
    let second_objective = objectives[1]["Arn"].as_str().expect("an objective ARN");

    // (case, route, items member, body, expected status)
    let rows: [(&str, &str, &str, String, u16); 6] = [
        (
            "an objective filter naming no catalog domain",
            "/objectives",
            "Objectives",
            format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{absent_domain}"}}]}}}}"#),
            200,
        ),
        (
            "a common control filter naming no catalog objective",
            "/common-controls",
            "CommonControls",
            format!(
                r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{absent_objective}"}}]}}}}"#
            ),
            200,
        ),
        (
            "an objective filter naming two domains",
            "/objectives",
            "Objectives",
            format!(
                r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{first_domain}"}},{{"Arn":"{second_domain}"}}]}}}}"#
            ),
            400,
        ),
        (
            "a common control filter naming two objectives",
            "/common-controls",
            "CommonControls",
            format!(
                r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{first_objective}"}},{{"Arn":"{second_objective}"}}]}}}}"#
            ),
            400,
        ),
        (
            "an objective filter entry carrying no ARN",
            "/objectives",
            "Objectives",
            r#"{"ObjectiveFilter":{"Domains":[{}]}}"#.to_string(),
            400,
        ),
        (
            "a common control filter entry carrying no ARN",
            "/common-controls",
            "CommonControls",
            r#"{"CommonControlFilter":{"Objectives":[{}]}}"#.to_string(),
            400,
        ),
    ];

    for (case, path, items_key, body, expected) in rows {
        let (status, response) = emulator.post(path, &body).await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} with {response:?}"
        );
        if expected == 200 {
            let items = response
                .get(items_key)
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: no {items_key} list in {response:?}"));
            assert!(
                items.is_empty(),
                "case {case}: expected no results, got {items:?}"
            );
            assert!(
                response.get("NextToken").is_none(),
                "case {case}: an empty result must not carry a NextToken: {response:?}"
            );
        } else {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case}: expected a ValidationException, got {response:?}"
            );
        }
    }
}

/// Every dimension of `ListControls`' filter narrows the result to the controls whose
/// own stored value matches, and dimensions combine.
#[tokio::test]
async fn controls_are_narrowed_by_implementation_and_governed_provider() {
    let emulator = Emulator::new();
    let all = drain(&emulator, "/list-controls", "Controls", None, "{}").await;

    for implementation_type in distinct(&all, &["Implementation", "Type"]) {
        let body =
            format!(r#"{{"Filter":{{"Implementations":{{"Types":["{implementation_type}"]}}}}}}"#);
        let filtered = drain(&emulator, "/list-controls", "Controls", Some(1), &body).await;
        let expected: Vec<Value> = all
            .iter()
            .filter(|control| {
                control["Implementation"]["Type"].as_str() == Some(implementation_type)
            })
            .cloned()
            .collect();
        assert!(
            !expected.is_empty(),
            "case type {implementation_type}: derived from the catalog, so it must match"
        );
        assert_eq!(
            filtered, expected,
            "case type {implementation_type}: the filter must return exactly the controls \
             implemented that way"
        );
    }

    for identifier in distinct(&all, &["Implementation", "Identifier"]) {
        let body =
            format!(r#"{{"Filter":{{"Implementations":{{"Identifiers":["{identifier}"]}}}}}}"#);
        let filtered = drain(&emulator, "/list-controls", "Controls", None, &body).await;
        let expected: Vec<Value> = all
            .iter()
            .filter(|control| control["Implementation"]["Identifier"].as_str() == Some(identifier))
            .cloned()
            .collect();
        assert_eq!(
            filtered, expected,
            "case identifier {identifier}: the filter must return exactly the controls with \
             that implementation identifier"
        );
    }

    let providers: Vec<String> = all
        .iter()
        .filter_map(|control| control["GovernedProviders"].as_array())
        .flatten()
        .filter_map(|provider| provider.as_str().map(str::to_string))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(
        providers.len() >= 2,
        "the catalog must govern more than one provider for this filter to narrow \
         anything: {providers:?}"
    );
    for provider in &providers {
        let body = format!(r#"{{"Filter":{{"GovernedProviders":["{provider}"]}}}}"#);
        let filtered = drain(&emulator, "/list-controls", "Controls", Some(3), &body).await;
        let expected: Vec<Value> = all
            .iter()
            .filter(|control| {
                control["GovernedProviders"]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|value| value.as_str() == Some(provider)))
            })
            .cloned()
            .collect();
        assert_eq!(
            filtered, expected,
            "case provider {provider}: the filter must return exactly the controls that \
             govern it"
        );
    }

    // Dimensions combine: a type from the catalog AND a provider no control with that
    // type governs yields nothing.
    let first_type = distinct(&all, &["Implementation", "Type"])[0].to_string();
    let type_providers: BTreeSet<String> = all
        .iter()
        .filter(|control| control["Implementation"]["Type"].as_str() == Some(&first_type))
        .filter_map(|control| control["GovernedProviders"].as_array())
        .flatten()
        .filter_map(|provider| provider.as_str().map(str::to_string))
        .collect();
    let other_provider = providers
        .iter()
        .find(|provider| !type_providers.contains(*provider));
    if let Some(other_provider) = other_provider {
        let body = format!(
            r#"{{"Filter":{{"Implementations":{{"Types":["{first_type}"]}},"GovernedProviders":["{other_provider}"]}}}}"#
        );
        let filtered = drain(&emulator, "/list-controls", "Controls", None, &body).await;
        assert!(
            filtered.is_empty(),
            "case type {first_type} + provider {other_provider}: the dimensions must \
             combine, got {filtered:?}"
        );
    }

    // A well-formed implementation type the catalog does not hold matches nothing —
    // it is a filter, not a bad request.
    let body = r#"{"Filter":{"Implementations":{"Types":["ZZZ::Nothing::Here"]}}}"#;
    let filtered = drain(&emulator, "/list-controls", "Controls", None, body).await;
    assert!(
        filtered.is_empty(),
        "case an unknown implementation type: expected no results, got {filtered:?}"
    );
}

// ---------------------------------------------------------------------------
// GetControl
// ---------------------------------------------------------------------------

/// Every control ListControls summarises round-trips through GetControl unchanged, and
/// its `RegionConfiguration` follows the documented scope rule.
#[tokio::test]
async fn every_listed_control_round_trips_through_get_control() {
    let emulator = Emulator::new();
    let controls = drain(&emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(!controls.is_empty(), "the catalog must hold controls");

    for summary in &controls {
        let arn = summary["Arn"].as_str().expect("a control carries its ARN");
        let control = emulator
            .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;

        // Every member ControlSummary and GetControlResponse share must agree: the
        // summary is a projection of the record GetControl answers with.
        for member in [
            "Arn",
            "Aliases",
            "Name",
            "Description",
            "Behavior",
            "Severity",
            "ParameterRequirementSummary",
            "Implementation",
            "CreateTime",
            "GovernedResources",
            "GovernedProviders",
        ] {
            assert_eq!(
                control.get(member),
                summary.get(member),
                "case {arn} member {member}: GetControl and ListControls disagree"
            );
        }

        let region_configuration = control
            .get("RegionConfiguration")
            .unwrap_or_else(|| panic!("case {arn}: RegionConfiguration is required"));
        let scope = region_configuration["Scope"].as_str().unwrap_or_default();
        let deployable = region_configuration
            .get("DeployableRegions")
            .and_then(Value::as_array);
        match scope {
            // A GLOBAL control does not show DeployableRegions: the field does not
            // apply to it.
            "GLOBAL" => assert!(
                deployable.is_none(),
                "case {arn}: a GLOBAL control must not report DeployableRegions, got \
                 {deployable:?}"
            ),
            "REGIONAL" => assert!(
                deployable.is_some_and(|regions| !regions.is_empty()),
                "case {arn}: a REGIONAL control must report the Regions it deploys to"
            ),
            // `ControlScope` models exactly these two values.
            other => panic!("case {arn}: Scope is {other}, which ControlScope does not model"),
        }
    }
}

/// GetControl accepts a Control Tower control ARN and answers with the catalog ARN —
/// for any Region or partition in that ARN, because those segments say where the
/// control was enabled rather than which control it is.
#[tokio::test]
async fn get_control_accepts_the_control_tower_arn_form() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let rows: Vec<(String, String)> = catalog
        .controls
        .values()
        .filter_map(|control| {
            control
                .control_tower_identifier
                .clone()
                .map(|identifier| (control.arn.clone(), identifier))
        })
        .collect();
    assert!(
        rows.len() >= 2,
        "the catalog must carry Control Tower control identifiers to exercise this rule"
    );

    for (catalog_arn, identifier) in rows {
        for prefix in [
            "arn:aws:controltower:us-east-1",
            "arn:aws:controltower:eu-west-1",
            "arn:aws-us-gov:controltower:us-gov-west-1",
        ] {
            let requested = format!("{prefix}::control/{identifier}");
            let control = emulator
                .ok("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
                .await;
            assert_eq!(
                control["Arn"].as_str(),
                Some(catalog_arn.as_str()),
                "case {requested}: GetControl must answer with the controlcatalog ARN"
            );
        }
    }
}

/// A well-formed ARN the catalog does not hold is a 404; an ARN that breaks the
/// modeled `ControlArn` shape is a 400.
#[tokio::test]
async fn get_control_refuses_an_unknown_or_malformed_arn() {
    let emulator = Emulator::new();
    // (case, ControlArn, expected status, expected error code)
    let rows: [(&str, &str, u16, &str); 7] = [
        (
            "a well-formed catalog ARN naming no control",
            "arn:aws:controlcatalog:::control/doesnotexist0",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed Control Tower ARN naming no control",
            "arn:aws:controltower:us-east-1::control/AWS-GR_NOT_A_REAL_CONTROL",
            404,
            "ResourceNotFoundException",
        ),
        ("not an ARN at all", "not-an-arn", 400, "ValidationException"),
        (
            "an ARN with no control identifier",
            "arn:aws:controlcatalog:::control/",
            400,
            "ValidationException",
        ),
        (
            "shorter than ControlArn's 34-character minimum",
            "arn:aws:controlcatalog:::control",
            400,
            "ValidationException",
        ),
        (
            "an ARN for another resource type",
            "arn:aws:controlcatalog:::domain/jjvi2tzmp7zefv32qjbhgmsnx",
            400,
            "ValidationException",
        ),
        (
            "an ARN for another service",
            "arn:aws:s3:::control/notthisservice123456",
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, expected_status, expected_code) in rows {
        let (status, response) = emulator
            .post("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected {expected_status}, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case}: expected {expected_code}, got {response:?}"
        );
    }

    // A missing required member is refused too, and never answered with a control.
    let (status, response) = emulator.post("/get-control", "{}").await;
    assert_eq!(
        status, 400,
        "case ControlArn absent: expected 400, got {status} with {response:?}"
    );
}

// ---------------------------------------------------------------------------
// ListControlMappings
// ---------------------------------------------------------------------------

/// Every mapping resolves to catalog entries the other operations return, and each
/// filter dimension narrows the set.
#[tokio::test]
async fn control_mappings_resolve_to_the_catalog_and_every_filter_narrows() {
    let emulator = Emulator::new();
    let controls = drain(&emulator, "/list-controls", "Controls", None, "{}").await;
    let common_controls = drain(&emulator, "/common-controls", "CommonControls", None, "{}").await;
    let mappings = drain(&emulator, "/list-control-mappings", "ControlMappings", None, "{}").await;
    assert!(!mappings.is_empty(), "the catalog must hold control mappings");

    let control_arns: BTreeSet<&str> = controls
        .iter()
        .filter_map(|control| control["Arn"].as_str())
        .collect();
    let common_control_arns: BTreeSet<&str> = common_controls
        .iter()
        .filter_map(|common_control| common_control["Arn"].as_str())
        .collect();

    for mapping in &mappings {
        let control_arn = mapping["ControlArn"].as_str().unwrap_or_default();
        assert!(
            control_arns.contains(control_arn),
            "case mapping {mapping}: ControlArn {control_arn} is not a control ListControls \
             returns"
        );
        // `MappingType` and the `Mapping` union member must name the same kind.
        match mapping["MappingType"].as_str().unwrap_or_default() {
            "COMMON_CONTROL" => {
                let target = mapping["Mapping"]["CommonControl"]["CommonControlArn"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    common_control_arns.contains(target),
                    "case mapping {mapping}: {target} is not a common control \
                     ListCommonControls returns"
                );
            }
            "RELATED_CONTROL" => {
                let target = mapping["Mapping"]["RelatedControl"]["ControlArn"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    control_arns.contains(target),
                    "case mapping {mapping}: related control {target} is not a control \
                     ListControls returns"
                );
            }
            "FRAMEWORK" => {
                let framework = &mapping["Mapping"]["Framework"];
                assert!(
                    framework["Name"].as_str().is_some_and(|n| !n.is_empty())
                        && framework["Item"].as_str().is_some_and(|i| !i.is_empty()),
                    "case mapping {mapping}: a framework mapping carries a name and an item"
                );
            }
            // `MappingType` models exactly these three values.
            other => panic!("case mapping {mapping}: MappingType {other} is not modeled"),
        }
    }

    // One row per control: its mappings, and nothing else.
    let mut by_control = 0usize;
    for control_arn in &control_arns {
        let body = format!(r#"{{"Filter":{{"ControlArns":["{control_arn}"]}}}}"#);
        let filtered = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            Some(1),
            &body,
        )
        .await;
        let expected: Vec<Value> = mappings
            .iter()
            .filter(|mapping| mapping["ControlArn"].as_str() == Some(*control_arn))
            .cloned()
            .collect();
        assert_eq!(
            filtered, expected,
            "case control {control_arn}: the filter must return exactly that control's \
             mappings"
        );
        by_control += filtered.len();
    }
    assert_eq!(
        by_control,
        mappings.len(),
        "the per-control filters must partition every mapping"
    );

    // One row per mapping type.
    let mut by_type = 0usize;
    for mapping_type in ["FRAMEWORK", "COMMON_CONTROL", "RELATED_CONTROL"] {
        let body = format!(r#"{{"Filter":{{"MappingTypes":["{mapping_type}"]}}}}"#);
        let filtered = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            Some(2),
            &body,
        )
        .await;
        let expected: Vec<Value> = mappings
            .iter()
            .filter(|mapping| mapping["MappingType"].as_str() == Some(mapping_type))
            .cloned()
            .collect();
        assert_eq!(
            filtered, expected,
            "case mapping type {mapping_type}: the filter must return exactly the mappings \
             of that type"
        );
        by_type += filtered.len();
    }
    assert_eq!(
        by_type,
        mappings.len(),
        "the per-type filters must partition every mapping"
    );

    // One row per common control.
    for common_control_arn in &common_control_arns {
        let body = format!(r#"{{"Filter":{{"CommonControlArns":["{common_control_arn}"]}}}}"#);
        let filtered = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            None,
            &body,
        )
        .await;
        let expected: Vec<Value> = mappings
            .iter()
            .filter(|mapping| {
                mapping["Mapping"]["CommonControl"]["CommonControlArn"].as_str()
                    == Some(*common_control_arn)
            })
            .cloned()
            .collect();
        assert_eq!(
            filtered, expected,
            "case common control {common_control_arn}: the filter must return exactly the \
             mappings to it"
        );
    }

    // Dimensions combine: one control AND one mapping type.
    let control_with_mappings = mappings[0]["ControlArn"]
        .as_str()
        .expect("a mapping carries its control ARN")
        .to_string();
    for mapping_type in ["FRAMEWORK", "COMMON_CONTROL", "RELATED_CONTROL"] {
        let body = format!(
            r#"{{"Filter":{{"ControlArns":["{control_with_mappings}"],"MappingTypes":["{mapping_type}"]}}}}"#
        );
        let filtered = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            None,
            &body,
        )
        .await;
        let expected: Vec<Value> = mappings
            .iter()
            .filter(|mapping| {
                mapping["ControlArn"].as_str() == Some(control_with_mappings.as_str())
                    && mapping["MappingType"].as_str() == Some(mapping_type)
            })
            .cloned()
            .collect();
        assert_eq!(
            filtered, expected,
            "case control {control_with_mappings} + type {mapping_type}: the dimensions must \
             combine"
        );
    }

    // A Control Tower ARN filters for the same control its catalog ARN does.
    let catalog = Catalog::default();
    for control in catalog.controls.values() {
        let Some(identifier) = control.control_tower_identifier.as_deref() else {
            continue;
        };
        let requested = format!("arn:aws:controltower:us-east-1::control/{identifier}");
        let by_tower_arn = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            None,
            &format!(r#"{{"Filter":{{"ControlArns":["{requested}"]}}}}"#),
        )
        .await;
        let by_catalog_arn = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            None,
            &format!(r#"{{"Filter":{{"ControlArns":["{}"]}}}}"#, control.arn),
        )
        .await;
        assert_eq!(
            by_tower_arn, by_catalog_arn,
            "case {requested}: both accepted ARN forms of one control must filter alike"
        );
    }
}

// ---------------------------------------------------------------------------
// The catalog is state
// ---------------------------------------------------------------------------

/// The catalog is shared, persisted state: two sessions on one emulator see the same
/// catalog, and a separate emulator serves an identical one.
#[tokio::test]
async fn the_catalog_is_shared_state_rather_than_per_request_literals() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let first = Emulator::with_engine(&engine);
    let second = Emulator::with_engine(&engine);
    let elsewhere = Emulator::new();

    for (path, items_key, _modeled_max) in LIST_OPERATIONS {
        let from_first = drain(&first, path, items_key, None, "{}").await;
        let from_second = drain(&second, path, items_key, Some(2), "{}").await;
        let from_elsewhere = drain(&elsewhere, path, items_key, None, "{}").await;
        assert_eq!(
            from_first, from_second,
            "case {path}: two sessions on one emulator must see one catalog"
        );
        assert_eq!(
            from_first, from_elsewhere,
            "case {path}: the catalog must be reproducible across emulators"
        );
    }

    // And a control read through one session is byte-identical through the other.
    let controls = drain(&first, "/list-controls", "Controls", None, "{}").await;
    for summary in &controls {
        let arn = summary["Arn"].as_str().expect("a control carries its ARN");
        let body = format!(r#"{{"ControlArn":"{arn}"}}"#);
        assert_eq!(
            first.ok("/get-control", &body).await,
            second.ok("/get-control", &body).await,
            "case {arn}: GetControl must answer alike in every session"
        );
    }
}
