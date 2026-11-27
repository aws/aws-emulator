// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Agent Registry discovery read path.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so routing, restJson1 (de)serialization and the modelled
//! constraints are exercised exactly as a client would exercise them.
//!
//! This model binds no operation that creates a registry or a record: both are the Agent
//! Registry CONTROL plane's resources, held in ITS store in the shared engine, and a
//! discovery read reports that store. So the seeding below writes the OWNER's store
//! through its own state module ([`seed_record`]) — which is also what makes a seeded
//! record readable through `GetRegistryRecord` — and a sibling suite drives the same
//! records in through the control plane's real operations.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aws_smithy_types::date_time::Format as TimestampFormat;
use aws_smithy_types::{DateTime, Document};
use serde_json::{json, Value};
use service_com_amazonaws_agentregistry_agentregistry::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_agentregistry_agentregistry::EmulatedService;
use service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::ops::types::{
    A2aAgentCardDescriptor, AgentSkillsDefinitionDescriptor, Descriptors, HttpDescriptor,
    McpServerDescriptor,
};
use service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::state as owner;
use smithy_server_runtime::MultiServiceRouter;

/// The registry the single-registry cases read.
pub(crate) const REGISTRY: &str = "abc123def456";
/// A second registry, holding more records than a default page, so the documented
/// page defaults are observable.
pub(crate) const BULK_REGISTRY: &str = "bulkreg00001";
/// A registry that exists but holds no *discoverable* record, so a refusal can be
/// shown not to depend on what the store holds.
pub(crate) const EMPTY_REGISTRY: &str = "emptyreg0001";
/// How many records `BULK_REGISTRY` holds.
const BULK_RECORDS: usize = 12;

/// 2026-01-01T00:00:00Z, the instant the seeded records were created.
const CREATED_AT_SECS: i64 = 1_767_225_600;
/// 2026-02-02T00:00:00Z, the instant `rec000000001` was last updated.
const UPDATED_AT_SECS: i64 = 1_769_990_400;

/// The discoverable (`APPROVED`) records of [`REGISTRY`], in the record-id order
/// every read reports them in.
const DISCOVERABLE: [&str; 3] = ["rec000000001", "rec000000002", "rec000000004"];

/// A router serving this service over a freshly seeded store.
pub(crate) fn seeded_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One record as the control plane stores it, with the members a discovery read reports
/// spelled by the case and every other member of the owner's record at its empty value.
///
/// A test states what the rule under test varies over — ids, name, type, version, status,
/// descriptors, custom metadata, timestamps — and nothing else: a record's tags,
/// provenance and compliance status are the control plane's and appear in no discovery
/// response.
pub(crate) struct Seeded(owner::RegistryRecord);

impl Seeded {
    /// A record with the members the model marks `@required`. Both ARNs are derived from
    /// the stored ids when it is inserted ([`seed_record`]), so a case never spells them.
    pub(crate) fn new(
        record_id: impl Into<String>,
        name: impl Into<String>,
        record_type: impl Into<String>,
        record_version: impl Into<String>,
        status: impl Into<String>,
        created_at: DateTime,
    ) -> Self {
        Self(owner::RegistryRecord {
            record_id: record_id.into(),
            record_arn: String::new(),
            registry_arn: String::new(),
            name: name.into(),
            display_name: None,
            description: None,
            record_type: record_type.into(),
            descriptors: None,
            record_version: record_version.into(),
            status: status.into(),
            status_reason: None,
            provenance: Vec::new(),
            custom_metadata: None,
            created_by: None,
            created_by_auto_detection: None,
            created_at,
            updated_at: created_at,
            tags: BTreeMap::new(),
            pending: None,
            // A seeded record publishes no revision of its own: a row states the status it
            // is in, and a discovery read projects the record as it stands.
            approved_revision: None,
        })
    }

    #[must_use]
    pub(crate) fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.0.display_name = Some(display_name.into());
        self
    }

    #[must_use]
    pub(crate) fn with_description(mut self, description: impl Into<String>) -> Self {
        self.0.description = Some(description.into());
        self
    }

    #[must_use]
    pub(crate) fn with_descriptors(mut self, descriptors: Descriptors) -> Self {
        self.0.descriptors = Some(descriptors);
        self
    }

    #[must_use]
    pub(crate) fn with_custom_metadata(mut self, custom_metadata: Document) -> Self {
        self.0.custom_metadata = Some(custom_metadata);
        self
    }

    #[must_use]
    pub(crate) fn with_updated_at(mut self, updated_at: DateTime) -> Self {
        self.0.updated_at = updated_at;
        self
    }
}

/// Insert `record` into `registry_id` in the store that OWNS it — the control plane's —
/// creating that registry there if it is absent, and deriving both ARNs from the stored
/// ids exactly as `CreateRegistry` and `CreateRegistryRecord` do, so a discovery read and
/// a control-plane read report the same ARNs for the same resource.
pub(crate) fn seed_record(
    engine: &Arc<engine::InMemoryEngine>,
    registry_id: &str,
    record: Seeded,
) {
    let state = engine.state::<owner::ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>();
    let mut state = state.lock();
    let registry_arn = owner::registry_arn(REGION, registry_id);
    let mut record = record.0;
    record.registry_arn = registry_arn.clone();
    record.record_arn = owner::record_arn(&registry_arn, &record.record_id);
    let created_at = record.created_at;
    state
        .registry_name_index
        .insert(registry_id.to_string(), registry_id.to_string());
    let registry = state
        .registries
        .entry(registry_id.to_string())
        .or_insert_with(|| owner::Registry {
            registry_id: registry_id.to_string(),
            registry_arn,
            // A registry's name is unique within the account; the id serves as one, so
            // every seeded registry carries a name no other does.
            name: registry_id.to_string(),
            description: None,
            encryption_configuration: None,
            discovery_configuration: None,
            approval_configuration: None,
            custom_metadata_schema_configuration: None,
            auto_detection: None,
            // A registry whose creation has finished: what `CreateRegistry` leaves once
            // the clock has moved it past `CREATING`.
            status: owner::REGISTRY_READY.to_string(),
            status_reason: None,
            created_at,
            updated_at: created_at,
            tags: BTreeMap::new(),
            records: BTreeMap::new(),
            pending: None,
        });
    registry
        .records
        .insert(record.record_id.clone(), record);
}

/// The Region every seeded ARN is scoped to, and the one every call below signs for.
pub(crate) const REGION: &str = "us-east-1";

/// Seed one registry with a mix of record types, statuses, descriptor sets and
/// custom metadata, plus a second registry large enough to see a page default.
fn seed(engine: &Arc<engine::InMemoryEngine>) {
    let created_at = DateTime::from_secs(CREATED_AT_SECS);

    seed_record(
        engine,
        REGISTRY,
        Seeded::new(
            "rec000000001",
            "payments-mcp",
            "MCP",
            "1.0.0",
            "APPROVED",
            created_at,
        )
        .with_display_name("Payments MCP")
        .with_description("Processes payment intents")
        .with_descriptors(Descriptors {
            mcp_server: Some(McpServerDescriptor {
                data: Some("{\"tools\":[\"charge\"]}".to_string()),
                data_schema_version: Some("2025-06-18".to_string()),
                ..McpServerDescriptor::default()
            }),
            http: Some(HttpDescriptor::default()),
            ..Descriptors::default()
        })
        .with_custom_metadata(document_object(&[
            ("environment", Document::String("production".to_string())),
            ("requiresApproval", Document::Bool(false)),
        ]))
        .with_updated_at(DateTime::from_secs(UPDATED_AT_SECS)),
    );

    seed_record(
        engine,
        REGISTRY,
        Seeded::new(
            "rec000000002",
            "support-agent",
            "AGENT",
            "2.1",
            "APPROVED",
            created_at,
        )
        .with_description("Handles support conversations")
        .with_descriptors(Descriptors {
            a2a_agent_card: Some(A2aAgentCardDescriptor::default()),
            ..Descriptors::default()
        })
        .with_custom_metadata(document_object(&[(
            "environment",
            Document::String("staging".to_string()),
        )])),
    );

    // Not discoverable: the model's `RegistryRecordStatus` doc says a DEPRECATED
    // record is not.
    seed_record(
        engine,
        REGISTRY,
        Seeded::new(
            "rec000000003",
            "old-agent",
            "AGENT",
            "1.0",
            "DEPRECATED",
            created_at,
        ),
    );

    seed_record(
        engine,
        REGISTRY,
        Seeded::new(
            "rec000000004",
            "skills-pack",
            "SKILL",
            "3.0",
            "APPROVED",
            created_at,
        )
        .with_display_name("Payments skills")
        .with_descriptors(Descriptors {
            agent_skills_definition: Some(AgentSkillsDefinitionDescriptor::default()),
            ..Descriptors::default()
        }),
    );

    // Not discoverable either: DRAFT is "before it is submitted".
    seed_record(
        engine,
        REGISTRY,
        Seeded::new(
            "rec000000005",
            "draft-gateway",
            "GATEWAY",
            "0.1",
            "DRAFT",
            created_at,
        ),
    );

    // REJECTED is not discoverable either, so this registry resolves but answers
    // every discovery read with nothing.
    seed_record(
        engine,
        EMPTY_REGISTRY,
        Seeded::new(
            "void00000001",
            "void-record",
            "CUSTOM",
            "1.0",
            "REJECTED",
            created_at,
        ),
    );

    for index in 1..=BULK_RECORDS {
        seed_record(
            engine,
            BULK_REGISTRY,
            Seeded::new(
                format!("bulk{index:08}"),
                format!("bulk-service-{index:02}"),
                "CUSTOM",
                "1.0",
                "APPROVED",
                created_at,
            ),
        );
    }
}

fn document_object(entries: &[(&str, Document)]) -> Document {
    let mut map = HashMap::new();
    for (key, value) in entries {
        map.insert((*key).to_string(), value.clone());
    }
    Document::Object(map)
}

/// POST `path` with `body` through the router, returning the HTTP status and the
/// parsed response body.
pub(crate) async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value) {
    let response = post_response(router, path, body).await;
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

/// POST `path` with `body` through the router, returning the WHOLE response — so a
/// test can read the headers a refusal carries as well as its body.
pub(crate) async fn post_response(
    router: &MultiServiceRouter,
    path: &str,
    body: &Value,
) -> http::Response<bytes::Bytes> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://agent-registry.us-east-1.amazonaws.com{path}"))
        .header("host", "agent-registry.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        // The SigV4 credential scope is what the router reads to pick the service.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=testkeyid/20260101/us-east-1/agent-registry/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    router.dispatch(request).await
}

pub(crate) fn list_path(registry: &str) -> String {
    format!(
        "/registries/{}/discoverable-records-list",
        encode_label(registry)
    )
}

/// The `@httpLabel` form of an identifier: every byte outside the unreserved set is
/// percent-encoded, so the `:` and `/` of a registry ARN arrive escaped and so does
/// the space or punctuation a malformed identifier carries.
pub(crate) fn encode_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The `recordId`s of a response's `registryRecords`, in response order.
pub(crate) fn record_ids(case: &str, body: &Value) -> Vec<String> {
    body["registryRecords"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: registryRecords must be a list, got {body}"))
        .iter()
        .map(|record| {
            record["recordId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: recordId must be a string: {record}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of List, returning the record ids in page order and asserting
/// the last page carries no `nextToken`.
async fn list_every_page(
    router: &MultiServiceRouter,
    case: &str,
    registry: &str,
    max_results: Option<i64>,
    filters: Option<Value>,
) -> Vec<String> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..64 {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["maxResults"] = json!(max);
        }
        if let Some(filters) = filters.clone() {
            body["filters"] = filters;
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let (status, response) = post(router, &list_path(registry), &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page} must be 200, got {status}: {response}"
        );
        collected.extend(record_ids(case, &response));
        next_token = response["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: List never reached a final page: {collected:?}");
}

#[tokio::test]
async fn list_pages_through_exactly_the_discoverable_records_at_every_page_size() {
    let router = seeded_router();
    // The rule generalises over page size, so the table spans it: one record per
    // page, a page that divides the result set, a page that does not, a page larger
    // than the result set, and no `maxResults` at all.
    let cases: [(&str, Option<i64>); 6] = [
        ("maxResults=1", Some(1)),
        ("maxResults=2", Some(2)),
        ("maxResults=3", Some(3)),
        ("maxResults=4", Some(4)),
        ("maxResults=100", Some(100)),
        ("maxResults absent", None),
    ];
    for (case, max_results) in cases {
        let collected = list_every_page(&router, case, REGISTRY, max_results, None).await;
        assert_eq!(
            collected, DISCOVERABLE,
            "case {case:?}: paging must yield every discoverable record exactly once, got {collected:?}"
        );
    }

    // The same rule over a result set bigger than one page, in a registry whose
    // records are seeded in bulk.
    let bulk: Vec<String> = (1..=BULK_RECORDS).map(|n| format!("bulk{n:08}")).collect();
    for max_results in [1_i64, 5, 7, 100] {
        let case = format!("bulk maxResults={max_results}");
        let collected =
            list_every_page(&router, &case, BULK_REGISTRY, Some(max_results), None).await;
        assert_eq!(
            collected, bulk,
            "case {case:?}: paging must yield every discoverable record exactly once, got {collected:?}"
        );
    }
}

/// The list read, as a page-size case addresses it.
const LIST_READ: &str = "list";
/// The search read, as a page-size case addresses it.
const SEARCH_READ: &str = "search";

/// Ask one of the two paginated reads for a page of [`BULK_REGISTRY`], with
/// `maxResults` spelled only when the case names one — so an absent member really is
/// absent on the wire.
async fn page_of(
    router: &MultiServiceRouter,
    read: &str,
    max_results: Option<i64>,
) -> (u16, Value) {
    let (path, mut body) = if read == LIST_READ {
        (list_path(BULK_REGISTRY), json!({}))
    } else {
        (
            "/discoverable-records-search".to_string(),
            // A query with no term matches every discoverable record, so what bounds
            // the answer is the page size alone.
            json!({ "searchQuery": "", "registryIds": [BULK_REGISTRY] }),
        )
    };
    if let Some(max) = max_results {
        body["maxResults"] = json!(max);
    }
    post(router, &path, &body).await
}

/// Resolving a paginated read's page size is ONE rule, so both reads that have one
/// answer by it: an absent `maxResults` is served at that read's documented default,
/// a value the documented range admits bounds the answer at exactly that many
/// records, and a value outside that range is refused 400 `ValidationException`
/// naming `maxResults`.
///
/// Each read keeps its own documented numbers: `maxResults` is `@range{min:1,max:100}`
/// with no default on the list and `@range{min:1,max:20}` with `@default: 10` on the
/// search (`smithy_query(query="operation",
/// operation="ListDiscoverableRegistryRecords,SearchDiscoverableRegistryRecords")`).
/// [`BULK_REGISTRY`] holds fewer discoverable records than either maximum, so a page
/// bounded above that count carries every one of them.
#[tokio::test]
async fn both_paginated_reads_resolve_their_page_size_by_the_same_rule() {
    let router = seeded_router();
    // (read, maxResults in the request, records in the answer): each read's published
    // minimum, a value strictly between its bounds, its published maximum, and the
    // absent member — the four points the rule distinguishes.
    let served: [(&str, Option<i64>, usize); 8] = [
        (LIST_READ, Some(1), 1),
        (LIST_READ, Some(7), 7),
        (LIST_READ, Some(100), BULK_RECORDS),
        (LIST_READ, None, BULK_RECORDS),
        (SEARCH_READ, Some(1), 1),
        (SEARCH_READ, Some(7), 7),
        (SEARCH_READ, Some(20), BULK_RECORDS),
        (SEARCH_READ, None, 10),
    ];
    for (read, max_results, expected) in served {
        let case = format!("{read} maxResults={max_results:?}");
        let (status, body) = page_of(&router, read, max_results).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let found = record_ids(&case, &body);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} records, got {found:?}"
        );
    }

    // (read, maxResults): one past each read's published maximum, its published
    // minimum less one, and a value that is no count at all.
    let refused: [(&str, i64); 6] = [
        (LIST_READ, 101),
        (LIST_READ, 0),
        (LIST_READ, -1),
        (SEARCH_READ, 21),
        (SEARCH_READ, 0),
        (SEARCH_READ, -1),
    ];
    for (read, max_results) in refused {
        let case = format!("{read} maxResults={max_results}");
        let (status, body) = page_of(&router, read, Some(max_results)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let message = body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: message must be a string: {body}"));
        assert!(
            message.contains("maxResults"),
            "case {case:?}: the refusal must name maxResults, got {message:?}"
        );
    }
}

#[tokio::test]
async fn list_first_page_reports_a_token_only_while_records_remain() {
    let router = seeded_router();
    // (maxResults, records on page 1, whether a token follows)
    let cases: [(i64, usize, bool); 4] = [(1, 1, true), (2, 2, true), (3, 3, false), (4, 3, false)];
    for (max_results, expected_len, expects_token) in cases {
        let case = format!("maxResults={max_results}");
        let (status, body) = post(
            &router,
            &list_path(REGISTRY),
            &json!({ "maxResults": max_results }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");
        let ids = record_ids(&case, &body);
        assert_eq!(
            ids.len(),
            expected_len,
            "case {case:?}: expected {expected_len} records, got {ids:?}"
        );
        assert_eq!(
            body["nextToken"].is_string(),
            expects_token,
            "case {case:?}: expected nextToken present={expects_token}, got {}",
            body["nextToken"]
        );
        if expects_token {
            let token = body["nextToken"].as_str().expect("a token");
            assert!(
                !ids.contains(&token.to_string()),
                "case {case:?}: nextToken must be opaque, not a record id: {token}"
            );
        }
    }
}

#[tokio::test]
async fn list_applies_filters_conjunctively() {
    let router = seeded_router();
    // Every case is driven through paging too, so filtering and pagination compose.
    let cases: [(&str, Value, Vec<&str>); 9] = [
        (
            "recordType=MCP",
            json!([{ "name": "recordType", "values": ["MCP"] }]),
            vec!["rec000000001"],
        ),
        (
            "recordType=AGENT",
            json!([{ "name": "recordType", "values": ["AGENT"] }]),
            vec!["rec000000002"],
        ),
        (
            "recordType=SKILL",
            json!([{ "name": "recordType", "values": ["SKILL"] }]),
            vec!["rec000000004"],
        ),
        (
            // The only GATEWAY record is DRAFT, so it is not discoverable.
            "recordType=GATEWAY",
            json!([{ "name": "recordType", "values": ["GATEWAY"] }]),
            vec![],
        ),
        (
            "descriptorType=http",
            json!([{ "name": "descriptorType", "values": ["http"] }]),
            vec!["rec000000001"],
        ),
        (
            "descriptorType=a2aAgentCard",
            json!([{ "name": "descriptorType", "values": ["a2aAgentCard"] }]),
            vec!["rec000000002"],
        ),
        (
            "descriptorType=agentSkillsDefinition",
            json!([{ "name": "descriptorType", "values": ["agentSkillsDefinition"] }]),
            vec!["rec000000004"],
        ),
        (
            "recordType=MCP AND descriptorType=mcpServer",
            json!([
                { "name": "recordType", "values": ["MCP"] },
                { "name": "descriptorType", "values": ["mcpServer"] },
            ]),
            vec!["rec000000001"],
        ),
        (
            "recordType=AGENT AND descriptorType=mcpServer (filters are conjunctive)",
            json!([
                { "name": "recordType", "values": ["AGENT"] },
                { "name": "descriptorType", "values": ["mcpServer"] },
            ]),
            vec![],
        ),
    ];
    for (case, filters, expected) in cases {
        let collected =
            list_every_page(&router, case, REGISTRY, Some(1), Some(filters.clone())).await;
        assert_eq!(
            collected, expected,
            "case {case:?}: expected {expected:?}, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn list_summaries_carry_the_derived_arns_and_descriptor_keys_but_no_descriptors() {
    let router = seeded_router();
    let (status, body) = post(&router, &list_path(REGISTRY), &json!({})).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let records = body["registryRecords"]
        .as_array()
        .expect("registryRecords must be a list");

    // (recordId, recordType, recordVersion, descriptorTypes)
    let expected: [(&str, &str, &str, Vec<&str>); 3] = [
        ("rec000000001", "MCP", "1.0.0", vec!["mcpServer", "http"]),
        ("rec000000002", "AGENT", "2.1", vec!["a2aAgentCard"]),
        ("rec000000004", "SKILL", "3.0", vec!["agentSkillsDefinition"]),
    ];
    for (record, (record_id, record_type, record_version, descriptor_types)) in
        records.iter().zip(expected)
    {
        let case = record_id;
        assert_eq!(
            record["recordId"], json!(record_id),
            "case {case:?}: wrong recordId: {record}"
        );
        assert_eq!(
            record["registryArn"],
            json!(format!(
                "arn:aws:agent-registry:us-east-1:000000000000:registry/{REGISTRY}"
            )),
            "case {case:?}: registryArn must be derived from the stored id: {record}"
        );
        assert_eq!(
            record["recordArn"],
            json!(format!(
                "arn:aws:agent-registry:us-east-1:000000000000:registry/{REGISTRY}/record/{record_id}"
            )),
            "case {case:?}: recordArn must be derived from the stored ids: {record}"
        );
        assert_eq!(
            record["recordType"], json!(record_type),
            "case {case:?}: wrong recordType: {record}"
        );
        assert_eq!(
            record["recordVersion"], json!(record_version),
            "case {case:?}: wrong recordVersion: {record}"
        );
        assert_eq!(
            record["status"], json!("APPROVED"),
            "case {case:?}: only APPROVED records are discoverable: {record}"
        );
        let actual_descriptor_types: Vec<&str> = record["descriptorTypes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: descriptorTypes must be a list: {record}"))
            .iter()
            .map(|value| value.as_str().expect("a descriptor key"))
            .collect();
        assert_eq!(
            actual_descriptor_types, descriptor_types,
            "case {case:?}: expected descriptorTypes {descriptor_types:?}, got {actual_descriptor_types:?}"
        );
        assert!(
            record.get("descriptors").is_none(),
            "case {case:?}: a list summary must not carry descriptors: {record}"
        );
        for (member, expected_secs) in [
            ("createdAt", CREATED_AT_SECS),
            (
                "updatedAt",
                if record_id == "rec000000001" {
                    UPDATED_AT_SECS
                } else {
                    CREATED_AT_SECS
                },
            ),
        ] {
            assert!(
                denotes_instant(&record[member], expected_secs),
                "case {case:?}: {member} must report the stored instant ({expected_secs}), got {}",
                record[member]
            );
        }
    }
}

/// Whether `value` reports the instant `expected_secs` in the wire form this model
/// declares for it: the RFC 3339 `date-time` STRING its target timestamp shape
/// (`DateTimestamp`, `@timestampFormat("date-time")`) asks for — never the
/// epoch-seconds number the `restJson1` protocol defaults to, which no client bound
/// to this model can read for such a member.
fn denotes_instant(value: &Value, expected_secs: i64) -> bool {
    match value {
        Value::String(rendered) => DateTime::from_secs(expected_secs)
            .fmt(TimestampFormat::DateTime)
            .is_ok_and(|expected| *rendered == expected),
        _ => false,
    }
}

#[tokio::test]
async fn a_registry_identifier_may_be_a_bare_id_or_the_registry_arn() {
    let router = seeded_router();
    let arn = format!("arn:aws:agent-registry:us-east-1:000000000000:registry/{REGISTRY}");
    for identifier in [REGISTRY.to_string(), arn] {
        let case = format!("List {identifier}");
        let (status, body) = post(&router, &list_path(&identifier), &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            record_ids(&case, &body),
            DISCOVERABLE,
            "case {case:?}: an ARN and a bare id must name the same registry"
        );

        let case = format!("Search {identifier}");
        let (status, body) = post(
            &router,
            "/discoverable-records-search",
            &json!({ "searchQuery": "", "registryIds": [identifier], "maxResults": 20 }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            record_ids(&case, &body),
            DISCOVERABLE,
            "case {case:?}: an ARN and a bare id must name the same registry"
        );

        let case = format!("BatchGet {identifier}");
        let (status, body) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": identifier, "recordIds": ["rec000000001"] }] }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            record_ids(&case, &body),
            vec!["rec000000001"],
            "case {case:?}: an ARN and a bare id must name the same registry"
        );
    }
}

#[tokio::test]
async fn an_unknown_registry_is_reported_as_resource_not_found() {
    let router = seeded_router();
    let unknown = "000000000000";
    let (status, body) = post(&router, &list_path(unknown), &json!({})).await;
    assert_eq!(status, 404, "List: expected 404, got {status}: {body}");
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|code| code.contains("ResourceNotFoundException")),
        "List: expected ResourceNotFoundException, got {body}"
    );

    let (status, body) = post(
        &router,
        "/discoverable-records-search",
        &json!({ "searchQuery": "payments", "registryIds": [unknown], "maxResults": 10 }),
    )
    .await;
    assert_eq!(status, 404, "Search: expected 404, got {status}: {body}");
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|code| code.contains("ResourceNotFoundException")),
        "Search: expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn batch_get_reports_each_unresolvable_record_without_failing_the_call() {
    let router = seeded_router();
    let record_arn =
        format!("arn:aws:agent-registry:us-east-1:000000000000:registry/{REGISTRY}/record/rec000000002");
    let missing_arn =
        format!("arn:aws:agent-registry:us-east-1:000000000000:registry/{REGISTRY}/record/zzz999999999");
    // An existing record id, but under a registry the entry does not name.
    let foreign_arn =
        format!("arn:aws:agent-registry:us-east-1:000000000000:registry/{BULK_REGISTRY}/record/rec000000001");
    // (requested identifier, resolves?) — the class spans the two identifier forms
    // the model accepts, an unknown id, and a record whose status keeps it off the
    // discovery surface.
    let cases: [(&str, bool); 6] = [
        ("rec000000001", true),
        (record_arn.as_str(), true),
        ("zzz999999999", false),
        (missing_arn.as_str(), false),
        // A record ARN scoped to a different registry than the entry names.
        (foreign_arn.as_str(), false),
        // rec000000003 is DEPRECATED: stored, but not discoverable.
        ("rec000000003", false),
    ];
    let requested: Vec<&str> = cases.iter().map(|(identifier, _)| *identifier).collect();
    let (status, body) = post(
        &router,
        "/discoverable-records-batch",
        &json!({ "entries": [{ "registryId": REGISTRY, "recordIds": requested }] }),
    )
    .await;
    assert_eq!(status, 200, "expected a partial success 200, got {status}: {body}");

    let returned = record_ids("batch", &body);
    assert_eq!(
        returned,
        vec!["rec000000001", "rec000000002"],
        "expected only the resolvable records, got {returned:?}"
    );
    let errors = body["errors"].as_array().expect("errors must be a list");
    let reported: Vec<(&str, &str, &str)> = errors
        .iter()
        .map(|error| {
            (
                error["registryId"].as_str().unwrap_or_default(),
                error["recordId"].as_str().unwrap_or_default(),
                error["errorCode"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let expected: Vec<(&str, &str, &str)> = cases
        .iter()
        .filter(|(_, resolves)| !resolves)
        .map(|(identifier, _)| (REGISTRY, *identifier, "RESOURCE_NOT_FOUND"))
        .collect();
    assert_eq!(
        reported, expected,
        "each unresolvable record must be reported once, with the identifier echoed in the form supplied: expected {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn batch_get_error_entries_carry_only_the_members_the_service_populates() {
    let router = seeded_router();
    let unknown_arn =
        format!("arn:aws:agent-registry:us-east-1:000000000000:registry/{REGISTRY}/record/zzz999999999");
    // An existing record id, but under a registry the entry does not name.
    let foreign_arn =
        format!("arn:aws:agent-registry:us-east-1:000000000000:registry/{BULK_REGISTRY}/record/rec000000001");
    // (case, the registry the entry names, the record ids it asks for, the ids
    // reported in `errors`) — the class spans why a record does not resolve (unknown
    // id, unknown ARN, an ARN scoped to another registry, a status that keeps the
    // record off the discovery surface, a registry exposing no record at all), the two
    // identifier forms `RecordIdentifier` admits, and how many entries one call
    // reports.
    let cases: [(&str, &str, Vec<&str>, Vec<&str>); 6] = [
        (
            "one unknown id",
            REGISTRY,
            vec!["zzz999999999"],
            vec!["zzz999999999"],
        ),
        (
            "an unknown id in ARN form",
            REGISTRY,
            vec![unknown_arn.as_str()],
            vec![unknown_arn.as_str()],
        ),
        (
            "an existing record scoped to another registry",
            REGISTRY,
            vec![foreign_arn.as_str()],
            vec![foreign_arn.as_str()],
        ),
        (
            // rec000000003 is DEPRECATED: stored, but not discoverable.
            "a stored record that is not discoverable",
            REGISTRY,
            vec!["rec000000003"],
            vec!["rec000000003"],
        ),
        (
            "several unresolvable ids beside a resolvable one",
            REGISTRY,
            vec!["rec000000001", "zzz999999999", "rec000000003", "yyy888888888"],
            vec!["zzz999999999", "rec000000003", "yyy888888888"],
        ),
        (
            "a registry exposing no discoverable record",
            EMPTY_REGISTRY,
            vec!["rec000000001"],
            vec!["rec000000001"],
        ),
    ];
    for (case, registry_id, requested, unresolvable) in cases {
        let (status, body) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": registry_id, "recordIds": requested }] }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        let errors = body["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: errors must be a list, got {body}"));
        // A per-record error reports the echoed registryId, the echoed recordId and
        // the machine-readable errorCode, and NOTHING else: the optional
        // human-readable `message` is a detail this service does not add, so the
        // serialized entry's member set is these three for every such record.
        let expected: Vec<Value> = unresolvable
            .iter()
            .map(|record_id| {
                json!({
                    "registryId": registry_id,
                    "recordId": record_id,
                    "errorCode": "RESOURCE_NOT_FOUND",
                })
            })
            .collect();
        assert_eq!(
            errors, &expected,
            "case {case:?}: expected exactly {expected:?}, got {errors:?}"
        );
        for error in errors {
            assert!(
                error.get("message").is_none(),
                "case {case:?}: a per-record error must carry no message member, got {error}"
            );
        }
    }
}

#[tokio::test]
async fn batch_get_serializes_both_required_lists_even_when_one_is_empty() {
    let router = seeded_router();
    // (case, the registry the entry names, the record ids it asks for, records, errors)
    let cases: [(&str, &str, Value, usize, usize); 3] = [
        (
            "all resolve",
            REGISTRY,
            json!(["rec000000001", "rec000000002"]),
            2,
            0,
        ),
        ("none resolve", REGISTRY, json!(["zzz999999999"]), 0, 1),
        (
            // A registry that EXISTS but exposes none of the requested records: the
            // per-record `errors` list is for exactly this, while a registry the
            // store does not hold is refused outright (pinned by
            // `tests/all/registry_read_refusals.rs`).
            "registry with no discoverable record",
            EMPTY_REGISTRY,
            json!(["rec000000001"]),
            0,
            1,
        ),
    ];
    for (case, registry_id, record_ids_requested, records, errors) in cases {
        let (status, body) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": registry_id, "recordIds": record_ids_requested }] }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            body["registryRecords"].as_array().map(Vec::len),
            Some(records),
            "case {case:?}: expected {records} records, got {body}"
        );
        assert_eq!(
            body["errors"].as_array().map(Vec::len),
            Some(errors),
            "case {case:?}: expected {errors} errors, got {body}"
        );
    }
}

#[tokio::test]
async fn batch_get_returns_the_full_summary_and_search_returns_the_identical_one() {
    let router = seeded_router();
    let (status, batch) = post(
        &router,
        "/discoverable-records-batch",
        &json!({ "entries": [{ "registryId": REGISTRY, "recordIds": ["rec000000001"] }] }),
    )
    .await;
    assert_eq!(status, 200, "BatchGet: expected 200, got {status}: {batch}");
    let from_batch = &batch["registryRecords"][0];

    assert_eq!(
        from_batch["descriptors"],
        json!({
            "mcpServer": {
                "data": "{\"tools\":[\"charge\"]}",
                "dataSchemaVersion": "2025-06-18",
            },
            "http": {},
        }),
        "BatchGet must return the stored descriptors: {from_batch}"
    );
    assert_eq!(
        from_batch["customMetadata"],
        json!({ "environment": "production", "requiresApproval": false }),
        "BatchGet must return the stored customMetadata: {from_batch}"
    );
    assert_eq!(
        from_batch["displayName"], json!("Payments MCP"),
        "BatchGet must return the stored displayName: {from_batch}"
    );

    let (status, search) = post(
        &router,
        "/discoverable-records-search",
        &json!({ "searchQuery": "payments-mcp", "registryIds": [REGISTRY], "maxResults": 10 }),
    )
    .await;
    assert_eq!(status, 200, "Search: expected 200, got {status}: {search}");
    let from_search = &search["registryRecords"][0];
    assert_eq!(
        from_search, from_batch,
        "a record must render identically in Search and BatchGet: {from_search} vs {from_batch}"
    );
}

#[tokio::test]
async fn search_matches_name_display_name_and_description_case_insensitively() {
    let router = seeded_router();
    // (searchQuery, expected recordIds in relevance order)
    let cases: [(&str, Vec<&str>); 8] = [
        // `payments` is in rec000000001's name and rec000000004's displayName, so
        // the name match ranks first.
        ("payments", vec!["rec000000001", "rec000000004"]),
        ("PAYMENTS", vec!["rec000000001", "rec000000004"]),
        ("Payments", vec!["rec000000001", "rec000000004"]),
        ("support-agent", vec!["rec000000002"]),
        // Only in a description.
        ("conversations", vec!["rec000000002"]),
        // Only in a displayName.
        ("payments skills", vec!["rec000000004"]),
        // An empty query matches every discoverable record.
        ("", DISCOVERABLE.to_vec()),
        ("no-such-substring", vec![]),
    ];
    for (query, expected) in cases {
        let (status, body) = post(
            &router,
            "/discoverable-records-search",
            &json!({ "searchQuery": query, "registryIds": [REGISTRY], "maxResults": 20 }),
        )
        .await;
        assert_eq!(status, 200, "case {query:?}: expected 200, got {status}: {body}");
        let found = record_ids(query, &body);
        assert_eq!(
            found, expected,
            "case {query:?}: expected {expected:?}, got {found:?}"
        );
    }
}

#[tokio::test]
async fn search_truncates_to_max_results_defaulting_to_ten() {
    let router = seeded_router();
    // (maxResults in the request, expected result count) over a registry holding
    // more records than the documented default page.
    let cases: [(Option<i64>, usize); 5] = [
        (Some(1), 1),
        (Some(10), 10),
        (Some(20), BULK_RECORDS),
        (Some(12), 12),
        // Absent: the model declares `@default: 10`.
        (None, 10),
    ];
    for (max_results, expected) in cases {
        let case = format!("maxResults={max_results:?}");
        let mut body = json!({ "searchQuery": "bulk", "registryIds": [BULK_REGISTRY] });
        if let Some(max) = max_results {
            body["maxResults"] = json!(max);
        }
        let (status, response) = post(&router, "/discoverable-records-search", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
        let found = record_ids(&case, &response);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} records, got {found:?}"
        );
    }
}

#[tokio::test]
async fn search_applies_the_documented_metadata_filter_operators() {
    let router = seeded_router();
    // The operators the `filters` member documents: $eq, $ne, $in, $and, $or, over
    // a record field and over a `customMetadata.{key}` field, with a string and a
    // native JSON boolean operand.
    let cases: [(&str, Value, Vec<&str>); 9] = [
        (
            "customMetadata.environment $eq production",
            json!({ "customMetadata.environment": { "$eq": "production" } }),
            vec!["rec000000001"],
        ),
        (
            "customMetadata.environment $ne production",
            json!({ "customMetadata.environment": { "$ne": "production" } }),
            vec!["rec000000002", "rec000000004"],
        ),
        (
            "customMetadata.environment $in {production,staging}",
            json!({ "customMetadata.environment": { "$in": ["production", "staging"] } }),
            vec!["rec000000001", "rec000000002"],
        ),
        (
            "customMetadata.requiresApproval $eq false (native boolean)",
            json!({ "customMetadata.requiresApproval": { "$eq": false } }),
            vec!["rec000000001"],
        ),
        (
            "recordType $eq AGENT",
            json!({ "recordType": { "$eq": "AGENT" } }),
            vec!["rec000000002"],
        ),
        (
            "status $in {APPROVED}",
            json!({ "status": { "$in": ["APPROVED"] } }),
            DISCOVERABLE.to_vec(),
        ),
        (
            "$or over recordType",
            json!({ "$or": [
                { "recordType": { "$eq": "MCP" } },
                { "recordType": { "$eq": "SKILL" } },
            ] }),
            vec!["rec000000001", "rec000000004"],
        ),
        (
            "$and over recordType and customMetadata",
            json!({ "$and": [
                { "recordType": { "$eq": "MCP" } },
                { "customMetadata.environment": { "$eq": "production" } },
            ] }),
            vec!["rec000000001"],
        ),
        (
            "$and that nothing satisfies",
            json!({ "$and": [
                { "recordType": { "$eq": "MCP" } },
                { "customMetadata.environment": { "$eq": "staging" } },
            ] }),
            vec![],
        ),
    ];
    for (case, filters, expected) in cases {
        let (status, body) = post(
            &router,
            "/discoverable-records-search",
            &json!({
                "searchQuery": "",
                "registryIds": [REGISTRY],
                "maxResults": 20,
                "filters": filters,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        let mut found = record_ids(case, &body);
        found.sort();
        let mut expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "case {case:?}: expected {expected:?}, got {found:?}"
        );
    }
}

#[tokio::test]
async fn input_outside_a_constraint_or_the_documented_grammar_is_refused_with_400() {
    let router = seeded_router();
    // (case, path, body) — each one violates a modelled constraint or the filter
    // grammar the `filters` member documents.
    let long_query = "q".repeat(300);
    let many_record_ids: Vec<String> = (1..=101).map(|n| format!("rec{n:09}")).collect();
    let cases: [(&str, &str, Value); 17] = [
        (
            "searchQuery longer than 256",
            "/discoverable-records-search",
            json!({ "searchQuery": long_query, "registryIds": [REGISTRY], "maxResults": 10 }),
        ),
        (
            "maxResults above the documented search maximum",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 21 }),
        ),
        (
            "maxResults explicitly zero (search)",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 0 }),
        ),
        (
            "maxResults below the documented search minimum",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": -1 }),
        ),
        (
            "registryIds empty",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [], "maxResults": 10 }),
        ),
        (
            // `registryIds` is modelled @length{min:1,max:1}.
            "registryIds names two registries",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY, BULK_REGISTRY], "maxResults": 10 }),
        ),
        (
            "filters names a field that is not filterable",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 10,
                    "filters": { "nope": { "$eq": "x" } } }),
        ),
        (
            "filters names an operator the service does not support",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 10,
                    "filters": { "name": { "$gt": "a" } } }),
        ),
        (
            "filters is not an object",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 10,
                    "filters": ["name"] }),
        ),
        (
            "filters carries an empty $in",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 10,
                    "filters": { "recordType": { "$in": [] } } }),
        ),
        (
            "filters hides a bad operator behind a term that does not hold",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 10,
                    "filters": { "$and": [
                        { "recordType": { "$eq": "NOTHING_MATCHES" } },
                        { "name": { "$gt": "a" } },
                    ] } }),
        ),
        (
            "recordIds empty",
            "/discoverable-records-batch",
            json!({ "entries": [{ "registryId": REGISTRY, "recordIds": [] }] }),
        ),
        (
            // `recordIds` is modelled @length{min:1,max:100}.
            "recordIds beyond the documented maximum of 100",
            "/discoverable-records-batch",
            json!({ "entries": [{ "registryId": REGISTRY, "recordIds": many_record_ids }] }),
        ),
        (
            // `entries` is modelled @length{min:1,max:1}.
            "entries names two registries",
            "/discoverable-records-batch",
            json!({ "entries": [
                { "registryId": REGISTRY, "recordIds": ["rec000000001"] },
                { "registryId": BULK_REGISTRY, "recordIds": ["bulk00000001"] },
            ] }),
        ),
        (
            "entries absent",
            "/discoverable-records-batch",
            json!({}),
        ),
        (
            "maxResults above the documented list maximum",
            "/registries/abc123def456/discoverable-records-list",
            json!({ "maxResults": 101 }),
        ),
        (
            "maxResults explicitly zero (list)",
            "/registries/abc123def456/discoverable-records-list",
            json!({ "maxResults": 0 }),
        ),
    ];
    for (case, path, body) in cases {
        let (status, response) = post(&router, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 ValidationException, got {status}: {response}"
        );
        assert!(
            response["__type"]
                .as_str()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {response}"
        );
    }

    // More than the modelled maximum of 10 filters.
    let filters: Vec<Value> = (0..11)
        .map(|_| json!({ "name": "recordType", "values": ["MCP"] }))
        .collect();
    let (status, response) = post(
        &router,
        &list_path(REGISTRY),
        &json!({ "filters": filters }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"11 filters\": expected 400, got {status}: {response}"
    );
}

#[tokio::test]
async fn a_malformed_filter_is_refused_whatever_the_store_holds() {
    let router = seeded_router();
    // The filter grammar is a property of the request, so the same bad filter must
    // be refused against a registry holding discoverable records and against one
    // holding none — refusal cannot depend on what the store happens to contain.
    for (case, registry) in [
        ("registry with discoverable records", REGISTRY),
        ("registry with no discoverable record", EMPTY_REGISTRY),
    ] {
        let (status, body) = post(
            &router,
            "/discoverable-records-search",
            &json!({
                "searchQuery": "",
                "registryIds": [registry],
                "maxResults": 10,
                "filters": { "nope": { "$eq": "x" } },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a filter naming an unfilterable field must be refused, got {status}: {body}"
        );
    }

    // A well-formed filter against the same empty registry is a plain empty page.
    let (status, body) = post(
        &router,
        "/discoverable-records-search",
        &json!({
            "searchQuery": "",
            "registryIds": [EMPTY_REGISTRY],
            "maxResults": 10,
            "filters": { "recordType": { "$eq": "CUSTOM" } },
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        record_ids("empty registry", &body).is_empty(),
        "a registry with no discoverable record must answer with none, got {body}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = seeded_router();
    for token in ["not-a-token", "rec000000001", "ffff"] {
        let (status, body) = post(
            &router,
            &list_path(REGISTRY),
            &json!({ "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status}: {body}"
        );
        let reason = body["reason"]
            .as_str()
            .unwrap_or_else(|| panic!("case {token:?}: reason must be a string: {body}"));
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
            "case {token:?}: reason must be one of the model's ValidationExceptionReason values {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?}"
        );
    }

    // A token this service DID issue, but for another registry, is refused too: it
    // names a position in the registry it was minted for.
    let (status, body) = post(
        &router,
        &list_path(BULK_REGISTRY),
        &json!({ "maxResults": 1 }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let foreign_token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first bulk page must carry a token: {body}"))
        .to_string();
    let (status, body) = post(
        &router,
        &list_path(REGISTRY),
        &json!({ "nextToken": foreign_token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a token minted for another registry must be refused, got {status}: {body}"
    );
}

#[tokio::test]
async fn reading_does_not_change_what_a_later_caller_sees() {
    let router = seeded_router();
    let mut seen = Vec::new();
    for round in 0..3 {
        let (status, body) = post(&router, &list_path(REGISTRY), &json!({})).await;
        assert_eq!(status, 200, "round {round}: expected 200, got {status}: {body}");
        seen.push(record_ids("repeat", &body));

        let (status, body) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": REGISTRY, "recordIds": ["rec000000001"] }] }),
        )
        .await;
        assert_eq!(status, 200, "round {round}: expected 200, got {status}: {body}");

        let (status, body) = post(
            &router,
            "/discoverable-records-search",
            &json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 20 }),
        )
        .await;
        assert_eq!(status, 200, "round {round}: expected 200, got {status}: {body}");
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "a read must not change what a later caller sees, got {seen:?}"
    );
}

/// A `searchQuery` is a natural-language phrase, so a record matches when every term
/// of the query occurs in its searchable text — in any order, across fields, and not
/// necessarily adjacently — and a term no record carries narrows the answer to none.
#[tokio::test]
async fn search_matches_every_term_of_a_phrase_in_any_order() {
    let router = seeded_router();
    // (searchQuery, expected recordIds in relevance order). The seeded text is
    // rec000000001 "payments-mcp" / "Payments MCP" / "Processes payment intents",
    // rec000000002 "support-agent" / "Handles support conversations" and
    // rec000000004 "skills-pack" / "Payments skills".
    let cases: [(&str, Vec<&str>); 10] = [
        // Every term of the phrase is in one description, in another order than the
        // description words them.
        ("intents processes payment", vec!["rec000000001"]),
        ("payment intents", vec!["rec000000001"]),
        // Both terms in the record's name.
        ("mcp payments", vec!["rec000000001"]),
        // One term in the name, the other only in the description.
        ("support conversations", vec!["rec000000002"]),
        // Casing is not part of the match.
        ("HANDLES Support", vec!["rec000000002"]),
        // One term in the name, the other only in the displayName, and the query is
        // written with the whitespace a person types.
        ("  skills   payments  ", vec!["rec000000004"]),
        // Terms are conjunctive: no record carries both of these.
        ("payments conversations", vec![]),
        ("payments mcp skills", vec![]),
        // A phrase nothing in the registry is described by.
        ("tide forecast harbor", vec![]),
        // A single term in two records ranks the one matching the more specific field
        // first: a name match before a displayName match.
        ("payments", vec!["rec000000001", "rec000000004"]),
    ];
    for (query, expected) in cases {
        let (status, body) = post(
            &router,
            "/discoverable-records-search",
            &json!({ "searchQuery": query, "registryIds": [REGISTRY], "maxResults": 20 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status}: {body}"
        );
        let found = record_ids(query, &body);
        assert_eq!(
            found, expected,
            "case {query:?}: expected {expected:?}, got {found:?}"
        );
    }
}
