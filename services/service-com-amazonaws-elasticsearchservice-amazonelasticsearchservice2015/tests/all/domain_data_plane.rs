// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an Elasticsearch domain publishes for its own data plane, and what
//! that address serves.
//!
//! Four rules, each asserted over a SET of inputs rather than one recorded call:
//!
//! 1. **Every address the control plane publishes for a domain's data plane is one
//!    a client DIALS as published, and dialling it reaches THAT domain.**
//!    `ElasticsearchDomainStatus$Endpoint` is "the Elasticsearch domain endpoint
//!    that you use to submit index and search requests", so a client builds its
//!    requests on the value it was handed and sends them there — a value nothing
//!    answers at is a value no client can use. The published address is therefore
//!    this emulator's own endpoint with the reserved path that names this plane
//!    and the domain, because the host AWS names that endpoint by resolves
//!    nowhere for a client of this emulator; that AWS host is served too, for a
//!    caller that resolves AWS's own names. The rows span the readers that
//!    publish such an address and the domains they publish it for.
//! 2. **A document written through the published endpoint is readable, countable
//!    and searchable through it**, out of the index it was written into. The rows
//!    span the shapes a stored document takes (a text field, a number, a list, a
//!    nested object) and the query vocabulary that selects it.
//! 3. **An index name outside the engine's own rule is refused, and nothing is
//!    created for it** — with the clause the engine states after the name. The
//!    rows span every clause that rule carries, and the valid names beside them.
//! 4. **A path, a method, a window, a body section or a parameter this cluster
//!    does not serve is answered as the engine answers one**, never as a success:
//!    a search that asked for an order, an aggregation, a narrowed `_source` or a
//!    highlight is refused rather than answered without them.
//!
//! Every case drives the real wire path — an HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::{
    state, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator_authority, Answer};

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The control-plane path prefix this model binds its operations under.
const PREFIX: &str = "/2015-01-01/es";

/// The engine version the probe domains run.
const VERSION: &str = "7.10";

/// How long a poll for a domain to finish provisioning may take before the test
/// gives up: far longer than the emulator's own provisioning span, so a loaded
/// host does not fail the suite, and the loop stops as soon as the condition
/// holds.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one turn of that poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The emulator under test, with the shared engine it was built on — so a suite
/// can read the store the control plane wrote and dial the AWS endpoint host a
/// domain holds there, which no API publishes.
struct Emulator {
    engine: Arc<::engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router carrying only this service, over one shared engine.
    fn new() -> Self {
        let engine = Arc::new(::engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// One request, addressed to `host` and signed for [`REGION`].
    async fn dispatch(&self, host: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/es/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
        };
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// One data-plane request to the ADDRESS the control plane published for a
    /// domain, dialled exactly as a client dials it: a client builds
    /// `https://{address}{path}`, so the AUTHORITY of the published value is the
    /// host the request carries and whatever follows it is the prefix of the
    /// request path.
    async fn at_address(
        &self,
        address: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Answer {
        let (authority, prefix) = match address.find('/') {
            Some(end) => (&address[..end], &address[end..]),
            None => (address, ""),
        };
        self.dispatch(authority, method, &format!("{prefix}{path}"), body)
            .await
    }

    /// `CreateElasticsearchDomain` for a domain running [`VERSION`] on one node.
    async fn create_domain(&self, name: &str) -> Answer {
        self.dispatch(
            emulator_authority(),
            "POST",
            &format!("{PREFIX}/domain"),
            Some(json!({
                "DomainName": name,
                "ElasticsearchVersion": VERSION,
                "ElasticsearchClusterConfig": {
                    "InstanceType": "t3.small.elasticsearch",
                    "InstanceCount": 1
                },
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 10 }
            })),
        )
        .await
    }

    /// `DescribeElasticsearchDomain`.
    async fn describe_domain(&self, name: &str) -> Answer {
        self.dispatch(
            emulator_authority(),
            "GET",
            &format!("{PREFIX}/domain/{name}"),
            None,
        )
        .await
    }

    /// `DescribeElasticsearchDomains`, the batch read of the same record.
    async fn describe_domains(&self, name: &str) -> Answer {
        self.dispatch(
            emulator_authority(),
            "POST",
            &format!("{PREFIX}/domain-info"),
            Some(json!({ "DomainNames": [name] })),
        )
        .await
    }

    /// The address the control plane publishes for `name`'s data plane, once the
    /// domain has finished provisioning.
    ///
    /// The wait is a POLL for that condition — the domain reporting
    /// `Processing: false` — with a generous deadline, so nothing here depends on
    /// how fast the host runs.
    async fn published_endpoint(&self, name: &str, case: &str) -> String {
        let began = Instant::now();
        loop {
            let read = self.describe_domain(name).await;
            let processing = read.body.pointer("/DomainStatus/Processing") == Some(&json!(true));
            if !processing {
                return read.body["DomainStatus"]["Endpoint"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected a published Endpoint for {name:?}, got {:?}",
                            read.body
                        )
                    })
                    .to_string();
            }
            assert!(
                began.elapsed() < SETTLE_DEADLINE,
                "case {case:?}: {name:?} never finished provisioning, last read {:?}",
                read.body
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The endpoint host AWS names a domain's data plane by, read out of the
    /// store the create wrote: the address a caller that resolves AWS's own hosts
    /// dials, which no API of this emulator publishes.
    fn aws_endpoint_host(&self, name: &str) -> String {
        let store = state::service_state(&*self.engine)
            .expect("the shared engine holds this service's store");
        let store = store.lock();
        store
            .domains
            .get(&state::domain_key(REGION, name))
            .and_then(|domain| domain.endpoint.clone())
            .unwrap_or_else(|| panic!("the store holds {name:?} with an endpoint host"))
    }
}

/// The `error.type` a refused data-plane answer carries.
fn error_type(answer: &Answer) -> String {
    answer.body["error"]["type"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `error.reason` a refused data-plane answer carries.
fn error_reason(answer: &Answer) -> String {
    answer.body["error"]["reason"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: every address the control plane publishes for a domain's data plane is
/// an address a client DIALS as published — the same value whichever reader
/// published it — and dialling it reaches THAT domain.
#[tokio::test]
async fn every_published_domain_address_is_dialled_as_published_and_reaches_that_domain() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id();
    // The rows span the domain names a client picks: the shortest the model
    // takes, one carrying every character class a name may, and one at a length
    // a client really uses.
    let domains = ["es1", "probe-domain-01", "sim2real-behaviour-probe"];

    for domain in domains {
        let case = domain;
        let created = emulator.create_domain(domain).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateElasticsearchDomain, got {} with {:?}",
            created.status, created.body
        );
        let published = format!("{}/_aws/es-2015-01-01/{domain}", emulator_authority());
        assert_eq!(
            created.body["DomainStatus"]["Endpoint"],
            json!(published),
            "case {case:?}: the create must publish this emulator's own endpoint with the \
             reserved path naming this plane and the domain, got {:?}",
            created.body["DomainStatus"]["Endpoint"]
        );

        let address = emulator.published_endpoint(domain, case).await;
        assert_eq!(
            address, published,
            "case {case:?}: expected the settled read to publish {published:?}, got {address:?}"
        );
        let batch = emulator.describe_domains(domain).await;
        assert_eq!(
            batch.body["DomainStatusList"][0]["Endpoint"],
            json!(published),
            "case {case:?}: expected the batch read to publish the same address, got {:?}",
            batch.body
        );

        // And the published address SERVES this domain: the banner it answers
        // names this domain's own cluster and the engine it runs.
        let banner = emulator.at_address(&address, "GET", "/", None).await;
        assert_eq!(
            banner.status, 200,
            "case {case:?}: expected 200 from the published address, got {} with {:?}",
            banner.status, banner.body
        );
        assert_eq!(
            banner.body["cluster_name"],
            json!(format!("{account}:{domain}")),
            "case {case:?}: expected the banner to name this domain's cluster, got {:?}",
            banner.body
        );
        assert_eq!(
            banner.body["version"]["number"],
            json!(format!("{VERSION}.0")),
            "case {case:?}: expected the banner to name the engine the domain runs, got {:?}",
            banner.body
        );

        // The host AWS names that endpoint by reaches the same domain, for a
        // caller that resolves AWS's own hosts.
        let aws_host = emulator.aws_endpoint_host(domain);
        let at_aws_host = emulator.dispatch(&aws_host, "GET", "/", None).await;
        assert_eq!(
            at_aws_host.body["cluster_name"], banner.body["cluster_name"],
            "case {case:?}: expected the AWS endpoint host {aws_host:?} to reach the same \
             cluster, got {:?}",
            at_aws_host.body
        );
    }
}

/// One row of the document round trip: the index it is written into, the id, the
/// source, and a query that must find it.
struct DocumentRow {
    case: &'static str,
    index: &'static str,
    id: &'static str,
    source: fn() -> Value,
    query: fn() -> Value,
}

/// The rows span the shapes a stored document takes and the query vocabulary
/// that selects each: an analyzed text field, a number compared exactly, a list
/// element, a nested field, and a boolean composition.
const DOCUMENT_ROWS: &[DocumentRow] = &[
    DocumentRow {
        case: "a text field found by an analyzed match",
        index: "roundtrip",
        id: "1",
        source: || json!({ "title": "sim2real", "count": 1 }),
        query: || json!({ "match": { "title": "sim2real" } }),
    },
    DocumentRow {
        case: "a number found by a term",
        index: "roundtrip",
        id: "2",
        source: || json!({ "title": "another document", "count": 42 }),
        query: || json!({ "term": { "count": 42 } }),
    },
    DocumentRow {
        case: "a list element found by terms",
        index: "roundtrip-lists",
        id: "with-tags",
        source: || json!({ "tags": ["alpha", "beta"] }),
        query: || json!({ "terms": { "tags": ["beta", "gamma"] } }),
    },
    DocumentRow {
        case: "a nested field found by a dotted match",
        index: "roundtrip.nested",
        id: "nested-1",
        source: || json!({ "outer": { "inner": "Nested Value" } }),
        query: || json!({ "match": { "outer.inner": "nested" } }),
    },
    DocumentRow {
        case: "a document found by a boolean composition",
        index: "roundtrip",
        id: "3",
        source: || json!({ "title": "third document", "count": 3 }),
        query: || json!({ "bool": {
            "must": [{ "match": { "title": "third" } }],
            "must_not": [{ "term": { "count": 4 } }]
        }}),
    },
];

/// RULE: a document written through the published endpoint is readable, countable
/// and searchable through it, out of the index it was written into — and a write
/// of the same id again replaces it rather than adding a second document.
#[tokio::test]
async fn a_document_written_through_the_published_endpoint_reads_back_through_it() {
    let emulator = Emulator::new();
    let domain = "data-plane-probe";
    let created = emulator.create_domain(domain).await;
    assert_eq!(
        created.status, 200,
        "case \"CreateElasticsearchDomain\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    let address = emulator
        .published_endpoint(domain, "CreateElasticsearchDomain")
        .await;

    for row in DOCUMENT_ROWS {
        let case = row.case;
        let source = (row.source)();
        let written = emulator
            .at_address(
                &address,
                "PUT",
                &format!("/{}/_doc/{}", row.index, row.id),
                Some(source.clone()),
            )
            .await;
        assert_eq!(
            written.status, 201,
            "case {case:?}: expected 201 for a document the index did not hold, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            written.body["result"],
            json!("created"),
            "case {case:?}: expected result created, got {:?}",
            written.body
        );
        assert_eq!(
            written.body["_version"],
            json!(1),
            "case {case:?}: expected the first write to be version 1, got {:?}",
            written.body
        );

        // A refresh reports the shards it ran on, and changes nothing about the
        // document.
        let refreshed = emulator
            .at_address(&address, "POST", &format!("/{}/_refresh", row.index), None)
            .await;
        assert_eq!(
            refreshed.status, 200,
            "case {case:?}: expected 200 from a refresh, got {} with {:?}",
            refreshed.status, refreshed.body
        );
        assert_eq!(
            refreshed.body["_shards"]["failed"],
            json!(0),
            "case {case:?}: expected a refresh to report no failed shard, got {:?}",
            refreshed.body
        );

        // The search finds exactly that document, with the source it was written
        // with.
        let found = emulator
            .at_address(
                &address,
                "POST",
                &format!("/{}/_search", row.index),
                Some(json!({ "query": (row.query)() })),
            )
            .await;
        assert_eq!(
            found.status, 200,
            "case {case:?}: expected 200 from a search, got {} with {:?}",
            found.status, found.body
        );
        let hits = found.body["hits"]["hits"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected hits, got {:?}", found.body))
            .clone();
        let matched: Vec<&Value> = hits
            .iter()
            .filter(|hit| hit["_id"] == json!(row.id))
            .collect();
        assert_eq!(
            matched.len(),
            1,
            "case {case:?}: expected the search to find the document once, got {:?}",
            found.body
        );
        assert_eq!(
            matched[0]["_source"], source,
            "case {case:?}: expected the hit to carry the source it was written with, got {:?}",
            matched[0]
        );
        assert_eq!(
            matched[0]["_index"],
            json!(row.index),
            "case {case:?}: expected the hit to name the index it was written into, got {:?}",
            matched[0]
        );

        // The read of that id answers the same source, and the count agrees with
        // the search.
        let read = emulator
            .at_address(
                &address,
                "GET",
                &format!("/{}/_doc/{}", row.index, row.id),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 reading the document back, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["_source"], source,
            "case {case:?}: expected the stored source, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["found"],
            json!(true),
            "case {case:?}: expected found true, got {:?}",
            read.body
        );
        let counted = emulator
            .at_address(
                &address,
                "POST",
                &format!("/{}/_count", row.index),
                Some(json!({ "query": (row.query)() })),
            )
            .await;
        assert_eq!(
            counted.body["count"],
            json!(hits.len()),
            "case {case:?}: expected the count to agree with the search, got {:?}",
            counted.body
        );

        // A second write of the same id REPLACES the document rather than adding
        // one: the version moves on and the index still holds one document of
        // that id.
        let again = emulator
            .at_address(
                &address,
                "PUT",
                &format!("/{}/_doc/{}", row.index, row.id),
                Some(source.clone()),
            )
            .await;
        assert_eq!(
            (again.status, &again.body["result"], &again.body["_version"]),
            (200, &json!("updated"), &json!(2)),
            "case {case:?}: expected a replacement to answer 200 updated at version 2, got {} \
             with {:?}",
            again.status,
            again.body
        );

        // And a write that must CREATE is refused for an id the index holds.
        let conflicted = emulator
            .at_address(
                &address,
                "PUT",
                &format!("/{}/_create/{}", row.index, row.id),
                Some(source.clone()),
            )
            .await;
        assert_eq!(
            conflicted.status, 409,
            "case {case:?}: expected 409 for a create of an id the index holds, got {} with {:?}",
            conflicted.status, conflicted.body
        );
        assert_eq!(
            error_type(&conflicted),
            "version_conflict_engine_exception",
            "case {case:?}: expected a version conflict, got {:?}",
            conflicted.body
        );

        // The document removed is gone from the read and from the search.
        let deleted = emulator
            .at_address(
                &address,
                "DELETE",
                &format!("/{}/_doc/{}", row.index, row.id),
                None,
            )
            .await;
        assert_eq!(
            (deleted.status, &deleted.body["result"]),
            (200, &json!("deleted")),
            "case {case:?}: expected 200 deleted, got {} with {:?}",
            deleted.status,
            deleted.body
        );
        let gone = emulator
            .at_address(
                &address,
                "GET",
                &format!("/{}/_doc/{}", row.index, row.id),
                None,
            )
            .await;
        assert_eq!(
            (gone.status, &gone.body["found"]),
            (404, &json!(false)),
            "case {case:?}: expected 404 found false once deleted, got {} with {:?}",
            gone.status,
            gone.body
        );
    }

    // The index a write created is readable as an index, with the settings the
    // cluster gives one whose create named none.
    let index = emulator.at_address(&address, "GET", "/roundtrip", None).await;
    assert_eq!(
        index.status, 200,
        "case \"GET /roundtrip\": expected 200, got {} with {:?}",
        index.status, index.body
    );
    assert_eq!(
        index.body["roundtrip"]["settings"]["index"]["provided_name"],
        json!("roundtrip"),
        "case \"GET /roundtrip\": expected the index's own settings, got {:?}",
        index.body
    );

    // A search reports how many documents the query HOLDS of and answers the
    // window the request asked for: the rows below span the windows a client
    // pages with over one set of three documents.
    for id in ["p1", "p2", "p3"] {
        let written = emulator
            .at_address(
                &address,
                "PUT",
                &format!("/paging/_doc/{id}"),
                Some(json!({ "title": "paged document" })),
            )
            .await;
        assert_eq!(
            written.status, 201,
            "case \"PUT /paging/_doc/{id}\": expected 201, got {} with {:?}",
            written.status, written.body
        );
    }
    // (the case, the window the search asks for, how many hits it answers)
    let windows: &[(&str, Value, usize)] = &[
        ("no window at all", json!({}), 3),
        ("the first page of one", json!({ "size": 1 }), 1),
        ("the second page of one", json!({ "from": 1, "size": 1 }), 1),
        ("a page past the last document", json!({ "from": 3, "size": 2 }), 0),
        ("a page larger than the set", json!({ "size": 50 }), 3),
    ];
    let mut paged: Vec<String> = Vec::new();
    for (case, window, expected) in windows {
        let mut body = window.clone();
        body["query"] = json!({ "match": { "title": "paged" } });
        let found = emulator
            .at_address(&address, "POST", "/paging/_search", Some(body))
            .await;
        let hits = found.body["hits"]["hits"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected hits, got {:?}", found.body))
            .clone();
        assert_eq!(
            hits.len(),
            *expected,
            "case {case:?}: expected {expected} hit(s), got {:?}",
            found.body
        );
        assert_eq!(
            found.body["hits"]["total"]["value"],
            json!(3),
            "case {case:?}: expected the total to count every document the query holds of, \
             got {:?}",
            found.body
        );
        if *case == "the first page of one" || *case == "the second page of one" {
            paged.push(hits[0]["_id"].as_str().unwrap_or_default().to_string());
        }
    }
    assert_eq!(
        paged.len(),
        2,
        "the two single-document pages must each answer one hit, got {paged:?}"
    );
    assert_ne!(
        paged[0], paged[1],
        "two successive pages must answer different documents, got {paged:?}"
    );
}

/// One row of the index-name rule: a name, and the clause the engine refuses it
/// with — or `None` for a name it accepts.
struct NameRow {
    case: &'static str,
    name: &'static str,
    clause: Option<&'static str>,
}

/// The rows span every clause the engine's index-name rule carries, and the valid
/// names beside them. Source: the engine's own index-name validation, stated once
/// in `crate::state::index_name_refusal`.
const NAME_ROWS: &[NameRow] = &[
    NameRow {
        case: "a name carrying a capital",
        name: "Uppercase-Index",
        clause: Some("must be lowercase"),
    },
    NameRow {
        case: "a name that is all capitals",
        name: "LOUD",
        clause: Some("must be lowercase"),
    },
    NameRow {
        case: "a name leading with a hyphen",
        name: "-leading",
        clause: Some("must not start with '_', '-', or '+'"),
    },
    NameRow {
        case: "a name leading with a plus",
        name: "+leading",
        clause: Some("must not start with '_', '-', or '+'"),
    },
    NameRow {
        case: "a name carrying a colon",
        name: "has:colon",
        clause: Some("must not contain ':'"),
    },
    NameRow {
        case: "a name carrying a comma",
        name: "has,comma",
        clause: Some("must not contain the following characters [ , \", *, \\, <, |, ,, >, /, ?]"),
    },
    NameRow {
        case: "a name carrying a star",
        name: "has*star",
        clause: Some("must not contain the following characters [ , \", *, \\, <, |, ,, >, /, ?]"),
    },
    NameRow {
        case: "the current-directory name",
        name: ".",
        clause: Some("must not be '.' or '..'"),
    },
    NameRow {
        case: "the parent-directory name",
        name: "..",
        clause: Some("must not be '.' or '..'"),
    },
    NameRow {
        case: "a lowercase name with digits and a hyphen",
        name: "my-index-000001",
        clause: None,
    },
    NameRow {
        case: "a lowercase name carrying a dot and an underscore",
        name: "logs.2026_01",
        clause: None,
    },
];

/// RULE: a write to an index name outside the engine's own rule is refused with
/// the clause that rule states, and nothing is created for it — so the name the
/// client meant is still absent afterwards.
#[tokio::test]
async fn an_index_name_outside_the_engines_rule_is_refused_and_creates_nothing() {
    let emulator = Emulator::new();
    let domain = "index-name-probe";
    let created = emulator.create_domain(domain).await;
    assert_eq!(
        created.status, 200,
        "case \"CreateElasticsearchDomain\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    let address = emulator
        .published_endpoint(domain, "CreateElasticsearchDomain")
        .await;

    for row in NAME_ROWS {
        let case = row.case;
        let written = emulator
            .at_address(
                &address,
                "PUT",
                &format!("/{}/_doc/1", row.name),
                Some(json!({ "title": "probe" })),
            )
            .await;
        match row.clause {
            None => {
                assert_eq!(
                    written.status, 201,
                    "case {case:?}: expected 201 for the valid name {:?}, got {} with {:?}",
                    row.name, written.status, written.body
                );
            }
            Some(clause) => {
                assert_eq!(
                    written.status, 400,
                    "case {case:?}: expected 400 for {:?}, got {} with {:?}",
                    row.name, written.status, written.body
                );
                assert_eq!(
                    error_type(&written),
                    "invalid_index_name_exception",
                    "case {case:?}: expected invalid_index_name_exception, got {:?}",
                    written.body
                );
                assert_eq!(
                    error_reason(&written),
                    format!("Invalid index name [{}], {clause}", row.name),
                    "case {case:?}: expected the engine's own clause, got {:?}",
                    written.body
                );
                // Nothing was created under the name, and nothing under the
                // name a client might have meant either.
                let lowered = row.name.to_lowercase();
                let read = emulator
                    .at_address(&address, "GET", &format!("/{lowered}/_doc/1"), None)
                    .await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: expected the refused name to create nothing, got {} with \
                     {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    error_type(&read),
                    "index_not_found_exception",
                    "case {case:?}: expected index_not_found_exception for {lowered:?}, got {:?}",
                    read.body
                );
            }
        }
    }

    // A name past the engine's byte ceiling is refused with the length that broke
    // it, which is a function of the name rather than a fixed sentence.
    let long = "a".repeat(state::MAX_INDEX_NAME_BYTES + 1);
    let refused = emulator
        .at_address(
            &address,
            "PUT",
            &format!("/{long}/_doc/1"),
            Some(json!({ "title": "probe" })),
        )
        .await;
    assert_eq!(
        error_reason(&refused),
        format!(
            "Invalid index name [{long}], index name is too long, ({} > {})",
            long.len(),
            state::MAX_INDEX_NAME_BYTES
        ),
        "case \"a name past the byte ceiling\": expected the engine's own clause, got {:?}",
        refused.body
    );
    // And the longest name it does take is accepted.
    let longest = "a".repeat(state::MAX_INDEX_NAME_BYTES);
    let taken = emulator
        .at_address(
            &address,
            "PUT",
            &format!("/{longest}/_doc/1"),
            Some(json!({ "title": "probe" })),
        )
        .await;
    assert_eq!(
        taken.status, 201,
        "case \"the longest name the ceiling takes\": expected 201, got {} with {:?}",
        taken.status, taken.body
    );
}

/// RULE: a read over a cluster's indexes reports the primary shards it was asked
/// over, as those indexes' own settings declare them — never a fixed figure.
#[tokio::test]
async fn a_read_reports_the_primary_shards_its_indexes_declare() {
    let emulator = Emulator::new();
    let domain = "shard-report-probe";
    let created = emulator.create_domain(domain).await;
    assert_eq!(
        created.status, 200,
        "case \"CreateElasticsearchDomain\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    let address = emulator
        .published_endpoint(domain, "CreateElasticsearchDomain")
        .await;

    // The rows span how a create states the setting — not at all (the engine's own
    // default of one), as a number under `settings.index`, as the flat
    // `index.number_of_shards` spelling, and as the string the settings API also
    // takes.
    let rows: &[(&str, &str, Option<Value>, i64)] = &[
        ("no settings at all", "defaulted", None, 1),
        (
            "a number under settings.index",
            "three-shards",
            Some(json!({ "settings": { "index": { "number_of_shards": 3 } } })),
            3,
        ),
        (
            "the flat index.number_of_shards spelling",
            "five-shards",
            Some(json!({ "settings": { "index.number_of_shards": 5 } })),
            5,
        ),
        (
            "the string form the settings API takes",
            "two-shards",
            Some(json!({ "settings": { "number_of_shards": "2" } })),
            2,
        ),
    ];
    let mut every_shard = 0;
    for (case, index, body, expected) in rows {
        let made = emulator
            .at_address(&address, "PUT", &format!("/{index}"), body.clone())
            .await;
        assert_eq!(
            made.status, 200,
            "case {case:?}: expected 200 creating {index:?}, got {} with {:?}",
            made.status, made.body
        );
        every_shard += expected;
        for path in [
            format!("/{index}/_search"),
            format!("/{index}/_count"),
        ] {
            let read = emulator.at_address(&address, "POST", &path, None).await;
            assert_eq!(
                read.body["_shards"]["total"],
                json!(expected),
                "case {case:?}: expected {path} to report the {expected} primary shard(s) the \
                 index declares, got {:?}",
                read.body
            );
            assert_eq!(
                read.body["_shards"]["failed"],
                json!(0),
                "case {case:?}: expected {path} to report no failed shard, got {:?}",
                read.body
            );
        }
        // The setting the create stated is what the index reports back.
        let settings = emulator
            .at_address(&address, "GET", &format!("/{index}/_settings"), None)
            .await;
        assert_eq!(
            settings.body[index]["settings"]["index"]["number_of_shards"],
            json!(expected.to_string()),
            "case {case:?}: expected the index to report its own shard count, got {:?}",
            settings.body
        );
    }

    // And a read over EVERY index of the cluster is a read over every one of their
    // shards.
    let across = emulator.at_address(&address, "POST", "/_search", None).await;
    assert_eq!(
        across.body["_shards"]["total"],
        json!(every_shard),
        "case \"a read over every index\": expected the shards of all four indexes, got {:?}",
        across.body
    );
}

/// One row of what the cluster refuses: a request, and the status and error type
/// the engine answers it with.
struct RefusalRow {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
    status: u16,
    error_type: &'static str,
}

/// The rows span the refusals this surface states: a path it serves nothing for,
/// a served path reached with a method it does not take, a read of an index the
/// cluster does not hold, a query outside the vocabulary, and a result window
/// past the engine's bound.
const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        case: "a first segment carrying the engine's reserved prefix, which names an API",
        method: "PUT",
        path: "/_leading/_doc/1",
        body: None,
        status: 400,
        error_type: "",
    },
    RefusalRow {
        case: "a path this cluster serves nothing for",
        method: "POST",
        path: "/_bulk",
        body: None,
        status: 400,
        error_type: "",
    },
    RefusalRow {
        case: "a served path reached with another method",
        method: "DELETE",
        path: "/_search",
        body: None,
        status: 405,
        error_type: "",
    },
    RefusalRow {
        case: "a search of an index the cluster does not hold",
        method: "POST",
        path: "/absent-index/_search",
        body: None,
        status: 404,
        error_type: "index_not_found_exception",
    },
    RefusalRow {
        case: "a refresh of an index the cluster does not hold",
        method: "POST",
        path: "/absent-index/_refresh",
        body: None,
        status: 404,
        error_type: "index_not_found_exception",
    },
    RefusalRow {
        case: "a query outside the vocabulary this cluster reads",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "query": { "fuzzy": { "title": "probe" } } })),
        status: 400,
        error_type: "parsing_exception",
    },
    RefusalRow {
        case: "a result window past the engine's bound",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "from": 10_000, "size": 10 })),
        status: 400,
        error_type: "illegal_argument_exception",
    },
    // Every section of a search body that would decide WHICH documents are
    // answered, in WHAT order, with WHAT of each one, or with what computed
    // beside them: this cluster evaluates none of them, so a search naming one is
    // refused rather than answered as if it had named none.
    RefusalRow {
        case: "a search asking for an order",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "sort": [{ "count": "asc" }] })),
        status: 400,
        error_type: "parsing_exception",
    },
    RefusalRow {
        case: "a search asking for aggregations",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "aggs": { "by_count": { "terms": { "field": "count" } } } })),
        status: 400,
        error_type: "parsing_exception",
    },
    RefusalRow {
        case: "a search asking for a narrowed _source",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "_source": ["title"] })),
        status: 400,
        error_type: "parsing_exception",
    },
    RefusalRow {
        case: "a search asking for highlighting",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "highlight": { "fields": { "title": {} } } })),
        status: 400,
        error_type: "parsing_exception",
    },
    RefusalRow {
        case: "a search asking how the total is tracked",
        method: "POST",
        path: "/_search",
        body: Some(|| json!({ "track_total_hits": false })),
        status: 400,
        error_type: "parsing_exception",
    },
    RefusalRow {
        case: "a count asking for a window it does not answer",
        method: "POST",
        path: "/_count",
        body: Some(|| json!({ "size": 1 })),
        status: 400,
        error_type: "parsing_exception",
    },
    // And the same through the URI: a search asked by parameter is asked as much
    // as one asked by body.
    RefusalRow {
        case: "a search asking a query string through the URI",
        method: "POST",
        path: "/_search?q=title:probe",
        body: None,
        status: 400,
        error_type: "illegal_argument_exception",
    },
    RefusalRow {
        case: "a search asking for an order through the URI",
        method: "POST",
        path: "/_search?sort=count:asc",
        body: None,
        status: 400,
        error_type: "illegal_argument_exception",
    },
    // And every other route: a parameter the engine would evaluate and this
    // cluster does not is refused there too, so no answer is given as if the
    // parameter had been honoured.
    RefusalRow {
        case: "a conditional write naming the revision it writes against",
        method: "PUT",
        path: "/probe-index/_doc/1?if_seq_no=5&if_primary_term=1",
        body: Some(|| json!({ "title": "probe" })),
        status: 400,
        error_type: "illegal_argument_exception",
    },
    RefusalRow {
        case: "a write naming an external version",
        method: "PUT",
        path: "/probe-index/_doc/1?version=3&version_type=external",
        body: Some(|| json!({ "title": "probe" })),
        status: 400,
        error_type: "illegal_argument_exception",
    },
    RefusalRow {
        case: "a write naming an ingest pipeline",
        method: "PUT",
        path: "/probe-index/_doc/1?pipeline=enrich",
        body: Some(|| json!({ "title": "probe" })),
        status: 400,
        error_type: "illegal_argument_exception",
    },
    RefusalRow {
        case: "a health request asking for a report level",
        method: "GET",
        path: "/_cluster/health?level=indices",
        body: None,
        status: 400,
        error_type: "illegal_argument_exception",
    },
    RefusalRow {
        case: "a document read asking for a narrowed source",
        method: "GET",
        path: "/probe-index/_doc/1?_source_includes=title",
        body: None,
        status: 400,
        error_type: "illegal_argument_exception",
    },
    RefusalRow {
        case: "an index read asking for flattened settings",
        method: "GET",
        path: "/probe-index?flat_settings=true",
        body: None,
        status: 400,
        error_type: "illegal_argument_exception",
    },
];

/// One row of what the cluster DOES evaluate: a request naming a parameter this
/// surface answers, and the status it is answered with.
struct EvaluatedRow {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
    status: u16,
}

/// The rows span every parameter a route of this surface evaluates — the window a
/// search answers, the status a health request waits for, what a write asks of an
/// id, the visibility a write waits for — and the diagnostic parameters every
/// route answers without evaluating.
const EVALUATED_ROWS: &[EvaluatedRow] = &[
    EvaluatedRow {
        case: "a write asking to create, waiting for visibility",
        method: "PUT",
        path: "/parameter-probe/_doc/1?op_type=create&refresh=wait_for&pretty=true",
        body: Some(|| json!({ "title": "probe" })),
        status: 201,
    },
    EvaluatedRow {
        case: "a search stating its window in the URI",
        method: "POST",
        path: "/parameter-probe/_search?from=0&size=1&human=true",
        body: None,
        status: 200,
    },
    EvaluatedRow {
        case: "a health request waiting for a status",
        method: "GET",
        path: "/_cluster/health?wait_for_status=green&error_trace=true",
        body: None,
        status: 200,
    },
    EvaluatedRow {
        case: "a count of an index, laid out for a human",
        method: "POST",
        path: "/parameter-probe/_count?pretty=true",
        body: None,
        status: 200,
    },
];

/// RULE: a path, a method, an index, a query, a window, a body section or a
/// parameter this cluster does not serve is answered as the engine answers one —
/// never as a success.
#[tokio::test]
async fn what_this_cluster_does_not_serve_is_refused_as_the_engine_refuses_it() {
    let emulator = Emulator::new();
    let domain = "refusal-probe";
    let created = emulator.create_domain(domain).await;
    assert_eq!(
        created.status, 200,
        "case \"CreateElasticsearchDomain\": expected 200, got {} with {:?}",
        created.status, created.body
    );

    // A domain still being provisioned has no cluster behind its endpoint yet, so
    // the address it published answers the block the engine answers while a
    // cluster is coming up. Read before the provisioning span is waited out.
    let address = format!("{}/_aws/es-2015-01-01/{domain}", emulator_authority());
    let creating = emulator.at_address(&address, "GET", "/", None).await;
    assert_eq!(
        (creating.status, error_type(&creating).as_str()),
        (503, "cluster_block_exception"),
        "case \"a domain still being provisioned\": expected the cluster block, got {} with {:?}",
        creating.status,
        creating.body
    );

    let address = emulator
        .published_endpoint(domain, "CreateElasticsearchDomain")
        .await;
    for row in REFUSAL_ROWS {
        let case = row.case;
        let answered = emulator
            .at_address(&address, row.method, row.path, row.body.map(|body| body()))
            .await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected {}, got {} with {:?}",
            row.status, answered.status, answered.body
        );
        if !row.error_type.is_empty() {
            assert_eq!(
                error_type(&answered),
                row.error_type,
                "case {case:?}: expected the type {:?}, got {:?}",
                row.error_type,
                answered.body
            );
        }
    }

    // Nothing the refused rows named was created or written: a refusal of a
    // parameter or a section is a refusal before the cluster is changed.
    let untouched = emulator
        .at_address(&address, "GET", "/probe-index/_doc/1", None)
        .await;
    assert_eq!(
        (untouched.status, error_type(&untouched).as_str()),
        (404, "index_not_found_exception"),
        "case \"after every refusal\": expected the refused writes to have created nothing, \
         got {} with {:?}",
        untouched.status,
        untouched.body
    );

    // And the parameters this surface DOES evaluate are answered, so the rule
    // above refuses what it does not evaluate rather than whatever it is handed.
    for row in EVALUATED_ROWS {
        let case = row.case;
        let answered = emulator
            .at_address(&address, row.method, row.path, row.body.map(|body| body()))
            .await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected {}, got {} with {:?}",
            row.status, answered.status, answered.body
        );
    }
}
