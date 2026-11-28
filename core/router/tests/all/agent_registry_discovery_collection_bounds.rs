// SPDX-License-Identifier: CC0-1.0

//! A modelled collection bound on an Agent Registry discovery read is enforced ONCE, at the
//! request boundary.
//!
//! The rule under test: a `@length` bound the model states on a list member of a request is
//! enforced by the framework's constraint validator before the operation runs — so a request
//! inside every bound reaches the handler and is answered from stored state, and a request
//! outside any one of them is refused `400 ValidationException` carrying the single sentence
//! that validator words, whichever of the three discovery reads was asked.
//!
//! The rows span every list member the three reads model a bound on, each at its published
//! edge and each one element past it:
//! `SearchDiscoverableRegistryRecordsRequest$registryIds` (`@length{min:1,max:1}`),
//! `BatchGetDiscoverableRegistryRecordRequest$entries` (`@length{min:1,max:1}`),
//! `RegistryRecordsEntry$recordIds` (`@length{min:1,max:100}`) and
//! `ListDiscoverableRegistryRecordsRequest$filters` (`@length{max:10}`) — every bound from
//! the model (`smithy_query(query="closure", operation="BatchGetDiscoverableRegistryRecord,\
//! ListDiscoverableRegistryRecords,SearchDiscoverableRegistryRecords")`).
//!
//! Both Agent Registry planes are mounted on one engine because a discovery read reports the
//! CONTROL plane's resources: the registry and the record every served row reads back are
//! created through the control plane's own operations.
//!
//! Every wait here is a wait on the CONDITION — a poll loop up to a generous deadline — so a
//! loaded host only polls more times.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Percent-encode one URI path segment the way an SDK encodes a non-greedy `@httpLabel`.
/// This crate's one encoder, shared rather than copied.
use crate::cloudfront_key_value_store_handshake::segment as encode_label;

/// The region in the credential scope every call carries.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` header whose credential scope names the signing name both Agent
/// Registry models publish. The signature is never checked: this router runs with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The hosts that name the two planes' endpoints — the `endpointPrefix` each model
/// publishes.
const CONTROL_HOST: &str = "agent-registry-control.us-west-2.amazonaws.com";
const DATA_HOST: &str = "agent-registry.us-west-2.amazonaws.com";

/// The data plane's two registry-less routes, from the model's `http` bindings.
const SEARCH_PATH: &str = "/discoverable-records-search";
const BATCH_PATH: &str = "/discoverable-records-batch";

/// How long a wait for a transition the clock owes (a registry becoming `READY`, a record
/// becoming `DRAFT` and then `APPROVED`) may take before a test gives up. Generous on
/// purpose: the wait is on the CONDITION, so a loaded host only polls more times.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop pauses between two looks at a resource.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `RegistryRecordFilterList` is modelled `@length{max:10}`.
const MOST_FILTERS: usize = 10;

/// `RegistryRecordsEntry$recordIds` is modelled `@length{min:1,max:100}`.
const MOST_RECORD_IDS: usize = 100;

/// A registry identifier of `RegistryIdentifier`'s shape (`^[a-zA-Z0-9]{12,16}$` or the
/// registry ARN) that names no registry this engine holds: a request carrying it alongside
/// another is refused for the BOUND, before any registry is resolved, so the answer is the
/// validator's 400 and not a 404.
const UNHELD_REGISTRY_ID: &str = "unheldreg001";

/// One HTTP answer, reduced to what these rows assert on.
struct Reply {
    status: u16,
    /// The wire error code, from `x-amzn-errortype` or the body's `__type`.
    code: String,
    body: Value,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The `recordId` of every summary the answer reports, in response order.
    fn reported_record_ids(&self) -> Vec<String> {
        self.body["registryRecords"]
            .as_array()
            .map(|records| {
                records
                    .iter()
                    .map(|record| record["recordId"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A router carrying both Agent Registry planes over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_agentregistry_agentregistry::EmulatedService::new(engine),
    );
    router
}

async fn call(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    let code = code.rsplit(['#', '/']).next().unwrap_or_default().to_string();
    Reply { status, code, body }
}

/// The id at the end of a minted ARN.
fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// The data plane's registry-scoped list route for `registry_id`.
fn list_path(registry_id: &str) -> String {
    format!(
        "/registries/{}/discoverable-records-list",
        encode_label(registry_id)
    )
}

/// The envelope the framework's constraint validator states one violation in.
const ONE_VIOLATION: &str = "1 validation error detected: Value at '";

/// The requirement end of that sentence: the bound the member's value had to satisfy, with
/// nothing of the collection quoted — a list has no scalar rendering, so neither its element
/// count nor its elements reach the sentence.
fn length_requirement(requirement: &str) -> String {
    format!("failed to satisfy constraint: Member must have length {requirement}")
}

/// `count` well-formed `filters` entries, each narrowing `recordType` to the type the seeded
/// record carries, so a list inside the bound still reports that record.
fn filters(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|_| json!({ "name": "recordType", "values": [RECORD_TYPE] }))
            .collect(),
    )
}

/// The `recordType` of the record every served row reads back, from the model's `RecordType`
/// enum.
const RECORD_TYPE: &str = "MCP";

/// `count` record ids of `RecordIdentifier`'s shape (`^[a-zA-Z0-9]{12}$`), the last of which
/// is `held`: a batch read inside the bound reports that one and an error per id the registry
/// does not expose.
fn record_ids(count: usize, held: &str) -> Value {
    let mut ids: Vec<String> = (1..count).map(|index| format!("rec{index:09}")).collect();
    ids.push(held.to_string());
    Value::Array(ids.into_iter().map(Value::String).collect())
}

/// Create a registry whose approval configuration approves every submission, and wait for the
/// creation the clock owes so it is `READY`. Returns its id.
async fn ready_registry(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = call(
        router,
        CONTROL_HOST,
        "POST",
        "/registries",
        // `APPROVE_ALL` is the one `AutoApprovalRule` the model declares, and it approves a
        // record on submission (`smithy_query(query="shape", shape="ApprovalConfiguration")`).
        Some(json!({
            "name": name,
            "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] }
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistry answers 202, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let registry_arn = created.str_field("registryArn").to_string();
    assert!(
        registry_arn.contains(REGION),
        "case {case:?}: a registry ARN carries the Region the call was scoped to, got \
         {registry_arn:?}"
    );
    let registry_id = id_of(&registry_arn).to_string();

    let started = std::time::Instant::now();
    loop {
        let got = call(
            router,
            CONTROL_HOST,
            "GET",
            &format!("/registries/{}", encode_label(&registry_id)),
            None,
        )
        .await;
        if got.str_field("status") == "READY" {
            return registry_id;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: a created registry becomes READY, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at a record through the control plane until it reports `wanted`, or give up at the
/// deadline.
async fn record_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    record_id: &str,
    wanted: &str,
) {
    let started = std::time::Instant::now();
    loop {
        let got = call(
            router,
            CONTROL_HOST,
            "GET",
            &format!(
                "/registries/{}/records/{}",
                encode_label(registry_id),
                encode_label(record_id)
            ),
            None,
        )
        .await;
        if got.str_field("status") == wanted {
            return;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the record never reached {wanted}, last answer {} {:?} with {:?}",
            got.status,
            got.code,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a record in `registry_id` and take it through the registry's approval workflow,
/// which approves it on submission — the status that puts it on the discovery surface.
/// Returns its record id.
async fn approved_record(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    name: &str,
) -> String {
    let created = call(
        router,
        CONTROL_HOST,
        "POST",
        &format!("/registries/{}/records", encode_label(registry_id)),
        Some(json!({
            "name": name,
            "recordType": RECORD_TYPE,
            // Member names are `Descriptors`' own
            // (`smithy_query(query="shape", shape="Descriptors")`).
            "descriptors": {
                "mcpServer": {
                    "data": r#"{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2"}"#,
                    "dataSchemaVersion": "2025-06-18"
                }
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistryRecord answers 202 for {name:?}, got {} {:?} with {:?}",
        created.status, created.code, created.body
    );
    let record_id = id_of(created.str_field("recordArn")).to_string();
    record_reaching(router, case, registry_id, &record_id, "DRAFT").await;

    let submitted = call(
        router,
        CONTROL_HOST,
        "POST",
        &format!(
            "/registries/{}/records/{}/submit-for-approval",
            encode_label(registry_id),
            encode_label(&record_id)
        ),
        None,
    )
    .await;
    assert_eq!(
        submitted.status, 202,
        "case {case:?}: SubmitRegistryRecordForApproval answers 202 for {name:?}, got {} {:?} \
         with {:?}",
        submitted.status, submitted.code, submitted.body
    );
    record_reaching(router, case, registry_id, &record_id, "APPROVED").await;
    record_id
}

#[tokio::test]
async fn a_modelled_collection_bound_of_a_discovery_read_is_enforced_once_at_the_boundary() {
    let case = "collection bounds";
    let router = router();
    let registry_id = ready_registry(&router, case, "discovery-collection-bounds").await;
    let record_id = approved_record(&router, case, &registry_id, "payments-mcp").await;

    // Inside every modelled bound, including AT the published edge: the read runs and
    // answers from the record the control plane holds.
    let served: Vec<(&str, String, Value)> = vec![
        (
            "filters at its published maximum of ten",
            list_path(&registry_id),
            json!({ "filters": filters(MOST_FILTERS) }),
        ),
        (
            "filters below its maximum",
            list_path(&registry_id),
            json!({ "filters": filters(1) }),
        ),
        (
            "registryIds at the one length it admits",
            SEARCH_PATH.to_string(),
            json!({ "searchQuery": "", "registryIds": [&registry_id] }),
        ),
        (
            "entries at its maximum of one, recordIds at its maximum of a hundred",
            BATCH_PATH.to_string(),
            json!({ "entries": [
                { "registryId": &registry_id,
                  "recordIds": record_ids(MOST_RECORD_IDS, &record_id) },
            ] }),
        ),
        (
            "entries at its maximum of one, recordIds at its minimum of one",
            BATCH_PATH.to_string(),
            json!({ "entries": [
                { "registryId": &registry_id, "recordIds": [&record_id] },
            ] }),
        ),
    ];

    for (row, path, body) in &served {
        let reply = call(&router, DATA_HOST, "POST", path, Some(body.clone())).await;
        assert_eq!(
            reply.status, 200,
            "case {row:?}: a request inside every modelled bound is served, got {} {:?} with \
             {:?}",
            reply.status, reply.code, reply.body
        );
        let reported = reply.reported_record_ids();
        assert!(
            reported.contains(&record_id),
            "case {row:?}: the approved record the control plane holds is reported, got \
             {reported:?}"
        );
    }

    // One element outside a bound: refused by the validator, in its one sentence, at the
    // member the bound sits at — whether that member is the request's own or one a list
    // element declares, which the validator states under the path it reached it by.
    let refused: Vec<(&str, String, Value, &str, String)> = vec![
        (
            "registryIds one past its maximum of one",
            SEARCH_PATH.to_string(),
            json!({ "searchQuery": "", "registryIds": [&registry_id, UNHELD_REGISTRY_ID] }),
            "registryIds",
            length_requirement("less than or equal to 1"),
        ),
        (
            "registryIds below its minimum of one",
            SEARCH_PATH.to_string(),
            json!({ "searchQuery": "", "registryIds": [] }),
            "registryIds",
            length_requirement("greater than or equal to 1"),
        ),
        (
            "entries one past its maximum of one",
            BATCH_PATH.to_string(),
            json!({ "entries": [
                { "registryId": &registry_id, "recordIds": [&record_id] },
                { "registryId": &registry_id, "recordIds": [&record_id] },
            ] }),
            "entries",
            length_requirement("less than or equal to 1"),
        ),
        (
            "entries below its minimum of one",
            BATCH_PATH.to_string(),
            json!({ "entries": [] }),
            "entries",
            length_requirement("greater than or equal to 1"),
        ),
        (
            "recordIds one past its maximum of a hundred",
            BATCH_PATH.to_string(),
            json!({ "entries": [
                { "registryId": &registry_id,
                  "recordIds": record_ids(MOST_RECORD_IDS + 1, &record_id) },
            ] }),
            "recordIds",
            length_requirement("less than or equal to 100"),
        ),
        (
            "recordIds below its minimum of one",
            BATCH_PATH.to_string(),
            json!({ "entries": [{ "registryId": &registry_id, "recordIds": [] }] }),
            "recordIds",
            length_requirement("greater than or equal to 1"),
        ),
        (
            "filters one past its maximum of ten",
            list_path(&registry_id),
            json!({ "filters": filters(MOST_FILTERS + 1) }),
            "filters",
            length_requirement("less than or equal to 10"),
        ),
    ];

    for (row, path, body, member, requirement) in &refused {
        let reply = call(&router, DATA_HOST, "POST", path, Some(body.clone())).await;
        assert_eq!(
            reply.status, 400,
            "case {row:?}: a collection outside its modelled bound is refused 400, got {} {:?} \
             with {:?}",
            reply.status, reply.code, reply.body
        );
        assert_eq!(
            reply.code, "ValidationException",
            "case {row:?}: expected ValidationException, got {:?}",
            reply.code
        );
        let stated = reply.str_field("message").to_string();
        assert!(
            stated.starts_with(ONE_VIOLATION)
                && stated.contains(&format!("{member}' "))
                && stated.ends_with(requirement.as_str()),
            "case {row:?}: expected one violation stated at {member:?} requiring length \
             {requirement:?}, got {stated:?}"
        );
        assert!(
            !stated.contains("items") && !stated.contains("Value '"),
            "case {row:?}: nothing of the collection may be quoted, got {stated:?}"
        );
    }
}
