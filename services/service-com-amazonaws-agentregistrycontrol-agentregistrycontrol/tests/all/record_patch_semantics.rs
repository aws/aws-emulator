// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's two update operations — `UpdateRegistryRecord` and
//! `UpdateRegistry` — driven over HTTP through the real router so every assertion goes
//! through routing, deserialization, the handler and serialization.
//!
//! The rule under test: an update is a PATCH of the STORED resource — "specify only the
//! fields you want to change, and omit the rest to leave them unchanged", a wrapper
//! carrying a value replaces the stored value and a wrapper supplied empty unsets the
//! field it wraps — and the update is asynchronous, so the resource carries `UPDATING`
//! while it is processed and then settles. What was written is what every later read
//! reports, since every read path projects the one stored resource.
//!
//! Every test asserts over a TABLE of inputs, so it holds for a class of requests rather
//! than for one recorded shape: the rows span which member is patched, whether it is set
//! or unset, how deep in the wrapper tree it sits, the type of the value (string,
//! document, nested structure, list), the number of members patched at once, and the status
//! the resource is in when the edit arrives.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::state as control;
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`) and the handlers read the region from the credential scope. The
/// signature itself is never checked: the test router runs with authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// How long a wait for a transition the clock owes may take before a test gives up.
/// Generous on purpose: the wait is on the CONDITION, so a loaded host only polls more.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll loop pauses between two looks at a resource.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// An MCP server definition a supported MCP server schema version accepts: the MCP server
/// descriptor's content is validated against that schema, so a patch that writes one carries
/// a named server with a description and a version.
const MCP_SERVER_DEFINITION: &str =
    r#"{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2"}"#;

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// The router and the engine behind it, so a test can put a record into a status only this
/// service's own asynchronous work reaches.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// Percent-encode a path segment the way an SDK encodes a non-greedy `@httpLabel`, so an
/// ARN (which carries `:` and `/`) can be given where the model allows the ARN form.
fn encode_label(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

async fn get_record(router: &MultiServiceRouter, registry: &str, record: &str) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry),
            encode_label(record)
        ),
        None,
    )
    .await
}

async fn update_record(
    router: &MultiServiceRouter,
    registry: &str,
    record: &str,
    body: Value,
) -> Reply {
    call(
        router,
        "PATCH",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry),
            encode_label(record)
        ),
        Some(body),
    )
    .await
}

async fn list_records(router: &MultiServiceRouter, registry: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records-list", encode_label(registry)),
        Some(json!({})),
    )
    .await
}

/// Look at a record until it reports `wanted`, or give up at the deadline. The wait is on
/// the CONDITION — a transition the clock owes — never on a fixed span.
async fn record_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry: &str,
    record: &str,
    wanted: &str,
) -> Reply {
    let started = std::time::Instant::now();
    loop {
        let reply = get_record(router, registry, record).await;
        if reply.str_field("status") == wanted {
            return reply;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the record never reached {wanted}, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `READY` registry, created through this service's own operations. `body` carries the
/// create request's members.
async fn ready_registry(router: &MultiServiceRouter, case: &str, body: Value) -> (String, String) {
    let created = call(router, "POST", "/registries", Some(body)).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistry must answer 202, got {} {:?}",
        created.status, created.body
    );
    let registry_arn = created.str_field("registryArn").to_string();
    let registry_id = id_of(&registry_arn).to_string();

    let started = std::time::Instant::now();
    loop {
        let reply = call(
            router,
            "GET",
            &format!("/registries/{}", encode_label(&registry_id)),
            None,
        )
        .await;
        if reply.str_field("status") == "READY" {
            break;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the registry never became READY, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    (registry_id, registry_arn)
}

/// The members a record in these tests is created with, so every row starts from one
/// known content and what an omitted member leaves alone is visible. `name` is the row's,
/// since "names are unique within a registry".
fn created_with(name: &str) -> Value {
    json!({
        "name": name,
        "displayName": "the display name it was created with",
        "description": "the description it was created with",
        "recordType": "CUSTOM",
        "recordVersion": "1.0.0",
        "descriptors": { "custom": { "data": "{\"created\":true}" } },
        "customMetadata": { "team": "catalogue", "critical": true },
    })
}

/// Create a record named `name` with [`created_with`] and wait for the creation the clock
/// owes, so it is `DRAFT`. Returns (recordId, recordArn).
async fn draft_record(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    name: &str,
) -> (String, String) {
    let created = call(
        router,
        "POST",
        &format!("/registries/{}/records", encode_label(registry_id)),
        Some(created_with(name)),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistryRecord must answer 202, got {} {:?}",
        created.status, created.body
    );
    let record_arn = created.str_field("recordArn").to_string();
    let record_id = id_of(&record_arn).to_string();
    record_reaching(router, case, registry_id, &record_id, "DRAFT").await;
    (record_id, record_arn)
}

/// Assert that `body` carries exactly `expected` for every member `expected` names, where
/// a `Value::Null` expectation means the member must be ABSENT.
fn assert_members(case: &str, body: &Value, expected: &Map<String, Value>, through: &str) {
    for (member, wanted) in expected {
        let actual = body.get(member).unwrap_or(&Value::Null);
        assert_eq!(
            actual,
            wanted,
            "case {case:?}: {through} must report {member} as {wanted:?}, got {actual:?} \
             (whole body {body:?})"
        );
    }
}

#[tokio::test]
async fn a_patch_is_applied_to_the_stored_record_and_served_by_every_read_path() {
    let (router, _engine) = router();
    let (registry_id, _registry_arn) = ready_registry(
        &router,
        "patch semantics",
        json!({ "name": "patch-semantics-registry" }),
    )
    .await;

    // (case, the PATCH body, what the record carries afterwards). Only the members a row
    // names change: every other member must still report what the record was created
    // with, which is what "omit the field to leave it unchanged" means. A `null`
    // expectation means the member is absent from the answer.
    //
    // The wrapper shapes come from the model: a leaf wrapper is
    // `{"optionalValue": <value>}` and an empty `{}` unsets the field
    // (`smithy_query(query="shape", shape="UpdatedDisplayName,UpdatedDescriptors")`).
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "set displayName",
            json!({ "displayName": { "optionalValue": "a display name set by the update" } }),
            json!({ "displayName": "a display name set by the update" }),
        ),
        (
            "unset displayName with an empty wrapper",
            json!({ "displayName": {} }),
            json!({ "displayName": null }),
        ),
        (
            "set description",
            json!({ "description": { "optionalValue": "a description set by the update" } }),
            json!({ "description": "a description set by the update" }),
        ),
        (
            "unset description with an empty wrapper",
            json!({ "description": {} }),
            json!({ "description": null }),
        ),
        (
            "change recordVersion",
            json!({ "recordVersion": "2.0.0" }),
            json!({ "recordVersion": "2.0.0" }),
        ),
        (
            "patch a descriptor leaf",
            json!({
                "descriptors": {
                    "optionalValue": {
                        "custom": { "optionalValue": { "data": { "optionalValue": "{\"patched\":true}" } } }
                    }
                }
            }),
            json!({ "descriptors": { "custom": { "data": "{\"patched\":true}" } } }),
        ),
        (
            "add a second descriptor of the typed set, leaving the first alone",
            json!({
                "descriptors": {
                    "optionalValue": {
                        "mcpServer": {
                            "optionalValue": {
                                "data": { "optionalValue": MCP_SERVER_DEFINITION },
                                "dataSchemaVersion": { "optionalValue": "2025-06-18" }
                            }
                        }
                    }
                }
            }),
            json!({
                "recordType": "CUSTOM",
                "descriptors": {
                    "custom": { "data": "{\"created\":true}" },
                    "mcpServer": { "data": MCP_SERVER_DEFINITION, "dataSchemaVersion": "2025-06-18" }
                }
            }),
        ),
        (
            "patch a descriptor nested two wrappers deep",
            json!({
                "recordType": "MCP",
                "descriptors": {
                    "optionalValue": {
                        "custom": {},
                        "mcpServer": {
                            "optionalValue": {
                                "data": { "optionalValue": MCP_SERVER_DEFINITION },
                                "additionalData": {
                                    "optionalValue": {
                                        "tools": {
                                            "optionalValue": {
                                                "data": { "optionalValue": "{\"count\":1}" },
                                                "dataSchemaVersion": { "optionalValue": "2025-06-18" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }),
            json!({
                "recordType": "MCP",
                "descriptors": {
                    "mcpServer": {
                        "data": MCP_SERVER_DEFINITION,
                        "additionalData": {
                            "tools": { "data": "{\"count\":1}", "dataSchemaVersion": "2025-06-18" }
                        }
                    }
                }
            }),
        ),
        (
            "clear the whole descriptor set with an empty wrapper",
            json!({ "descriptors": {} }),
            json!({ "descriptors": null }),
        ),
        (
            "replace customMetadata",
            json!({ "customMetadata": { "optionalValue": { "team": "platform", "critical": false, "tier": "1" } } }),
            json!({ "customMetadata": { "team": "platform", "critical": false, "tier": "1" } }),
        ),
        (
            "clear customMetadata with an empty wrapper",
            json!({ "customMetadata": {} }),
            json!({ "customMetadata": null }),
        ),
        (
            "patch the signature agent card descriptor, leaving the first alone",
            json!({
                "descriptors": {
                    "optionalValue": {
                        "signatureAgentCard": {
                            "optionalValue": {
                                "data": { "optionalValue": "{\"name\":\"crawler\"}" },
                                "dataSchemaVersion": { "optionalValue": "0.3.0" },
                                "additionalData": {
                                    "optionalValue": {
                                        "category": { "optionalValue": "ai_search" }
                                    }
                                }
                            }
                        }
                    }
                }
            }),
            json!({
                "descriptors": {
                    "custom": { "data": "{\"created\":true}" },
                    "signatureAgentCard": {
                        "data": "{\"name\":\"crawler\"}",
                        "dataSchemaVersion": "0.3.0",
                        "additionalData": { "category": "ai_search" }
                    }
                }
            }),
        ),
        (
            "patch every leaf of the agent-skills additional data",
            json!({
                "descriptors": {
                    "optionalValue": {
                        "agentSkillsDefinition": {
                            "optionalValue": {
                                "data": { "optionalValue": "{\"skills\":[]}" },
                                "additionalData": {
                                    "optionalValue": {
                                        "skillPackage": {
                                            "optionalValue": {
                                                "source": {
                                                    "fromUrl": { "url": "https://example.test/skill.zip" }
                                                }
                                            }
                                        },
                                        "repositoryProvider": { "optionalValue": "GITHUB" },
                                        "sourceConfiguration": {
                                            "optionalValue": {
                                                "s3": { "bucketOwnerAccountId": "000000000000" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }),
            json!({
                "descriptors": {
                    "custom": { "data": "{\"created\":true}" },
                    "agentSkillsDefinition": {
                        "data": "{\"skills\":[]}",
                        "additionalData": {
                            "skillPackage": {
                                "source": { "fromUrl": { "url": "https://example.test/skill.zip" } }
                            },
                            "repositoryProvider": "GITHUB",
                            "sourceConfiguration": {
                                "s3": { "bucketOwnerAccountId": "000000000000" }
                            }
                        }
                    }
                }
            }),
        ),
        (
            "patch two members in one request",
            json!({
                "displayName": { "optionalValue": "both at once" },
                "description": {}
            }),
            json!({ "displayName": "both at once", "description": null }),
        ),
    ];

    for (index, (case, patch, changes)) in rows.into_iter().enumerate() {
        let name = format!("patch-probe-{index}");
        let (record_id, record_arn) = draft_record(&router, case, &registry_id, &name).await;

        // What the record must report afterwards: what it was created with, overwritten
        // by the row's expectations.
        let mut expected: Map<String, Value> = created_with(&name)
            .as_object()
            .expect("an object")
            .clone();
        for (member, value) in changes.as_object().expect("an object") {
            expected.insert(member.clone(), value.clone());
        }

        let updated = update_record(&router, &registry_id, &record_id, patch.clone()).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: UpdateRegistryRecord must answer 202, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            (
                updated.str_field("recordId"),
                updated.str_field("recordArn"),
                updated.str_field("status"),
            ),
            (record_id.as_str(), record_arn.as_str(), "UPDATING"),
            "case {case:?}: the answer addresses the record it patched and reports the \
             asynchronous status, got {:?}",
            updated.body
        );
        // The answer already carries the patched content, since it reports the record
        // that was just written.
        assert_members(case, &updated.body, &expected, "the update's own answer");

        // The record settles back at DRAFT (the status it was in), and the read path
        // reports the patch.
        let read_back = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
        assert_members(case, &read_back.body, &expected, "GetRegistryRecord");

        // The list path projects the SAME stored record, so the patch is visible there
        // too — for the members a `RegistryRecordSummary` carries.
        let listed = list_records(&router, &registry_id).await;
        let summary = listed.body["registryRecords"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .find(|record| record["recordId"].as_str() == Some(record_id.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        let summarised: Map<String, Value> = ["name", "displayName", "description", "recordType", "recordVersion"]
            .into_iter()
            .map(|member| {
                (
                    member.to_string(),
                    expected.get(member).cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        assert_members(case, &summary, &summarised, "the ListRegistryRecords summary");
    }
}

#[tokio::test]
async fn a_rename_is_persisted_and_keeps_record_names_unique_in_the_registry() {
    let (router, _engine) = router();
    let (registry_id, _registry_arn) = ready_registry(
        &router,
        "rename",
        json!({ "name": "rename-registry" }),
    )
    .await;
    let (record_id, _record_arn) = draft_record(&router, "rename", &registry_id, "first").await;
    let (other_id, _other_arn) = draft_record(&router, "rename", &registry_id, "second").await;

    // Rows span the names a rename can ask for: a free one, the record's own name (the
    // same record, so not a duplicate), and one another record of the registry holds.
    let rows: Vec<(&str, &str, Option<u16>)> = vec![
        ("a name no record holds", "first-renamed", None),
        ("the name the record already holds", "first-renamed", None),
        ("a name another record of the registry holds", "second", Some(409)),
    ];
    for (case, wanted, refused_with) in rows {
        let answered = update_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "name": wanted }),
        )
        .await;
        match refused_with {
            None => {
                assert_eq!(
                    (answered.status, answered.str_field("name")),
                    (202, wanted),
                    "case {case:?}: the rename is accepted and answered, got {} {:?}",
                    answered.status,
                    answered.body
                );
                let read_back =
                    record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
                assert_eq!(
                    read_back.str_field("name"),
                    wanted,
                    "case {case:?}: a read reports the renamed record, got {:?}",
                    read_back.body
                );
            }
            Some(status) => {
                assert_eq!(
                    (
                        answered.status,
                        answered.error_code.as_deref(),
                        answered.message()
                    ),
                    (
                        status,
                        Some("ConflictException"),
                        control::duplicate_record_name_message(wanted, &registry_id)
                    ),
                    "case {case:?}: expected this service's duplicate-name refusal, got {} \
                     {:?}",
                    answered.status,
                    answered.body
                );
                // The refused rename wrote nothing: the other record keeps its name and
                // this one keeps the name it had.
                let read_back = get_record(&router, &registry_id, &record_id).await;
                assert_eq!(
                    read_back.str_field("name"),
                    "first-renamed",
                    "case {case:?}: a refused rename leaves the stored name alone, got {:?}",
                    read_back.body
                );
                let untouched = get_record(&router, &registry_id, &other_id).await;
                assert_eq!(
                    untouched.str_field("name"),
                    "second",
                    "case {case:?}: a refused rename leaves the other record alone, got {:?}",
                    untouched.body
                );
            }
        }
    }
}

/// One row of the edit-settles-where table: the status a record is in when an edit
/// arrives, and where the edit leaves it.
struct EditRow {
    case: &'static str,
    /// Whether the record's registry approves a submission instead of sending it to
    /// review, which is how a row reaches `APPROVED`.
    auto_approve: bool,
    /// The status the record is in when the edit arrives.
    start_status: &'static str,
    /// The reason it carries then, set with the status when the row needs one.
    start_reason: Option<&'static str>,
    /// The status the record settles at once the update has been processed.
    settles_at: &'static str,
    /// The reason it carries afterwards.
    reason_after: Option<&'static str>,
}

#[tokio::test]
async fn an_edit_settles_the_record_at_the_status_its_curation_state_decides() {
    let (router, _engine) = router();

    // An edit of a record that is in the registry's approval workflow takes it back out of
    // that workflow: the content a curator saw is not the content the record now holds, so
    // the record settles at DRAFT and can be submitted again. A record that is not in the
    // workflow — a DRAFT — settles back where it was. The rows are every status an edit can
    // arrive in: the asynchronous ones are refused while the change is in flight and the
    // terminal one is refused outright, both in the suites below.
    let rows: Vec<EditRow> = vec![
        EditRow {
            case: "a draft record",
            auto_approve: false,
            start_status: "DRAFT",
            start_reason: None,
            settles_at: "DRAFT",
            reason_after: None,
        },
        EditRow {
            case: "a record waiting for review",
            auto_approve: false,
            start_status: "PENDING_APPROVAL",
            start_reason: None,
            settles_at: "DRAFT",
            reason_after: None,
        },
        EditRow {
            case: "a rejected record",
            auto_approve: true,
            start_status: "REJECTED",
            start_reason: Some("descriptor does not describe a reachable endpoint"),
            settles_at: "DRAFT",
            reason_after: None,
        },
        EditRow {
            case: "an approved record",
            auto_approve: true,
            start_status: "APPROVED",
            start_reason: Some("reviewed by the catalogue team"),
            // An edit invalidates the approval it was made against, so the record is
            // withdrawn from the workflow and the reason that described the status it has
            // left goes with it.
            settles_at: "DRAFT",
            reason_after: None,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let EditRow {
            case,
            auto_approve,
            start_status,
            start_reason,
            settles_at,
            reason_after,
        } = row;
        let mut registry_body = json!({ "name": format!("edit-registry-{index}") });
        if auto_approve {
            registry_body["approvalConfiguration"] = json!({ "autoApprovalRules": ["APPROVE_ALL"] });
        }
        let (registry_id, _registry_arn) = ready_registry(&router, case, registry_body).await;
        let (record_id, _record_arn) = draft_record(&router, case, &registry_id, "edited").await;

        if start_status != "DRAFT" {
            let submitted = call(
                &router,
                "POST",
                &format!(
                    "/registries/{}/records/{}/submit-for-approval",
                    encode_label(&registry_id),
                    encode_label(&record_id)
                ),
                None,
            )
            .await;
            assert_eq!(
                submitted.status, 202,
                "case {case:?}: SubmitRegistryRecordForApproval must answer 202, got {} {:?}",
                submitted.status, submitted.body
            );
            if let Some(reason) = start_reason {
                let moved = call(
                    &router,
                    "PATCH",
                    &format!(
                        "/registries/{}/records/{}/status",
                        encode_label(&registry_id),
                        encode_label(&record_id)
                    ),
                    Some(json!({ "status": start_status, "statusReason": reason })),
                )
                .await;
                assert_eq!(
                    moved.status, 202,
                    "case {case:?}: UpdateRegistryRecordStatus must answer 202, got {} {:?}",
                    moved.status, moved.body
                );
            }
            record_reaching(&router, case, &registry_id, &record_id, start_status).await;
        }

        let updated = update_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "displayName": { "optionalValue": "edited while in the workflow" } }),
        )
        .await;
        assert_eq!(
            (updated.status, updated.str_field("status")),
            (202, "UPDATING"),
            "case {case:?}: an accepted update reports the asynchronous status, got {} {:?}",
            updated.status,
            updated.body
        );

        let settled = record_reaching(&router, case, &registry_id, &record_id, settles_at).await;
        assert_eq!(
            settled.str_field("displayName"),
            "edited while in the workflow",
            "case {case:?}: the settled record carries the edit, got {:?}",
            settled.body
        );
        assert_eq!(
            settled.body.get("statusReason").and_then(Value::as_str),
            reason_after,
            "case {case:?}: expected statusReason {reason_after:?} after the edit, got {:?}",
            settled.body
        );
    }
}

/// Submit a record into its registry's approval workflow, asserting the submission was
/// accepted.
async fn submit_for_approval(
    router: &MultiServiceRouter,
    case: &str,
    registry: &str,
    record: &str,
) -> Reply {
    let submitted = call(
        router,
        "POST",
        &format!(
            "/registries/{}/records/{}/submit-for-approval",
            encode_label(registry),
            encode_label(record)
        ),
        None,
    )
    .await;
    assert_eq!(
        submitted.status, 202,
        "case {case:?}: SubmitRegistryRecordForApproval must answer 202, got {} {:?}",
        submitted.status, submitted.body
    );
    submitted
}

/// One row of the withdrawal table: the workflow status the record holds when an edit
/// arrives, and the form that edit takes.
struct WithdrawalRow {
    case: &'static str,
    /// The approval-workflow status the record is in when the edit arrives.
    start_status: &'static str,
    /// The edit's request body.
    patch: Value,
    /// The members the edited record reports afterwards, over what it was created with; a
    /// `null` expectation means the member is absent.
    changes: Value,
}

/// An edit of a record that is inside its registry's approval workflow WITHDRAWS it from
/// that workflow: the record goes through the asynchronous `UPDATING` status, settles at
/// `DRAFT` carrying no status reason, serves the edited members on every read path, and can
/// be submitted for approval again — whichever workflow status the edit arrived in and
/// whichever members it carried.
#[tokio::test]
async fn an_edit_inside_the_approval_workflow_withdraws_the_record_to_draft() {
    let (router, _engine) = router();
    // No approval configuration, so a submission goes to a curator rather than being
    // approved outright: that is what puts a row's record under review.
    let (registry_id, _registry_arn) = ready_registry(
        &router,
        "withdrawal",
        json!({ "name": "withdrawal-registry" }),
    )
    .await;

    // The reason a curator's rejection leaves on the record, so a row that starts from
    // `REJECTED` can show the edit clears it.
    let rejection_reason = "the descriptor names no reachable endpoint";

    // The rows span BOTH workflow statuses an edit can withdraw a record from and the form
    // of the edit: a leaf replaced, a leaf unset, two members at once, a map replaced and a
    // nested descriptor patched. The wrapper shapes are the model's, as above.
    let rows: Vec<WithdrawalRow> = vec![
        WithdrawalRow {
            case: "a description replaced while the record waits for review",
            start_status: "PENDING_APPROVAL",
            patch: json!({ "description": { "optionalValue": "coastal flood alerts" } }),
            changes: json!({ "description": "coastal flood alerts" }),
        },
        WithdrawalRow {
            case: "a description unset while the record waits for review",
            start_status: "PENDING_APPROVAL",
            patch: json!({ "description": {} }),
            changes: json!({ "description": null }),
        },
        WithdrawalRow {
            case: "a version and a display name changed while the record waits for review",
            start_status: "PENDING_APPROVAL",
            patch: json!({
                "recordVersion": "3.1.4",
                "displayName": { "optionalValue": "edited under review" }
            }),
            changes: json!({ "recordVersion": "3.1.4", "displayName": "edited under review" }),
        },
        WithdrawalRow {
            case: "a description replaced after the record was rejected",
            start_status: "REJECTED",
            patch: json!({ "description": { "optionalValue": "river gauge alerts" } }),
            changes: json!({ "description": "river gauge alerts" }),
        },
        WithdrawalRow {
            case: "the custom metadata replaced after the record was rejected",
            start_status: "REJECTED",
            patch: json!({ "customMetadata": { "optionalValue": { "team": "weather", "critical": false } } }),
            changes: json!({ "customMetadata": { "team": "weather", "critical": false } }),
        },
        WithdrawalRow {
            case: "a descriptor leaf patched after the record was rejected",
            start_status: "REJECTED",
            patch: json!({
                "descriptors": {
                    "optionalValue": {
                        "custom": { "optionalValue": { "data": { "optionalValue": "{\"edited\":true}" } } }
                    }
                }
            }),
            changes: json!({ "descriptors": { "custom": { "data": "{\"edited\":true}" } } }),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let WithdrawalRow {
            case,
            start_status,
            patch,
            changes,
        } = row;
        let name = format!("withdrawal-probe-{index}");
        let (record_id, _record_arn) = draft_record(&router, case, &registry_id, &name).await;

        // Into the workflow: a submission puts the record under review, and a curator's
        // rejection is the second workflow status an edit can arrive in.
        submit_for_approval(&router, case, &registry_id, &record_id).await;
        if start_status == "REJECTED" {
            let rejected = call(
                &router,
                "PATCH",
                &format!(
                    "/registries/{}/records/{}/status",
                    encode_label(&registry_id),
                    encode_label(&record_id)
                ),
                Some(json!({ "status": "REJECTED", "statusReason": rejection_reason })),
            )
            .await;
            assert_eq!(
                rejected.status, 202,
                "case {case:?}: UpdateRegistryRecordStatus must answer 202, got {} {:?}",
                rejected.status, rejected.body
            );
        }
        let under_review =
            record_reaching(&router, case, &registry_id, &record_id, start_status).await;
        assert_eq!(
            under_review.body.get("statusReason").and_then(Value::as_str),
            (start_status == "REJECTED").then_some(rejection_reason),
            "case {case:?}: the record must be in the workflow before the edit, got {:?}",
            under_review.body
        );

        // What the withdrawn record must report: what it was created with, overwritten by
        // the row's edit.
        let mut expected: Map<String, Value> =
            created_with(&name).as_object().expect("an object").clone();
        for (member, value) in changes.as_object().expect("an object") {
            expected.insert(member.clone(), value.clone());
        }

        let updated = update_record(&router, &registry_id, &record_id, patch).await;
        assert_eq!(
            (updated.status, updated.str_field("status")),
            (202, "UPDATING"),
            "case {case:?}: an accepted edit reports the asynchronous status, got {} {:?}",
            updated.status,
            updated.body
        );

        // The edit takes the record out of the approval workflow: it settles at DRAFT,
        // carries no reason for a workflow status it is no longer in, and serves the edit.
        let settled = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
        assert_members(case, &settled.body, &expected, "GetRegistryRecord");
        assert_eq!(
            settled.body.get("statusReason").and_then(Value::as_str),
            None,
            "case {case:?}: a withdrawn record keeps no status reason, got {:?}",
            settled.body
        );

        // The list path projects the same stored record, so the withdrawal and the edit are
        // visible there too.
        let listed = list_records(&router, &registry_id).await;
        let summary = listed.body["registryRecords"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .find(|record| record["recordId"].as_str() == Some(record_id.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        let mut summarised: Map<String, Value> =
            ["name", "displayName", "description", "recordType", "recordVersion"]
                .into_iter()
                .map(|member| {
                    (
                        member.to_string(),
                        expected.get(member).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect();
        summarised.insert("status".to_string(), json!("DRAFT"));
        assert_members(case, &summary, &summarised, "the ListRegistryRecords summary");

        // A withdrawn record is a draft again, so the workflow accepts it anew — which is
        // what a client that edits a record under review waits for DRAFT in order to do.
        submit_for_approval(&router, case, &registry_id, &record_id).await;
        let resubmitted =
            record_reaching(&router, case, &registry_id, &record_id, "PENDING_APPROVAL").await;
        assert_eq!(
            resubmitted.str_field("status"),
            "PENDING_APPROVAL",
            "case {case:?}: a withdrawn record can be submitted for approval again, got {:?}",
            resubmitted.body
        );
    }
}

#[tokio::test]
async fn an_update_this_service_cannot_apply_is_refused_and_writes_nothing() {
    let (router, engine) = router();
    let (registry_id, registry_arn) = ready_registry(
        &router,
        "refusals",
        json!({ "name": "refusal-registry" }),
    )
    .await;
    let (record_id, record_arn) = draft_record(&router, "refusals", &registry_id, "refused").await;

    // A registry id and a record id of the modelled shapes (`RegistryId`
    // ^[a-zA-Z0-9]{12,16}$, `RegistryRecordId` ^[a-zA-Z0-9]{12}$) that this service holds
    // nothing under, each given bare and as the resource's full ARN, so the handler and
    // not the boundary validator is what refuses them.
    let absent = "zzz999999999";
    let absent_registry_arn =
        format!("arn:aws:agent-registry:{REGION}:000000000000:registry/{absent}");
    let absent_record_arn = format!("{registry_arn}/record/{absent}");
    let not_found: Vec<(&str, String, String, String)> = vec![
        (
            "a registry this service holds nothing under",
            absent.to_string(),
            record_id.clone(),
            control::registry_not_found_message(absent),
        ),
        (
            "the same registry named by its ARN",
            absent_registry_arn.clone(),
            record_id.clone(),
            control::registry_not_found_message(&absent_registry_arn),
        ),
        (
            "a record this registry holds nothing under",
            registry_id.clone(),
            absent.to_string(),
            control::record_not_found_message(absent),
        ),
        (
            "the same record named by its ARN",
            registry_arn.clone(),
            absent_record_arn.clone(),
            control::record_not_found_message(absent),
        ),
    ];
    for (case, registry, record, expected) in not_found {
        let refused = update_record(
            &router,
            &registry,
            &record,
            json!({ "displayName": { "optionalValue": "never stored" } }),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.error_code.as_deref(),
                refused.message()
            ),
            (404, Some("ResourceNotFoundException"), expected.clone()),
            "case {case:?}: expected the modelled 404 with this service's own sentence \
             ({expected:?}), got {} {:?}",
            refused.status,
            refused.body
        );
    }

    // `provenance` is reserved for the service principal that auto-detects records, so a
    // caller asserting a lineage for itself is refused. The entry satisfies
    // `Provenance`'s own members (`relation`, `sourceId` matching its ARN pattern).
    let forbidden = update_record(
        &router,
        &registry_id,
        &record_id,
        json!({
            "provenance": [{
                "relation": "DETECTED_FROM",
                "sourceId": format!("arn:aws:bedrock-agentcore:{REGION}:000000000000:runtime/abc"),
            }]
        }),
    )
    .await;
    assert_eq!(
        (forbidden.status, forbidden.error_code.as_deref()),
        (403, Some("AccessDeniedException")),
        "case \"provenance from another caller\": expected the modelled 403, got {} {:?}",
        forbidden.status,
        forbidden.body
    );

    // A record whose own asynchronous change is still in flight has no settled content to
    // patch. Only this service's own work puts a record in one of those statuses, so each
    // row is staged in its store directly.
    for in_flight in ["CREATING", "UPDATING"] {
        {
            let state = engine
                .state::<control::ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>();
            let mut state = state.lock();
            let record = state
                .registries
                .get_mut(&registry_id)
                .and_then(|registry| registry.records.get_mut(&record_id))
                .expect("the record the suite created");
            record.status = in_flight.to_string();
            record.pending = None;
        }
        let refused = update_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "displayName": { "optionalValue": "never stored" } }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case \"a record in {in_flight}\": expected the modelled 409 ConflictException, \
             got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(in_flight),
            "case \"a record in {in_flight}\": the refusal names the status the record is \
             in, got {:?}",
            refused.message()
        );
    }

    // A retired record is in the lifecycle's terminal status, so no later change applies to
    // it — the sentence this service states for that class, from its one place.
    {
        let state =
            engine.state::<control::ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>();
        let mut state = state.lock();
        let record = state
            .registries
            .get_mut(&registry_id)
            .and_then(|registry| registry.records.get_mut(&record_id))
            .expect("the record the suite created");
        record.status = "DEPRECATED".to_string();
        record.pending = None;
    }
    let retired = update_record(
        &router,
        &registry_id,
        &record_id,
        json!({ "displayName": { "optionalValue": "never stored" } }),
    )
    .await;
    assert_eq!(
        (
            retired.status,
            retired.error_code.as_deref(),
            retired.message()
        ),
        (
            400,
            Some("ValidationException"),
            control::terminal_record_status_message("DEPRECATED")
        ),
        "case \"a retired record\": expected the modelled 400 with this service's terminal \
         sentence, got {} {:?}",
        retired.status,
        retired.body
    );

    // None of the refusals above wrote anything: the record still carries what it was
    // created with.
    {
        let state =
            engine.state::<control::ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>();
        let mut state = state.lock();
        let record = state
            .registries
            .get_mut(&registry_id)
            .and_then(|registry| registry.records.get_mut(&record_id))
            .expect("the record the suite created");
        record.status = control::RECORD_DRAFT.to_string();
    }
    let read_back = get_record(&router, &registry_id, &record_arn).await;
    assert_eq!(
        read_back.str_field("displayName"),
        "the display name it was created with",
        "case \"after every refusal\": a refused update leaves the stored record alone, \
         got {:?}",
        read_back.body
    );
}

#[tokio::test]
async fn record_names_are_unique_within_a_registry_and_not_across_registries() {
    let (router, _engine) = router();
    let (registry_id, registry_arn) = ready_registry(
        &router,
        "name scope",
        json!({ "name": "name-scope-registry" }),
    )
    .await;
    let (other_registry_id, _other_registry_arn) = ready_registry(
        &router,
        "name scope",
        json!({ "name": "name-scope-other-registry" }),
    )
    .await;
    let (record_id, _record_arn) = draft_record(&router, "name scope", &registry_id, "alpha").await;
    let (sibling_id, _sibling_arn) =
        draft_record(&router, "name scope", &registry_id, "beta").await;
    // A record of the OTHER registry, holding a name this record is renamed to below: the
    // scope a record name is unique within is one registry, so that name is free here.
    let (neighbour_id, _neighbour_arn) =
        draft_record(&router, "name scope", &other_registry_id, "gamma").await;

    // Rows span the names a rename can ask for and the identifier form the record's
    // registry is addressed by: a name nothing holds, the name the record already holds,
    // the name a sibling of the same registry holds (bare id and ARN), and a name only a
    // record of another registry holds.
    let rows: Vec<(&str, bool, &str, bool)> = vec![
        (
            "a name no record of the registry holds",
            false,
            "delta",
            false,
        ),
        ("the name the record already holds", false, "delta", false),
        (
            "a name a sibling of the same registry holds",
            false,
            "beta",
            true,
        ),
        (
            "the same clash with the registry given as its ARN",
            true,
            "beta",
            true,
        ),
        (
            "a name only a record of another registry holds",
            false,
            "gamma",
            false,
        ),
    ];

    for (case, by_arn, wanted, refused) in rows {
        let registry = if by_arn {
            registry_arn.as_str()
        } else {
            registry_id.as_str()
        };
        let answered =
            update_record(&router, registry, &record_id, json!({ "name": wanted })).await;
        if refused {
            let expected = control::duplicate_record_name_message(wanted, registry);
            assert_eq!(
                (
                    answered.status,
                    answered.error_code.as_deref(),
                    answered.message()
                ),
                (409, Some("ConflictException"), expected.clone()),
                "case {case:?}: expected this service's duplicate-name refusal \
                 ({expected:?}), got {} {:?}",
                answered.status,
                answered.body
            );
        } else {
            assert_eq!(
                (answered.status, answered.str_field("name")),
                (202, wanted),
                "case {case:?}: the rename is accepted and answered, got {} {:?}",
                answered.status,
                answered.body
            );
            let settled = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
            assert_eq!(
                settled.str_field("name"),
                wanted,
                "case {case:?}: a read reports the renamed record, got {:?}",
                settled.body
            );
        }
    }

    // No row touched another record: the sibling keeps the name a refused rename asked for,
    // and the record of the other registry keeps the name an accepted rename took.
    let sibling = get_record(&router, &registry_id, &sibling_id).await;
    assert_eq!(
        sibling.str_field("name"),
        "beta",
        "case \"after every rename\": the sibling keeps its own name, got {:?}",
        sibling.body
    );
    let neighbour = get_record(&router, &other_registry_id, &neighbour_id).await;
    assert_eq!(
        neighbour.str_field("name"),
        "gamma",
        "case \"after every rename\": the record of the other registry is untouched, got \
         {:?}",
        neighbour.body
    );
}

/// Put a registry into `status` directly in this service's store: only this service's own
/// asynchronous work reaches `CREATING` and `DELETING`, and neither lasts long enough for a
/// test to address one of the registry's records while it holds.
fn set_registry_status(engine: &Arc<engine::InMemoryEngine>, registry_id: &str, status: &str) {
    let state =
        engine.state::<control::ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>();
    let mut state = state.lock();
    let registry = state
        .registries
        .get_mut(registry_id)
        .expect("the registry the suite created");
    registry.status = status.to_string();
    registry.pending = None;
}

/// One row of the registry-state table: the status the record's registry is in when the
/// edit arrives, and whether the edit is accepted then.
struct RegistryStateRow {
    case: &'static str,
    registry_status: &'static str,
    accepted: bool,
}

#[tokio::test]
async fn an_edit_of_a_record_is_held_to_its_registry_being_ready() {
    let (router, engine) = router();
    let (registry_id, _registry_arn) = ready_registry(
        &router,
        "registry state",
        json!({ "name": "registry-state-registry" }),
    )
    .await;
    let (record_id, _record_arn) =
        draft_record(&router, "registry state", &registry_id, "held").await;

    // Rows span the statuses a registry can be in when an edit of one of its records
    // arrives: the two transient ones and the usable one.
    let rows: Vec<RegistryStateRow> = vec![
        RegistryStateRow {
            case: "a registry still being created",
            registry_status: control::REGISTRY_CREATING,
            accepted: false,
        },
        RegistryStateRow {
            case: "a registry being deleted",
            registry_status: control::REGISTRY_DELETING,
            accepted: false,
        },
        RegistryStateRow {
            case: "a ready registry",
            registry_status: control::REGISTRY_READY,
            accepted: true,
        },
    ];

    for row in rows {
        let RegistryStateRow {
            case,
            registry_status,
            accepted,
        } = row;
        set_registry_status(&engine, &registry_id, registry_status);
        let wanted = format!("edited with the registry in {registry_status}");
        let answered = update_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "displayName": { "optionalValue": wanted } }),
        )
        .await;
        if accepted {
            assert_eq!(
                (answered.status, answered.str_field("status")),
                (202, "UPDATING"),
                "case {case:?}: an accepted update reports the asynchronous status, got {} \
                 {:?}",
                answered.status,
                answered.body
            );
            let settled = record_reaching(&router, case, &registry_id, &record_id, "DRAFT").await;
            assert_eq!(
                settled.str_field("displayName"),
                wanted,
                "case {case:?}: the settled record carries the edit, got {:?}",
                settled.body
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: expected the modelled 409 ConflictException, got {} {:?}",
                answered.status,
                answered.body
            );
            assert!(
                answered.message().contains(registry_status),
                "case {case:?}: the refusal names the status the registry is in, got {:?}",
                answered.message()
            );
            // The refused edit wrote nothing: with the registry usable again the record
            // still carries the display name it was created with.
            set_registry_status(&engine, &registry_id, control::REGISTRY_READY);
            let read_back = get_record(&router, &registry_id, &record_id).await;
            assert_eq!(
                read_back.str_field("displayName"),
                "the display name it was created with",
                "case {case:?}: a refused update leaves the stored record alone, got {:?}",
                read_back.body
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// `UpdateRegistry`: the same PATCH rule, applied to the registry itself.
// ---------------------------------------------------------------------------------------

/// Read a registry: `GET /registries/{registryId}`.
async fn get_registry(router: &MultiServiceRouter, registry: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/registries/{}", encode_label(registry)),
        None,
    )
    .await
}

/// Patch a registry: `PATCH /registries/{registryId}`.
async fn update_registry(router: &MultiServiceRouter, registry: &str, body: Value) -> Reply {
    call(
        router,
        "PATCH",
        &format!("/registries/{}", encode_label(registry)),
        Some(body),
    )
    .await
}

/// Look at a registry until it reports `wanted`, or give up at the deadline. The wait is on
/// the CONDITION — a transition the clock owes — never on a fixed span.
async fn registry_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry: &str,
    wanted: &str,
) -> Reply {
    let started = std::time::Instant::now();
    loop {
        let reply = get_registry(router, registry).await;
        if reply.str_field("status") == wanted {
            return reply;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the registry never reached {wanted}, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A custom JWT authorizer configuration, the one member `AuthorizerConfiguration` declares
/// (`smithy_query(query="shape", shape="AuthorizerConfiguration")`). `audience` is what
/// makes one configuration distinguishable from another across a patch.
fn jwt_authorizer(audience: &str) -> Value {
    json!({
        "customJWTAuthorizer": {
            "discoveryUrl": "https://idp.example.test/.well-known/openid-configuration",
            "allowedAudience": [audience]
        }
    })
}

/// A `CustomMetadataSchemaDefinition`: the JSON Schema a registry declares its custom
/// metadata fields in, over the `properties` given.
fn schema_definition(properties: Value) -> String {
    json!({ "type": "object", "properties": properties }).to_string()
}

/// The fields the registries in these tests declare for every record type: a typed field, a
/// field with an enumerated set of allowed values, and an object field with a field of its
/// own, so a NESTED declaration is covered as well as a top-level one.
fn declared_properties() -> Value {
    json!({
        "owner": { "type": "string" },
        "tier": { "enum": ["gold", "silver"] },
        "contact": { "type": "object", "properties": { "email": { "type": "string" } } }
    })
}

/// A `CustomMetadataSchemaConfiguration` over the fields every record type is held to and
/// the per-record-type schemas.
fn schema_configuration(properties: Value, overrides: Value) -> Value {
    json!({
        "defaultSchema": schema_definition(properties),
        "recordTypeSchemaOverrides": overrides
    })
}

/// The per-record-type schemas of the registries in these tests: one for `CUSTOM`, a record
/// type of the model's own catalogue (`smithy_query(query="shape", shape="RecordType")`).
fn declared_overrides(custom_properties: Value) -> Value {
    json!([{ "recordType": "CUSTOM", "schema": schema_definition(custom_properties) }])
}

/// The custom-metadata schema configuration a registry in these tests is created with.
fn created_schema_configuration() -> Value {
    schema_configuration(
        declared_properties(),
        declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
    )
}

/// The members a registry in these tests is created with, so every row starts from one
/// known configuration and what an omitted member leaves alone is visible. `name` is the
/// row's, since a registry name "already exists in this account" is a conflict.
fn registry_created_with(name: &str) -> Value {
    json!({
        "name": name,
        "description": "the description it was created with",
        "discoveryConfiguration": {
            "authorizerType": "CUSTOM_JWT",
            "authorizerConfiguration": jwt_authorizer("audience-at-create")
        },
        "approvalConfiguration": { "autoApprovalRules": [] },
        "customMetadataSchemaConfiguration": created_schema_configuration(),
        "autoDetectionConfiguration": { "scope": "ORGANIZATION", "enabled": true }
    })
}

/// What a read reports for a registry created with [`registry_created_with`] before any
/// patch: the create's own members, with `autoDetectionConfiguration` reported as the
/// `autoDetection` the service derives from it.
fn registry_reports(name: &str) -> Map<String, Value> {
    json!({
        "name": name,
        "description": "the description it was created with",
        "discoveryConfiguration": {
            "authorizerType": "CUSTOM_JWT",
            "authorizerConfiguration": jwt_authorizer("audience-at-create")
        },
        "approvalConfiguration": { "autoApprovalRules": [] },
        "customMetadataSchemaConfiguration": created_schema_configuration(),
        "autoDetection": {
            "configuration": { "scope": "ORGANIZATION", "enabled": true },
            "status": "ACTIVE"
        }
    })
    .as_object()
    .expect("an object")
    .clone()
}

/// A registry created with [`registry_created_with`] and waited for, so it is `READY`.
async fn ready_configured_registry(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    ready_registry(router, case, registry_created_with(name)).await
}

#[tokio::test]
async fn a_registry_patch_is_applied_to_the_stored_registry_and_read_back_by_get_registry() {
    let (router, _engine) = router();

    // (case, the PATCH body, what the registry carries afterwards). Only the members a row
    // names change: every other member must still report what the registry was created
    // with, which is what "specify only the fields you want to change, and omit the rest to
    // leave them unchanged" means. A `null` expectation means the member is absent from the
    // answer.
    //
    // The wrapper shapes come from the model: a leaf wrapper is `{"optionalValue": <value>}`
    // and an empty `{}` unsets the field it wraps, while `UpdatedDiscoveryConfiguration`
    // patches its members one by one (`smithy_query(query="shape",
    // shape="UpdatedDescription,UpdatedApprovalConfiguration,UpdatedDiscoveryConfiguration")`).
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "set description",
            json!({ "description": { "optionalValue": "a description set by the update" } }),
            json!({ "description": "a description set by the update" }),
        ),
        (
            "unset description with an empty wrapper",
            json!({ "description": {} }),
            json!({ "description": null }),
        ),
        (
            "rename the registry",
            json!({ "name": "registry-patch-renamed" }),
            json!({ "name": "registry-patch-renamed" }),
        ),
        (
            "replace the discovery authorizer configuration",
            json!({
                "discoveryConfiguration": {
                    "authorizerConfiguration": {
                        "optionalValue": jwt_authorizer("audience-after-update")
                    }
                }
            }),
            json!({
                "discoveryConfiguration": {
                    "authorizerType": "CUSTOM_JWT",
                    "authorizerConfiguration": jwt_authorizer("audience-after-update")
                }
            }),
        ),
        (
            "unset the discovery authorizer configuration with an empty wrapper",
            json!({ "discoveryConfiguration": { "authorizerConfiguration": {} } }),
            json!({ "discoveryConfiguration": { "authorizerType": "CUSTOM_JWT" } }),
        ),
        (
            "a discovery patch that names no member of its own leaves it alone",
            json!({ "discoveryConfiguration": {} }),
            json!({}),
        ),
        (
            "replace the approval configuration with an auto-approval rule",
            json!({
                "approvalConfiguration": {
                    "optionalValue": { "autoApprovalRules": ["APPROVE_ALL"] }
                }
            }),
            json!({ "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] } }),
        ),
        (
            "unset the approval configuration with an empty wrapper",
            json!({ "approvalConfiguration": {} }),
            json!({ "approvalConfiguration": { "autoApprovalRules": [] } }),
        ),
        (
            "widen the custom metadata schema",
            json!({
                "customMetadataSchemaConfiguration": {
                    "optionalValue": schema_configuration(
                        json!({
                            "owner": { "type": "string" },
                            "tier": { "enum": ["gold", "silver"] },
                            "contact": {
                                "type": "object",
                                "properties": { "email": { "type": "string" } }
                            },
                            "costCentre": { "type": "string" }
                        }),
                        declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
                    )
                }
            }),
            json!({
                "customMetadataSchemaConfiguration": schema_configuration(
                    json!({
                        "owner": { "type": "string" },
                        "tier": { "enum": ["gold", "silver"] },
                        "contact": {
                            "type": "object",
                            "properties": { "email": { "type": "string" } }
                        },
                        "costCentre": { "type": "string" }
                    }),
                    declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
                )
            }),
        ),
        (
            "replace the auto-detection configuration",
            json!({
                "autoDetectionConfiguration": {
                    "optionalValue": { "scope": "ORGANIZATION", "enabled": false }
                }
            }),
            json!({
                "autoDetection": {
                    "configuration": { "scope": "ORGANIZATION", "enabled": false },
                    "status": "INACTIVE"
                }
            }),
        ),
        (
            "unset auto-detection with an empty wrapper",
            json!({ "autoDetectionConfiguration": {} }),
            json!({ "autoDetection": null }),
        ),
        (
            "patch two members in one request",
            json!({
                "name": "registry-patch-both-at-once",
                "description": {}
            }),
            json!({ "name": "registry-patch-both-at-once", "description": null }),
        ),
    ];

    for (index, (case, patch, changes)) in rows.into_iter().enumerate() {
        let name = format!("registry-patch-probe-{index}");
        let (registry_id, registry_arn) = ready_configured_registry(&router, case, &name).await;

        // What the registry must report afterwards: what it was created with, overwritten
        // by the row's expectations.
        let mut expected = registry_reports(&name);
        for (member, value) in changes.as_object().expect("an object") {
            expected.insert(member.clone(), value.clone());
        }

        let updated = update_registry(&router, &registry_id, patch).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: UpdateRegistry must answer 202, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            (
                updated.str_field("registryId"),
                updated.str_field("registryArn"),
                updated.str_field("status"),
            ),
            (registry_id.as_str(), registry_arn.as_str(), "UPDATING"),
            "case {case:?}: the answer addresses the registry it patched and reports the \
             asynchronous status, got {:?}",
            updated.body
        );
        // The answer already carries the patched configuration, since it reports the
        // registry that was just written.
        assert_members(case, &updated.body, &expected, "the update's own answer");

        // The update is asynchronous, so the registry settles back at READY, and the read
        // path reports the patch.
        let settled = registry_reaching(&router, case, &registry_id, "READY").await;
        assert_members(case, &settled.body, &expected, "GetRegistry");
    }
}

/// One row of the schema-evolution table: the configuration an update asks for, and whether
/// the registry's current one admits it.
struct SchemaEvolutionRow {
    case: &'static str,
    /// The `optionalValue` of the patch wrapper, or `Value::Null` for an empty wrapper —
    /// which asks for the configuration to be cleared.
    wanted: Value,
    /// The parts of the refusal that name what cannot be withdrawn, or `None` when the
    /// update is accepted.
    refusal: Option<Vec<&'static str>>,
}

#[tokio::test]
async fn the_custom_metadata_schema_may_only_be_widened() {
    let (router, _engine) = router();

    // A registry's custom metadata schema configuration "replaces the current one. You can
    // add fields and allowed values, but you can't remove a field or an allowed value, or
    // change a field's type". The rows span each of those three withdrawals and each way of
    // widening, at the top level of the schema every record type is held to, inside a
    // nested object field, and in the schema of one record type.
    let rows: Vec<SchemaEvolutionRow> = vec![
        SchemaEvolutionRow {
            case: "a field is added",
            wanted: schema_configuration(
                json!({
                    "owner": { "type": "string" },
                    "tier": { "enum": ["gold", "silver"] },
                    "contact": {
                        "type": "object",
                        "properties": { "email": { "type": "string" } }
                    },
                    "costCentre": { "type": "string" }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: None,
        },
        SchemaEvolutionRow {
            case: "an allowed value is added",
            wanted: schema_configuration(
                json!({
                    "owner": { "type": "string" },
                    "tier": { "enum": ["gold", "silver", "bronze"] },
                    "contact": {
                        "type": "object",
                        "properties": { "email": { "type": "string" } }
                    }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: None,
        },
        SchemaEvolutionRow {
            case: "a field is added inside a nested object field",
            wanted: schema_configuration(
                json!({
                    "owner": { "type": "string" },
                    "tier": { "enum": ["gold", "silver"] },
                    "contact": {
                        "type": "object",
                        "properties": {
                            "email": { "type": "string" },
                            "pager": { "type": "string" }
                        }
                    }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: None,
        },
        SchemaEvolutionRow {
            case: "a schema for another record type is added",
            wanted: schema_configuration(
                declared_properties(),
                json!([
                    {
                        "recordType": "CUSTOM",
                        "schema": schema_definition(
                            json!({ "environment": { "enum": ["dev", "prod"] } })
                        )
                    },
                    {
                        "recordType": "MCP",
                        "schema": schema_definition(json!({ "transport": { "type": "string" } }))
                    }
                ]),
            ),
            refusal: None,
        },
        SchemaEvolutionRow {
            case: "a field is removed",
            wanted: schema_configuration(
                json!({
                    "tier": { "enum": ["gold", "silver"] },
                    "contact": {
                        "type": "object",
                        "properties": { "email": { "type": "string" } }
                    }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: Some(vec!["'owner'", "record type 'default schema'"]),
        },
        SchemaEvolutionRow {
            case: "a field nested inside an object field is removed",
            wanted: schema_configuration(
                json!({
                    "owner": { "type": "string" },
                    "tier": { "enum": ["gold", "silver"] },
                    "contact": { "type": "object", "properties": {} }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: Some(vec!["'contact.email'", "record type 'default schema'"]),
        },
        SchemaEvolutionRow {
            case: "a field's type is changed",
            wanted: schema_configuration(
                json!({
                    "owner": { "type": "integer" },
                    "tier": { "enum": ["gold", "silver"] },
                    "contact": {
                        "type": "object",
                        "properties": { "email": { "type": "string" } }
                    }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: Some(vec!["'owner'", "from 'string' to 'integer'"]),
        },
        SchemaEvolutionRow {
            case: "an allowed value is removed",
            wanted: schema_configuration(
                json!({
                    "owner": { "type": "string" },
                    "tier": { "enum": ["gold"] },
                    "contact": {
                        "type": "object",
                        "properties": { "email": { "type": "string" } }
                    }
                }),
                declared_overrides(json!({ "environment": { "enum": ["dev", "prod"] } })),
            ),
            refusal: Some(vec!["enum value 'silver'", "field 'tier'"]),
        },
        SchemaEvolutionRow {
            case: "the schema of a record type is dropped",
            wanted: schema_configuration(declared_properties(), json!([])),
            refusal: Some(vec!["'environment'", "record type 'CUSTOM'"]),
        },
        SchemaEvolutionRow {
            case: "the whole configuration is cleared with an empty wrapper",
            wanted: Value::Null,
            refusal: Some(vec!["'contact'", "record type 'default schema'"]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let SchemaEvolutionRow {
            case,
            wanted,
            refusal,
        } = row;
        let name = format!("registry-schema-probe-{index}");
        let (registry_id, _registry_arn) = ready_configured_registry(&router, case, &name).await;

        // A wrapper carrying no value asks for the configuration to be cleared; one
        // carrying a value asks for it to be replaced by that value.
        let patch = match &wanted {
            Value::Null => json!({ "customMetadataSchemaConfiguration": {} }),
            value => json!({ "customMetadataSchemaConfiguration": { "optionalValue": value } }),
        };
        let answered = update_registry(&router, &registry_id, patch).await;

        match refusal {
            None => {
                assert_eq!(
                    answered.status, 202,
                    "case {case:?}: a widening of the schema must be accepted, got {} {:?}",
                    answered.status, answered.body
                );
                let settled = registry_reaching(&router, case, &registry_id, "READY").await;
                assert_eq!(
                    &settled.body["customMetadataSchemaConfiguration"], &wanted,
                    "case {case:?}: the registry reports the schema the update installed, \
                     got {:?}",
                    settled.body["customMetadataSchemaConfiguration"]
                );
            }
            Some(fragments) => {
                assert_eq!(
                    (answered.status, answered.error_code.as_deref()),
                    (400, Some("ValidationException")),
                    "case {case:?}: expected the modelled 400 ValidationException, got {} {:?}",
                    answered.status,
                    answered.body
                );
                for fragment in fragments {
                    assert!(
                        answered.message().contains(fragment),
                        "case {case:?}: the refusal names {fragment:?}, got {:?}",
                        answered.message()
                    );
                }
                // The refused update wrote nothing: the registry is still READY and still
                // declares the schema it was created with.
                let read_back = get_registry(&router, &registry_id).await;
                assert_eq!(
                    (
                        read_back.str_field("status"),
                        &read_back.body["customMetadataSchemaConfiguration"],
                    ),
                    ("READY", &created_schema_configuration()),
                    "case {case:?}: a refused update leaves the stored registry alone, got \
                     {:?}",
                    read_back.body
                );
            }
        }
    }
}

#[tokio::test]
async fn an_approval_configuration_update_decides_only_the_submissions_made_after_it() {
    let (router, _engine) = router();
    let (registry_id, _registry_arn) = ready_registry(
        &router,
        "approval configuration",
        json!({ "name": "approval-configuration-registry" }),
    )
    .await;

    // The updated approval configuration "applies only to records that move to
    // PENDING_APPROVAL after the update; records already in PENDING_APPROVAL are
    // unaffected". The rows span both directions of the change and both ways of asking for
    // manual review (an empty rule list, and no configuration at all), applied one after
    // another to the one registry, so each row also re-checks every record submitted under
    // an earlier configuration. The rule installed is the model's one `AutoApprovalRule`
    // (`smithy_query(query="shape", shape="AutoApprovalRule")`: APPROVE_ALL).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an auto-approval rule is installed",
            json!({ "optionalValue": { "autoApprovalRules": ["APPROVE_ALL"] } }),
            "APPROVED",
        ),
        (
            "the rule list is emptied",
            json!({ "optionalValue": { "autoApprovalRules": [] } }),
            "PENDING_APPROVAL",
        ),
        (
            "the rule is installed again",
            json!({ "optionalValue": { "autoApprovalRules": ["APPROVE_ALL"] } }),
            "APPROVED",
        ),
        (
            "the configuration is unset with an empty wrapper",
            json!({}),
            "PENDING_APPROVAL",
        ),
    ];

    // Every record submitted so far, with the status its own submission settled at.
    let mut submitted: Vec<(String, String)> = Vec::new();

    for (index, (case, wanted, settles_at)) in rows.into_iter().enumerate() {
        // A record submitted BEFORE the update, which the update must not move.
        let (before_id, _before_arn) =
            draft_record(&router, case, &registry_id, &format!("before-{index}")).await;
        let answered = submit_for_approval(&router, case, &registry_id, &before_id).await;
        submitted.push((before_id, answered.str_field("status").to_string()));

        let updated = update_registry(
            &router,
            &registry_id,
            json!({ "approvalConfiguration": wanted }),
        )
        .await;
        assert_eq!(
            (updated.status, updated.str_field("status")),
            (202, "UPDATING"),
            "case {case:?}: UpdateRegistry must accept the change and report the \
             asynchronous status, got {} {:?}",
            updated.status,
            updated.body
        );
        registry_reaching(&router, case, &registry_id, "READY").await;

        // Nothing already in the workflow moved: each record still reports the status its
        // own submission settled at, whichever configuration has been installed since.
        for (record_id, status) in &submitted {
            let read_back = get_record(&router, &registry_id, record_id).await;
            assert_eq!(
                read_back.str_field("status"),
                status,
                "case {case:?}: record {record_id} was submitted under an earlier \
                 configuration and must still report {status}, got {:?}",
                read_back.body
            );
        }

        // A submission made AFTER the update follows the configuration as it stands now.
        let (after_id, _after_arn) =
            draft_record(&router, case, &registry_id, &format!("after-{index}")).await;
        let answered = submit_for_approval(&router, case, &registry_id, &after_id).await;
        assert_eq!(
            answered.str_field("status"),
            settles_at,
            "case {case:?}: a submission made after the update settles at {settles_at}, got \
             {:?}",
            answered.body
        );
        let read_back = get_record(&router, &registry_id, &after_id).await;
        assert_eq!(
            read_back.str_field("status"),
            settles_at,
            "case {case:?}: the read path reports the submitted record as {settles_at}, got \
             {:?}",
            read_back.body
        );
        submitted.push((after_id, settles_at.to_string()));
    }
}

#[tokio::test]
async fn an_update_of_a_registry_is_held_to_it_being_ready() {
    let (router, engine) = router();
    let (registry_id, _registry_arn) = ready_registry(
        &router,
        "registry status",
        json!({ "name": "registry-update-status" }),
    )
    .await;

    // Rows span the statuses a registry can be in when an update arrives: the three
    // transient ones of `RegistryStatus` and the usable one
    // (`smithy_query(query="shape", shape="RegistryStatus")`).
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a registry still being created",
            control::REGISTRY_CREATING,
            false,
        ),
        (
            "a registry whose previous update is still being processed",
            control::REGISTRY_UPDATING,
            false,
        ),
        ("a registry being deleted", control::REGISTRY_DELETING, false),
        ("a ready registry", control::REGISTRY_READY, true),
    ];

    for (case, registry_status, accepted) in rows {
        set_registry_status(&engine, &registry_id, registry_status);
        let wanted = format!("updated with the registry in {registry_status}");
        let answered = update_registry(
            &router,
            &registry_id,
            json!({ "description": { "optionalValue": wanted.clone() } }),
        )
        .await;
        if accepted {
            assert_eq!(
                (answered.status, answered.str_field("status")),
                (202, "UPDATING"),
                "case {case:?}: an accepted update reports the asynchronous status, got {} \
                 {:?}",
                answered.status,
                answered.body
            );
            let settled = registry_reaching(&router, case, &registry_id, "READY").await;
            assert_eq!(
                settled.str_field("description"),
                wanted,
                "case {case:?}: the settled registry carries the update, got {:?}",
                settled.body
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: expected the modelled 409 ConflictException, got {} {:?}",
                answered.status,
                answered.body
            );
            assert!(
                answered.message().contains(registry_status),
                "case {case:?}: the refusal names the status the registry is in, got {:?}",
                answered.message()
            );
            // The refused update wrote nothing: with the registry usable again it still
            // carries no description at all, the way it was created.
            set_registry_status(&engine, &registry_id, control::REGISTRY_READY);
            let read_back = get_registry(&router, &registry_id).await;
            assert!(
                read_back.body["description"].is_null(),
                "case {case:?}: a refused update leaves the stored registry alone, got {:?}",
                read_back.body
            );
        }
    }
}

#[tokio::test]
async fn a_registry_rename_keeps_registry_names_unique_in_the_account() {
    let (router, _engine) = router();
    let (held_id, _held_arn) =
        ready_registry(&router, "held", json!({ "name": "rename-held" })).await;
    let (mover_id, _mover_arn) =
        ready_registry(&router, "mover", json!({ "name": "rename-mover" })).await;

    // (case, the name the rename asks for, whether it is accepted). A name another live
    // registry holds is a conflict; the registry's OWN name is not a duplicate of itself,
    // and a name nothing holds is free — including the one this registry has just left.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("onto the name another registry holds", "rename-held", false),
        ("onto its own name", "rename-mover", true),
        ("onto a free name", "rename-moved", true),
        ("onto the name it has just left", "rename-mover", true),
    ];

    for (case, wanted, accepted) in rows {
        let answered = update_registry(&router, &mover_id, json!({ "name": wanted })).await;
        if accepted {
            assert_eq!(
                answered.status, 202,
                "case {case:?}: the rename must be accepted, got {} {:?}",
                answered.status, answered.body
            );
            let settled = registry_reaching(&router, case, &mover_id, "READY").await;
            assert_eq!(
                settled.str_field("name"),
                wanted,
                "case {case:?}: the renamed registry reports its new name, got {:?}",
                settled.body
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: expected the modelled 409 ConflictException, got {} {:?}",
                answered.status,
                answered.body
            );
            let read_back = get_registry(&router, &mover_id).await;
            assert_eq!(
                read_back.str_field("name"),
                "rename-mover",
                "case {case:?}: a refused rename leaves the stored name alone, got {:?}",
                read_back.body
            );
        }
    }

    // The registry that held the contested name is untouched by the refused rename, the
    // name the mover ended up on is the one a create cannot take now, and the name it left
    // behind is free for a create.
    let held = get_registry(&router, &held_id).await;
    assert_eq!(
        held.str_field("name"),
        "rename-held",
        "the registry holding the name is untouched, got {:?}",
        held.body
    );
    let taken = call(
        &router,
        "POST",
        "/registries",
        Some(json!({ "name": "rename-mover" })),
    )
    .await;
    assert_eq!(
        (taken.status, taken.error_code.as_deref()),
        (409, Some("ConflictException")),
        "a create cannot take the name the rename moved onto, got {} {:?}",
        taken.status,
        taken.body
    );
    let freed = call(
        &router,
        "POST",
        "/registries",
        Some(json!({ "name": "rename-moved" })),
    )
    .await;
    assert_eq!(
        freed.status, 202,
        "the name the rename left is free for a create, got {} {:?}",
        freed.status, freed.body
    );
}

#[tokio::test]
async fn an_update_of_a_registry_nothing_holds_is_a_resource_not_found() {
    let (router, _engine) = router();

    // `RegistryIdentifier` admits the bare id or the registry's ARN, at the lengths the
    // model's own pattern allows, and the value that resolved to nothing is the one the
    // refusal quotes (`smithy_query(query="shape", shape="RegistryIdentifier")`).
    let rows: Vec<(&str, String)> = vec![
        ("a bare id nothing holds", "a1b2c3d4e5f6".to_string()),
        (
            "an ARN nothing holds",
            format!("arn:aws:agent-registry:{REGION}:000000000000:registry/a1b2c3d4e5f6"),
        ),
        (
            "a bare id of the longest form the model admits",
            "a1b2c3d4e5f6a1b2".to_string(),
        ),
    ];

    for (case, identifier) in rows {
        let answered = update_registry(
            &router,
            &identifier,
            json!({ "description": { "optionalValue": "nothing to update" } }),
        )
        .await;
        assert_eq!(
            (answered.status, answered.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the modelled 404 ResourceNotFoundException, got {} {:?}",
            answered.status,
            answered.body
        );
        assert!(
            answered.message().contains(&identifier),
            "case {case:?}: the refusal quotes the identifier as the request supplied it, \
             got {:?}",
            answered.message()
        );
    }
}
