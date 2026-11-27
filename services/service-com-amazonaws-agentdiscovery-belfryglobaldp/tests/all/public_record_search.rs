// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `SearchPublicRecords`, the one operation this service
//! binds.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so routing, restJson1 (de)serialization and the modelled
//! constraints are exercised exactly as a client would exercise them.
//!
//! The model binds no operation that publishes a public record — the index is what the
//! authoritative and crawler ingest paths write and what a search REPORTS — so the
//! seeding below writes the index through its own state module
//! (`insert_record`), which is the one seam a record enters by.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_agentdiscovery_belfryglobaldp::ops::types::{
    DescriptorEntry, DescriptorSource, DescriptorSourceUrl, Identifier, ProvenanceLink,
    PublicRecord, TrustManifest, ARD_ERROR_CODE_VALUES,
};
use service_com_amazonaws_agentdiscovery_belfryglobaldp::state::{
    identifier_value, ComAmazonawsAgentdiscoveryBelfryglobaldpState, ERROR_CODE_INTERNAL_ERROR,
    ERROR_CODE_INVALID_ARGUMENT,
};
use service_com_amazonaws_agentdiscovery_belfryglobaldp::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The route `SearchPublicRecords` is bound to (`POST /records/search`).
const SEARCH: &str = "/records/search";

/// 2026-01-01T00:00:00Z, the instant the first seeded record was published. Each later
/// record is published one day after the one before it, so every seeded record carries
/// its own timestamps.
const PUBLISHED_AT_SECS: i64 = 1_767_225_600;
/// One day, the step between two seeded records' timestamps.
const ONE_DAY_SECS: i64 = 86_400;
/// How long after it was created a seeded record was last updated.
const UPDATED_AFTER_SECS: i64 = 3_600;

/// One descriptor slot of a seeded record.
#[derive(Debug)]
struct SeedDescriptor {
    /// The `DescriptorMap` key — a `descriptorSlotKey`, which is what a
    /// `descriptorType` filter narrows on.
    slot: &'static str,
    data: &'static str,
    version: &'static str,
    /// The `DescriptorSource$fromUrl` the slot was fetched from, when it has one.
    source_url: Option<&'static str>,
}

/// One record of the seeded index, stated in the members a search reports.
#[derive(Debug)]
struct Seed {
    identifier: Identifier,
    display_name: &'static str,
    description: Option<&'static str>,
    record_type: &'static str,
    domain: Option<&'static str>,
    descriptors: Vec<SeedDescriptor>,
    /// `TrustManifest$contributor`, `@required` of a manifest, so a record carries a
    /// manifest only when it names one.
    contributor: Option<&'static str>,
    /// The manifest's provenance links, as (`relation`, `sourceId`) pairs.
    provenance: Vec<(&'static str, &'static str)>,
    record_version: Option<&'static str>,
}

/// The seeded index: records of mixed identifier form, record type, domain, descriptor
/// slots, provenance and version, including ones that carry none of the optional
/// members at all — so a rule about any of those dimensions has rows on both sides of
/// it.
///
/// `recordType` values come from the model's own `RecordType` enum
/// (`RECORD_TYPE_VALUES`); `relation` values from `ProvenanceRelation`
/// (`PROVENANCE_RELATION_VALUES`).
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            identifier: Identifier::Domain("payments.acme.example".to_string()),
            display_name: "Payments Agent",
            description: Some("Settles invoices and issues refunds."),
            record_type: "AGENT",
            domain: Some("payments.acme.example"),
            descriptors: vec![SeedDescriptor {
                slot: "a2aAgentCard",
                data: "{\"name\":\"payments\"}",
                version: "0.3",
                source_url: Some("https://payments.acme.example/.well-known/agent-card.json"),
            }],
            contributor: Some("https://acme.example/trust"),
            provenance: vec![("publishedFrom", "https://acme.example/publish")],
            record_version: Some("1.0"),
        },
        Seed {
            identifier: Identifier::Uri("https://tools.acme.example/mcp/invoices".to_string()),
            display_name: "Invoice Lookup Tool",
            description: Some("An MCP tool that looks up invoices."),
            record_type: "TOOL",
            domain: Some("tools.acme.example"),
            descriptors: vec![
                SeedDescriptor {
                    slot: "mcpServer",
                    data: "{\"transport\":\"http\"}",
                    version: "1.2",
                    source_url: None,
                },
                SeedDescriptor {
                    slot: "agentSkills",
                    data: "{\"skills\":[\"lookup\"]}",
                    version: "1.2",
                    source_url: None,
                },
            ],
            contributor: Some("https://acme.example/trust"),
            provenance: vec![
                ("discoveredFrom", "https://crawler.example/seed"),
                ("publishedFrom", "https://acme.example/publish"),
            ],
            record_version: Some("2.1"),
        },
        Seed {
            identifier: Identifier::Domain("gateway.example.org".to_string()),
            display_name: "Edge Gateway",
            description: Some("Routes agent traffic to registered tools."),
            record_type: "GATEWAY",
            domain: Some("gateway.example.org"),
            descriptors: Vec::new(),
            contributor: None,
            provenance: Vec::new(),
            record_version: None,
        },
        Seed {
            identifier: Identifier::Domain("support.acme.example".to_string()),
            display_name: "Support Agent",
            description: Some("Answers customer questions about refunds."),
            record_type: "AGENT",
            domain: Some("support.acme.example"),
            descriptors: vec![SeedDescriptor {
                slot: "a2aAgentCard",
                data: "{\"name\":\"support\"}",
                version: "0.3",
                source_url: None,
            }],
            contributor: Some("https://acme.example/trust"),
            provenance: vec![("federatedFrom", "https://partner.example/registry")],
            record_version: Some("1.0"),
        },
        Seed {
            // A record with no domain and no description: a search still reports it, and
            // a filter on a member it does not carry does not match it.
            identifier: Identifier::Uri("https://skills.example.net/skill/翻訳".to_string()),
            display_name: "Translate Skill",
            description: None,
            record_type: "SKILL",
            domain: None,
            descriptors: Vec::new(),
            contributor: None,
            provenance: Vec::new(),
            record_version: Some("3.0"),
        },
        Seed {
            identifier: Identifier::Domain("bot.example.com".to_string()),
            display_name: "Crawler Bot",
            description: Some("Crawls public agent domains."),
            record_type: "WEB_BOT",
            domain: Some("bot.example.com"),
            descriptors: vec![SeedDescriptor {
                slot: "http",
                data: "{\"userAgent\":\"crawler\"}",
                version: "1.0",
                source_url: None,
            }],
            contributor: Some("https://crawler.example/trust"),
            provenance: vec![("discoveredFrom", "https://crawler.example/seed")],
            record_version: Some("0.9"),
        },
    ]
}

/// When the record at `index` of the seeded index was created.
fn created_at(index: usize) -> i64 {
    PUBLISHED_AT_SECS + ONE_DAY_SECS * index as i64
}

/// The record one seed publishes, as the index holds it.
fn public_record(index: usize, seed: &Seed) -> PublicRecord {
    let descriptors: HashMap<String, DescriptorEntry> = seed
        .descriptors
        .iter()
        .map(|descriptor| {
            (
                descriptor.slot.to_string(),
                DescriptorEntry {
                    data: Some(descriptor.data.to_string()),
                    version: Some(descriptor.version.to_string()),
                    source: descriptor.source_url.map(|url| DescriptorSource {
                        from_url: Some(DescriptorSourceUrl {
                            url: url.to_string(),
                        }),
                    }),
                    additional_data: None,
                },
            )
        })
        .collect();
    let trust_manifests: Vec<TrustManifest> = seed
        .contributor
        .map(|contributor| TrustManifest {
            contributor: contributor.to_string(),
            provenance: Some(
                seed.provenance
                    .iter()
                    .map(|(relation, source_id)| ProvenanceLink {
                        relation: (*relation).to_string(),
                        source_id: (*source_id).to_string(),
                    })
                    .collect(),
            ),
        })
        .into_iter()
        .collect();
    PublicRecord {
        identifier: seed.identifier.clone(),
        display_name: seed.display_name.to_string(),
        description: seed.description.map(str::to_string),
        record_type: seed.record_type.to_string(),
        domain: seed.domain.map(str::to_string),
        descriptors: (!descriptors.is_empty()).then_some(descriptors),
        trust_manifests: (!trust_manifests.is_empty()).then_some(trust_manifests),
        record_version: seed.record_version.map(str::to_string),
        created_at: Some(DateTime::from_secs(created_at(index))),
        updated_at: Some(DateTime::from_secs(
            created_at(index) + UPDATED_AFTER_SECS,
        )),
    }
}

/// A router serving this service over a freshly seeded index.
fn seeded_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<ComAmazonawsAgentdiscoveryBelfryglobaldpState>();
        let mut index = state.lock();
        for (position, seed) in seeds().iter().enumerate() {
            index.insert_record(public_record(position, seed));
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// The identifiers of the seeded index, in the order it was published in.
fn seeded_identifiers() -> Vec<String> {
    seeds()
        .iter()
        .map(|seed| identifier_value(&seed.identifier).to_string())
        .collect()
}

/// POST the search route with `body`, returning the HTTP status and the parsed body.
async fn search(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://agent-discovery.us-east-1.amazonaws.com{SEARCH}"
        ))
        .header("host", "agent-discovery.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        // The SigV4 credential scope is what the router reads to pick the service.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=testkeyid/20260101/us-east-1/agent-discovery/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// The identifier of each record a response reports, in the order it reports them.
fn reported_identifiers(case: &str, body: &Value) -> Vec<String> {
    let records = body["records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: records must be a list, got {body}"));
    records
        .iter()
        .map(|record| {
            let identifier = &record["identifier"];
            identifier["domain"]
                .as_str()
                .or_else(|| identifier["uri"].as_str())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an identifier names domain or uri, got {identifier}")
                })
                .to_string()
        })
        .collect()
}

/// The records a whole search answers, keyed by identifier.
async fn searched(case: &str, router: &MultiServiceRouter, body: &Value) -> Vec<String> {
    let (status, response) = search(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status}: {response}"
    );
    reported_identifiers(case, &response)
}

/// Every page of a search is cut from one order, so walking the pages of the seeded
/// index reports each record exactly once whatever page size the caller asked for, and
/// the last page omits `nextToken`.
#[tokio::test]
async fn the_pages_of_a_search_cover_the_index_exactly_once() {
    let router = seeded_router();
    let expected = seeded_identifiers();
    let total = expected.len();
    // Page sizes spanning what the rule generalises over: one record per page, sizes
    // that divide the index and sizes that do not, the index's own size, one past it,
    // and the documented maximum.
    for size in [1_usize, 2, 3, 4, total, total + 1, 100] {
        let case = format!("maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({ "maxResults": size });
            if let Some(resume) = token.as_deref() {
                body["nextToken"] = json!(resume);
            }
            let (status, response) = search(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            let page = reported_identifiers(&case, &response);
            pages += 1;
            assert!(
                page.len() <= size,
                "case {case:?}: page {pages} carries {} records, more than the page size",
                page.len()
            );
            walked.extend(page.clone());
            token = response["nextToken"].as_str().map(str::to_string);
            match token.is_some() {
                // A page handed back with a token is a FULL page: a short page with more
                // to come would make a caller walk more pages than the index needs.
                true => assert_eq!(
                    page.len(),
                    size,
                    "case {case:?}: page {pages} is short but carries a nextToken"
                ),
                false => break,
            }
            assert!(
                pages <= total,
                "case {case:?}: the walk did not end after {pages} pages"
            );
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every seeded record exactly once in index order"
        );
        assert_eq!(
            pages,
            total.div_ceil(size),
            "case {case:?}: expected the index to need {} pages, took {pages}",
            total.div_ceil(size)
        );
    }
}

/// A reported record carries the members the index holds, exactly as it holds them.
#[tokio::test]
async fn a_reported_record_carries_what_the_index_holds() {
    let router = seeded_router();
    let (status, response) = search(&router, &json!({ "maxResults": 100 })).await;
    assert_eq!(status, 200, "expected 200, got {status}: {response}");
    let records = response["records"]
        .as_array()
        .expect("records must be a list")
        .clone();
    let seeds = seeds();
    assert_eq!(
        records.len(),
        seeds.len(),
        "expected every seeded record, got {}",
        records.len()
    );
    for (position, (seed, actual)) in seeds.iter().zip(records.iter()).enumerate() {
        let case = seed.display_name;
        let expected_identifier = match &seed.identifier {
            Identifier::Domain(domain) => json!({ "domain": domain }),
            Identifier::Uri(uri) => json!({ "uri": uri }),
        };
        assert_eq!(
            actual["identifier"], expected_identifier,
            "case {case:?}: expected the identifier {expected_identifier}, got {}",
            actual["identifier"]
        );
        assert_eq!(
            actual["displayName"].as_str(),
            Some(seed.display_name),
            "case {case:?}: expected the display name {:?}, got {}",
            seed.display_name,
            actual["displayName"]
        );
        assert_eq!(
            actual["description"].as_str(),
            seed.description,
            "case {case:?}: expected the description {:?}, got {}",
            seed.description,
            actual["description"]
        );
        assert_eq!(
            actual["recordType"].as_str(),
            Some(seed.record_type),
            "case {case:?}: expected the record type {:?}, got {}",
            seed.record_type,
            actual["recordType"]
        );
        assert_eq!(
            actual["domain"].as_str(),
            seed.domain,
            "case {case:?}: expected the domain {:?}, got {}",
            seed.domain,
            actual["domain"]
        );
        assert_eq!(
            actual["recordVersion"].as_str(),
            seed.record_version,
            "case {case:?}: expected the version {:?}, got {}",
            seed.record_version,
            actual["recordVersion"]
        );
        assert_eq!(
            actual["createdAt"].as_f64(),
            Some(created_at(position) as f64),
            "case {case:?}: expected createdAt {}, got {}",
            created_at(position),
            actual["createdAt"]
        );
        assert_eq!(
            actual["updatedAt"].as_f64(),
            Some((created_at(position) + UPDATED_AFTER_SECS) as f64),
            "case {case:?}: expected updatedAt {}, got {}",
            created_at(position) + UPDATED_AFTER_SECS,
            actual["updatedAt"]
        );

        // Every descriptor slot the record holds, with the entry each slot carries.
        for descriptor in &seed.descriptors {
            let slot = &actual["descriptors"][descriptor.slot];
            assert_eq!(
                slot["data"].as_str(),
                Some(descriptor.data),
                "case {case:?}: expected slot {:?} to carry its data, got {slot}",
                descriptor.slot
            );
            assert_eq!(
                slot["version"].as_str(),
                Some(descriptor.version),
                "case {case:?}: expected slot {:?} to carry its version, got {slot}",
                descriptor.slot
            );
            assert_eq!(
                slot["source"]["fromUrl"]["url"].as_str(),
                descriptor.source_url,
                "case {case:?}: expected slot {:?} to carry its source, got {slot}",
                descriptor.slot
            );
        }
        let reported_slots = match actual["descriptors"].as_object() {
            Some(slots) => slots.len(),
            None => 0,
        };
        assert_eq!(
            reported_slots,
            seed.descriptors.len(),
            "case {case:?}: expected {} descriptor slots, got {}",
            seed.descriptors.len(),
            actual["descriptors"]
        );

        // The trust manifests, each with the provenance links it publishes in order.
        match seed.contributor {
            None => assert!(
                actual["trustManifests"].is_null(),
                "case {case:?}: expected no trust manifest, got {}",
                actual["trustManifests"]
            ),
            Some(contributor) => {
                let manifest = &actual["trustManifests"][0];
                assert_eq!(
                    manifest["contributor"].as_str(),
                    Some(contributor),
                    "case {case:?}: expected the contributor {contributor:?}, got {manifest}"
                );
                let provenance: Vec<(String, String)> = manifest["provenance"]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: provenance must be a list, got {manifest}")
                    })
                    .iter()
                    .map(|link| {
                        (
                            link["relation"].as_str().unwrap_or_default().to_string(),
                            link["sourceId"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                let expected: Vec<(String, String)> = seed
                    .provenance
                    .iter()
                    .map(|(relation, source)| ((*relation).to_string(), (*source).to_string()))
                    .collect();
                assert_eq!(
                    provenance, expected,
                    "case {case:?}: expected the provenance {expected:?}, got {provenance:?}"
                );
            }
        }
    }
}

/// A filter narrows the result set to the records it names, over every operator and
/// field the documented grammar admits.
#[tokio::test]
async fn a_filter_selects_the_records_the_grammar_names() {
    let router = seeded_router();
    let all = seeded_identifiers();
    let agents = vec![
        "payments.acme.example".to_string(),
        "support.acme.example".to_string(),
    ];
    // The rows differ along the operator, the field, the value's form and how many
    // records the condition admits — none, one, several, all but one.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "$eq on recordType",
            json!({ "recordType": { "$eq": "AGENT" } }),
            agents.clone(),
        ),
        (
            "$eq on a recordType nothing carries",
            json!({ "recordType": { "$eq": "CUSTOM" } }),
            Vec::new(),
        ),
        (
            "$ne on recordType",
            json!({ "recordType": { "$ne": "AGENT" } }),
            all.iter()
                .filter(|identifier| !agents.contains(identifier))
                .cloned()
                .collect(),
        ),
        (
            "$in on recordType",
            json!({ "recordType": { "$in": ["SKILL", "WEB_BOT"] } }),
            vec![
                "https://skills.example.net/skill/翻訳".to_string(),
                "bot.example.com".to_string(),
            ],
        ),
        (
            "$eq on domain",
            json!({ "domain": { "$eq": "gateway.example.org" } }),
            vec!["gateway.example.org".to_string()],
        ),
        (
            "$ne on a domain, which a record carrying no domain also satisfies",
            json!({ "domain": { "$ne": "gateway.example.org" } }),
            all.iter()
                .filter(|identifier| identifier.as_str() != "gateway.example.org")
                .cloned()
                .collect(),
        ),
        (
            "$in on domain, which a record carrying no domain does not satisfy",
            json!({ "domain": { "$in": ["payments.acme.example", "bot.example.com"] } }),
            vec![
                "payments.acme.example".to_string(),
                "bot.example.com".to_string(),
            ],
        ),
        (
            "$eq on identifier, a URI",
            json!({ "identifier": { "$eq": "https://tools.acme.example/mcp/invoices" } }),
            vec!["https://tools.acme.example/mcp/invoices".to_string()],
        ),
        (
            "$prefix on identifier",
            json!({ "identifier": { "$prefix": "https://" } }),
            vec![
                "https://tools.acme.example/mcp/invoices".to_string(),
                "https://skills.example.net/skill/翻訳".to_string(),
            ],
        ),
        (
            "$prefix on identifier that nothing begins with",
            json!({ "identifier": { "$prefix": "ftp://" } }),
            Vec::new(),
        ),
        (
            "$eq on descriptorType, a set-valued field",
            json!({ "descriptorType": { "$eq": "a2aAgentCard" } }),
            agents.clone(),
        ),
        (
            "$in on descriptorType",
            json!({ "descriptorType": { "$in": ["mcpServer", "http"] } }),
            vec![
                "https://tools.acme.example/mcp/invoices".to_string(),
                "bot.example.com".to_string(),
            ],
        ),
        (
            "$eq on provenance by relation",
            json!({ "provenance": { "$eq": "federatedFrom" } }),
            vec!["support.acme.example".to_string()],
        ),
        (
            "$eq on provenance by source",
            json!({ "provenance": { "$eq": "https://crawler.example/seed" } }),
            vec![
                "https://tools.acme.example/mcp/invoices".to_string(),
                "bot.example.com".to_string(),
            ],
        ),
        (
            "$eq on recordVersion",
            json!({ "recordVersion": { "$eq": "1.0" } }),
            agents.clone(),
        ),
        (
            "two fields in one object, which are conjunctive",
            json!({ "recordType": { "$eq": "AGENT" }, "domain": { "$eq": "support.acme.example" } }),
            vec!["support.acme.example".to_string()],
        ),
        (
            "$and over single-condition objects",
            json!({ "$and": [
                { "recordType": { "$eq": "AGENT" } },
                { "descriptorType": { "$eq": "a2aAgentCard" } },
            ] }),
            agents.clone(),
        ),
        (
            "$and nothing satisfies",
            json!({ "$and": [
                { "recordType": { "$eq": "AGENT" } },
                { "recordType": { "$eq": "TOOL" } },
            ] }),
            Vec::new(),
        ),
        (
            "$or over single-condition objects",
            json!({ "$or": [
                { "recordType": { "$eq": "GATEWAY" } },
                { "domain": { "$eq": "bot.example.com" } },
            ] }),
            vec![
                "gateway.example.org".to_string(),
                "bot.example.com".to_string(),
            ],
        ),
        (
            "a filter that admits every record",
            json!({ "identifier": { "$ne": "nothing.example" } }),
            all.clone(),
        ),
    ];
    for (case, filters, expected) in cases {
        let body = json!({ "filters": filters, "maxResults": 100 });
        let reported = searched(case, &router, &body).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter expression outside the documented grammar is refused, before any record is
/// read and whatever the index holds.
#[tokio::test]
async fn a_malformed_filter_is_refused_as_an_invalid_argument() {
    let router = seeded_router();
    // A value one character past the documented maximum, and one `$in` list past its.
    let too_long = "a".repeat(513);
    let too_many: Vec<Value> = (0..26).map(|index| json!(format!("v{index}"))).collect();
    // Eleven conditions: one more than the documented maximum.
    let too_many_conditions: Vec<Value> = (0..11)
        .map(|_| json!({ "recordType": { "$eq": "AGENT" } }))
        .collect();
    let cases: Vec<(&str, Value)> = vec![
        ("an empty filter", json!({})),
        ("a filter that is not an object", json!("recordType")),
        ("a filter that is a list", json!([{ "recordType": { "$eq": "AGENT" } }])),
        (
            "a field the grammar does not name",
            json!({ "displayName": { "$eq": "Payments Agent" } }),
        ),
        (
            "an operator the grammar does not name",
            json!({ "recordType": { "$like": "AGENT" } }),
        ),
        (
            "$prefix on a field other than identifier",
            json!({ "domain": { "$prefix": "acme" } }),
        ),
        (
            "a condition that is not an operator object",
            json!({ "recordType": "AGENT" }),
        ),
        ("a condition naming no operator", json!({ "recordType": {} })),
        (
            "a value that is not a string",
            json!({ "recordType": { "$eq": 7 } }),
        ),
        (
            "an empty value",
            json!({ "recordType": { "$eq": "" } }),
        ),
        (
            "a value past the documented length",
            json!({ "recordType": { "$eq": too_long } }),
        ),
        (
            "$in that is not a list",
            json!({ "recordType": { "$in": "AGENT" } }),
        ),
        ("an empty $in", json!({ "recordType": { "$in": [] } })),
        (
            "$in past the documented count",
            json!({ "recordType": { "$in": too_many } }),
        ),
        (
            "$in carrying a value that is not a string",
            json!({ "recordType": { "$in": ["AGENT", 7] } }),
        ),
        (
            "$and nested inside $or",
            json!({ "$or": [{ "$and": [{ "recordType": { "$eq": "AGENT" } }] }] }),
        ),
        (
            "a branch naming more than one condition",
            json!({ "$or": [{ "recordType": { "$eq": "AGENT" }, "domain": { "$eq": "a.example" } }] }),
        ),
        (
            "a branch that is not an object",
            json!({ "$or": ["recordType"] }),
        ),
        ("$or over no branch", json!({ "$or": [] })),
        (
            "$and that is not a list",
            json!({ "$and": { "recordType": { "$eq": "AGENT" } } }),
        ),
        (
            "more conditions than the documented maximum",
            json!({ "$and": too_many_conditions }),
        ),
    ];
    for (case, filters) in cases {
        let (status, response) = search(&router, &json!({ "filters": filters })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert!(
            response["__type"]
                .as_str()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {response}"
        );
        assert_eq!(
            response["errorCode"].as_str(),
            Some(ERROR_CODE_INVALID_ARGUMENT),
            "case {case:?}: expected the errorCode {ERROR_CODE_INVALID_ARGUMENT}, got {response}"
        );
        assert!(
            response["message"]
                .as_str()
                .is_some_and(|message| message.contains("filters")),
            "case {case:?}: the refusal must name the member it refused, got {response}"
        );
    }

    // A filter the grammar admits is served over the same index, so none of the above
    // was refused for a reason other than its own.
    let reported = searched(
        "the documented example",
        &router,
        &json!({ "filters": { "recordType": { "$eq": "AGENT" } } }),
    )
    .await;
    assert_eq!(
        reported.len(),
        2,
        "the documented example must still be served, got {reported:?}"
    );
}

/// A free-text query matches the record texts it names — display name, description and
/// domain — term by term and case-insensitively, and a query nothing matches answers an
/// empty list with no token.
#[tokio::test]
async fn a_free_text_query_matches_the_texts_the_search_reads() {
    let router = seeded_router();
    // The rows differ along which text carries the term, the letter case, the number of
    // terms, the punctuation between them, and a query nothing carries.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "a term only a display name carries",
            "gateway",
            vec!["gateway.example.org".to_string()],
        ),
        (
            "a term only a description carries",
            "invoices",
            vec![
                "payments.acme.example".to_string(),
                "https://tools.acme.example/mcp/invoices".to_string(),
            ],
        ),
        (
            "the same term in another letter case",
            "INVOICES",
            vec![
                "payments.acme.example".to_string(),
                "https://tools.acme.example/mcp/invoices".to_string(),
            ],
        ),
        (
            "a term only a domain carries",
            "bot.example.com",
            vec!["bot.example.com".to_string()],
        ),
        (
            "two terms, which must both occur",
            "refunds support",
            vec!["support.acme.example".to_string()],
        ),
        (
            "two terms no record carries together",
            "refunds crawls",
            Vec::new(),
        ),
        (
            "a term carried by a display name ranks ahead of one carried by a description",
            "agent",
            vec![
                // "Payments Agent" and "Support Agent" name the term in their display
                // name; the gateway and the bot only describe or publish it.
                "payments.acme.example".to_string(),
                "support.acme.example".to_string(),
                "gateway.example.org".to_string(),
                "bot.example.com".to_string(),
            ],
        ),
        ("a query nothing matches", "no-such-agent-xyz", Vec::new()),
        ("a query naming no term at all", "   ", seeded_identifiers()),
    ];
    for (case, query, expected) in cases {
        let body = json!({ "query": query, "maxResults": 100 });
        let (status, response) = search(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
        let reported = reported_identifiers(case, &response);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            response["nextToken"].is_null(),
            "case {case:?}: the whole result set fits in one page, so no token is due, got {response}"
        );
        // `records` is `@required`: an empty result set is an empty list, never an
        // absent member.
        assert!(
            response["records"].is_array(),
            "case {case:?}: expected records to be a list, got {response}"
        );
    }
}

/// The documented page-size range is the one the search serves: both published bounds
/// are accepted, a value past either is refused, and an absent member is served at the
/// documented default.
#[tokio::test]
async fn the_page_size_outside_the_documented_range_is_refused() {
    let router = seeded_router();
    for size in [1, 100] {
        let case = format!("maxResults={size}");
        let (status, response) = search(&router, &json!({ "maxResults": size })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults={size}");
        let (status, response) = search(&router, &json!({ "maxResults": size })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert!(
            response["__type"]
                .as_str()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {response}"
        );
    }

    // No `maxResults`: the documented default is 20, which is more than the index
    // holds, so the whole index comes back in one page.
    let (status, response) = search(&router, &json!({})).await;
    assert_eq!(status, 200, "no maxResults: expected 200, got {response}");
    assert_eq!(
        reported_identifiers("no maxResults", &response),
        seeded_identifiers(),
        "no maxResults: the default page must carry the whole index"
    );
    assert!(
        response["nextToken"].is_null(),
        "no maxResults: nothing remains past the page, so no token is due, got {response}"
    );
}

/// A `nextToken` names a position in the result set of the search that issued it: the
/// same token replayed answers the same page, and a token offered against other
/// criteria — or one a caller assembled — is refused.
#[tokio::test]
async fn a_token_names_a_page_of_the_search_that_issued_it() {
    let router = seeded_router();
    let criteria = json!({ "query": "", "maxResults": 2 });
    let (status, first) = search(&router, &criteria).await;
    assert_eq!(status, 200, "the first page: expected 200, got {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "a page of 2 of the {} seeded records must carry a token: {first}",
                seeds().len()
            )
        })
        .to_string();

    // A paginated read is a read: replaying the token answers the same page again.
    let resumed = json!({ "query": "", "maxResults": 2, "nextToken": token });
    let once = searched("the second page", &router, &resumed).await;
    let twice = searched("the second page again", &router, &resumed).await;
    assert_eq!(
        once, twice,
        "replaying a token must answer the same page, got {once:?} then {twice:?}"
    );
    assert_eq!(
        once,
        seeded_identifiers()[2..4].to_vec(),
        "the second page must resume exactly after the first, got {once:?}"
    );

    // The rows differ along which component of the criteria the replay changed, plus a
    // value the service never issued at all.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a token replayed against another query",
            json!({ "query": "agent", "maxResults": 2, "nextToken": token.clone() }),
        ),
        (
            "a token replayed against another page size",
            json!({ "query": "", "maxResults": 3, "nextToken": token.clone() }),
        ),
        (
            "a token replayed against another filter",
            json!({ "query": "", "maxResults": 2, "nextToken": token.clone(),
                    "filters": { "recordType": { "$eq": "AGENT" } } }),
        ),
        (
            "a token no search issued",
            json!({ "query": "", "maxResults": 2, "nextToken": "not-a-token" }),
        ),
        (
            "a token with a character changed",
            json!({ "query": "", "maxResults": 2,
                    "nextToken": format!("{}x", &token[..token.len() - 1]) }),
        ),
    ];
    for (case, body) in cases {
        let (status, response) = search(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            response["errorCode"].as_str(),
            Some(ERROR_CODE_INVALID_ARGUMENT),
            "case {case:?}: expected the errorCode {ERROR_CODE_INVALID_ARGUMENT}, got {response}"
        );
        assert!(
            response["message"]
                .as_str()
                .is_some_and(|message| message.contains("nextToken")),
            "case {case:?}: the refusal must name the member it refused, got {response}"
        );
    }

    // A token minted under a FILTERED search resumes that filtered set: the sequence a
    // token names is the one its own criteria produce, filter included.
    let filtered = json!({ "filters": { "recordType": { "$eq": "AGENT" } }, "maxResults": 1 });
    let (status, page) = search(&router, &filtered).await;
    assert_eq!(status, 200, "the filtered first page: expected 200, got {page}");
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first of two AGENT records must carry a token: {page}"))
        .to_string();
    let mut resumed = filtered.clone();
    resumed["nextToken"] = json!(token);
    let (status, page) = search(&router, &resumed).await;
    assert_eq!(status, 200, "the filtered second page: expected 200, got {page}");
    assert_eq!(
        reported_identifiers("the filtered second page", &page),
        vec!["support.acme.example".to_string()],
        "the filtered walk must resume after the first AGENT record, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "the filtered walk ends after its second record, so no token is due, got {page}"
    );
}

/// A search changes nothing a later caller sees: the operation is `@readonly`, so the
/// index it reports is the index it was given, however often it is called.
#[tokio::test]
async fn a_search_changes_nothing_a_later_caller_sees() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<ComAmazonawsAgentdiscoveryBelfryglobaldpState>();
        let mut index = state.lock();
        for (position, seed) in seeds().iter().enumerate() {
            index.insert_record(public_record(position, seed));
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let held = seeded_identifiers();
    for pass in 1..=3 {
        let case = format!("pass {pass}");
        // Pages, filters and a query, so a call of each shape is made against the index.
        let walked = searched(&case, &router, &json!({ "maxResults": 2 })).await;
        assert_eq!(
            walked,
            held[..2].to_vec(),
            "case {case:?}: expected the first page of the same index"
        );
        let filtered = searched(
            &case,
            &router,
            &json!({ "filters": { "recordType": { "$eq": "AGENT" } } }),
        )
        .await;
        assert_eq!(
            filtered.len(),
            2,
            "case {case:?}: expected the same two AGENT records, got {filtered:?}"
        );
        let state = engine.state::<ComAmazonawsAgentdiscoveryBelfryglobaldpState>();
        let index = state.lock();
        let identifiers: Vec<String> = index
            .records()
            .iter()
            .map(|record| identifier_value(&record.identifier).to_string())
            .collect();
        assert_eq!(
            identifiers, held,
            "case {case:?}: a read must leave the index exactly as it was"
        );
    }
}

/// Re-publishing a record updates it in place, so the index holds one record per
/// identifier and the order a search pages over does not change under an update.
#[tokio::test]
async fn republishing_a_record_updates_it_in_place() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<ComAmazonawsAgentdiscoveryBelfryglobaldpState>();
        let mut index = state.lock();
        for (position, seed) in seeds().iter().enumerate() {
            index.insert_record(public_record(position, seed));
        }
        // The first seeded record again, with its version moved on.
        let mut republished = public_record(0, &seeds()[0]);
        republished.record_version = Some("2.0".to_string());
        index.insert_record(republished);
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let (status, response) = search(&router, &json!({ "maxResults": 100 })).await;
    assert_eq!(status, 200, "expected 200, got {status}: {response}");
    assert_eq!(
        reported_identifiers("republished", &response),
        seeded_identifiers(),
        "a re-published record must keep its place and not be listed twice"
    );
    assert_eq!(
        response["records"][0]["recordVersion"].as_str(),
        Some("2.0"),
        "expected the re-published version, got {}",
        response["records"][0]
    );
    let updated = searched(
        "the updated version",
        &router,
        &json!({ "filters": { "recordVersion": { "$eq": "2.0" } } }),
    )
    .await;
    assert_eq!(
        updated,
        vec!["payments.acme.example".to_string()],
        "a filter must read the updated record, got {updated:?}"
    );
}

/// An engine carrying no store for this service is a defect in how the emulator was
/// assembled, which the model's own `InternalServerException` reports — never a hollow
/// 200 and never a fresh store built inside the handler.
#[tokio::test]
async fn an_engine_without_this_services_store_is_reported_as_an_internal_error() {
    /// An engine this service's state module cannot reach a store in.
    #[derive(Debug, Default)]
    struct OtherEngine;

    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(OtherEngine)));

    let (status, response) = search(&router, &json!({ "maxResults": 2 })).await;
    assert_eq!(status, 500, "expected 500, got {status}: {response}");
    assert!(
        response["__type"]
            .as_str()
            .is_some_and(|code| code.contains("InternalServerException")),
        "expected an InternalServerException, got {response}"
    );
    assert_eq!(
        response["errorCode"].as_str(),
        Some(ERROR_CODE_INTERNAL_ERROR),
        "expected the errorCode {ERROR_CODE_INTERNAL_ERROR}, got {response}"
    );
}

/// The codes the errors carry are the model's own `ArdErrorCode` values, not values
/// spelled beside them.
#[test]
fn the_error_codes_are_the_models_own() {
    for code in [ERROR_CODE_INVALID_ARGUMENT, ERROR_CODE_INTERNAL_ERROR] {
        assert!(
            ARD_ERROR_CODE_VALUES.contains(&code),
            "case {code:?}: expected one of the model's {ARD_ERROR_CODE_VALUES:?}"
        );
    }
}
