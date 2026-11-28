// SPDX-License-Identifier: CC0-1.0

//! The Deployment read path, end to end over HTTP: `CreateDeployment`, `GetDeployment`
//! and `ListDeployments`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a client sees, not what a handler
//! returns.
//!
//! RULE under test: a read of one of this service's entities resolves its identifier
//! member (the entity's ARN, as the model documents it, or its bare id) against that
//! entity's store and answers `200` with the persisted record's members on a hit, and the
//! MODELLED `ResourceNotFoundException` (HTTP 404) on a miss — never a protocol-level
//! 501, which the operation's model does not declare. A read changes nothing: two reads
//! of one identifier answer alike, and a listing never surfaces an identifier no create
//! persisted.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// What a caller sees: the HTTP facts it branches on plus the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error code (`x-amzn-errortype`), absent on success.
    error_code: Option<String>,
    json: serde_json::Value,
}

impl Reply {
    fn string_at(&self, member: &str) -> String {
        self.json[member]
            .as_str()
            .unwrap_or_else(|| panic!("{member} is required, got {:?}", self.json))
            .to_string()
    }
}

/// The emulator under test: this one service mounted on the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The wire behaviour under test is the operations', not SigV4's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<serde_json::Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "network-security-manager.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The credential scope is what routes the request to this service and names
            // the region its ARNs carry.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/us-east-1/\
                 network-security-manager/aws4_request, SignedHeaders=host, Signature=0",
            )
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let json = if raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null)
        };
        Reply {
            status,
            error_code,
            json,
        }
    }

    async fn create_deployment(&self, body: serde_json::Value) -> Reply {
        self.call("POST", "/deployments", Some(body)).await
    }

    async fn get_deployment(&self, identifier: &str) -> Reply {
        self.call(
            "GET",
            &format!("/deployments/{}", path_label(identifier)),
            None,
        )
        .await
    }

    /// Follow `nextToken` to the end and return every `deploymentId` listed, in page
    /// order.
    async fn list_all_deployments(
        &self,
        max_results: Option<u32>,
        status: Option<&str>,
    ) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        // Bounded, so a token that failed to advance fails the test instead of hanging.
        for _ in 0..64 {
            let mut params: Vec<String> = Vec::new();
            if let Some(limit) = max_results {
                params.push(format!("maxResults={limit}"));
            }
            if let Some(filter) = status {
                params.push(format!("status={filter}"));
            }
            if let Some(token) = &next {
                params.push(format!("nextToken={token}"));
            }
            let path = if params.is_empty() {
                "/deployments".to_string()
            } else {
                format!("/deployments?{}", params.join("&"))
            };
            let reply = self.call("GET", &path, None).await;
            assert_eq!(
                reply.status, 200,
                "case listing {path}: expected 200, got {} {:?}",
                reply.status, reply.json
            );
            let page = reply.json["deployments"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case listing {path}: deployments is required, got {:?}",
                        reply.json
                    )
                })
                .clone();
            if let Some(limit) = max_results {
                assert!(
                    page.len() <= limit as usize,
                    "case listing {path}: a page holds at most maxResults={limit}, got {}",
                    page.len()
                );
            }
            for summary in &page {
                ids.push(
                    summary["deploymentId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case listing {path}: deploymentId is required, got {summary:?}")
                        })
                        .to_string(),
                );
            }
            match reply.json["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        !page.is_empty(),
                        "case listing {path}: a page that continues must not be empty"
                    );
                    next = Some(token.to_string());
                }
                None => return ids,
            }
        }
        panic!("case following nextToken: the listing never terminated");
    }

    /// A published policy, so a deployment can reference an ARN this emulator really
    /// minted.
    async fn create_policy(&self, name: &str) -> Reply {
        let reply = self
            .call(
                "POST",
                "/policies",
                Some(serde_json::json!({
                    "policyName": name,
                    "priority": 1,
                    "firewallType": "WAF",
                    // "For AWS WAF policies, specify 1 to 100 templates or rules."
                    "associatedTemplateAndRuleList": [{
                        "ruleIdentifier": resource_arn("rule", "deploymentrule"),
                    }],
                    "policyConfiguration": {
                        "remediationEnabled": false,
                        "resourcesCleanUp": true,
                        // A WAF policy carries the `wafConfig` members the model declares
                        // `@required` ("Required for AWS WAF policies").
                        "wafConfig": {
                            "existingCustomerWebACLResolution": "RETROFIT",
                            "conflictResolution": "MERGE_WHERE_APPLICABLE",
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case creating the policy {name}: expected 201, got {} {:?}",
            reply.status, reply.json
        );
        reply
    }
}

/// `deploymentIdentifier` is a NON-greedy `@httpLabel`, so an identifier containing `/` —
/// the deployment's ARN — reaches the server percent-encoded, exactly as an AWS SDK sends
/// it.
fn path_label(identifier: &str) -> String {
    identifier.replace('/', "%2F")
}

/// An ARN of one of this service's resource types, built from the same helper the
/// handlers mint ARNs with rather than from a hardcoded literal.
fn resource_arn(resource_type: &str, id: &str) -> String {
    engine::emulator_arn(
        "network-security-manager",
        "us-east-1",
        &format!("{resource_type}/{id}"),
    )
}

/// `DeploymentId`'s modelled pattern, `^[a-z0-9]{1,64}$`.
fn is_deployment_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// `EntityVersion`'s modelled pattern, `^[1-9][0-9]*$`.
fn is_entity_version(value: &str) -> bool {
    !value.starts_with('0') && !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// The `message` AWS states for this read's `ResourceNotFoundException`: the
/// `ServiceResourceType` of the entity being read, then the id the identifier names.
/// Pinned over the whole entity surface by `not_found_refusal_wording.rs`.
const NOT_FOUND_RESOURCE_TYPE: &str = "Deployment";

/// The id an identifier names: the resource segment of the ARN the model documents an
/// identifier to be, or the value itself when it is a bare id. This reading covers only
/// those two forms, which are the ones the table below spells; a version-qualified
/// identifier (whose `EntityVersion` suffix the service drops as well) is covered by
/// `not_found_refusal_wording.rs`.
fn entity_id_in(identifier: &str) -> &str {
    identifier.split('/').next_back().unwrap_or(identifier)
}

/// One row of the create table: what a `CreateDeployment` request may vary along.
#[derive(Debug, Clone)]
struct CreateCase {
    deployment_name: &'static str,
    description: Option<&'static str>,
    /// `None` exercises the modelled `@default: true` of `isPublished`.
    is_published: Option<bool>,
    cross_account_visibility: bool,
    /// How many policies the deployment associates (`AssociatedPolicyList` is
    /// `@length(1..2)`).
    policies: usize,
    tags: Option<serde_json::Value>,
    client_token: Option<&'static str>,
}

impl CreateCase {
    fn body(&self, policy_arns: &[String], scope_arn: &str) -> serde_json::Value {
        let mut body = serde_json::json!({
            "deploymentName": self.deployment_name,
            "deploymentConfiguration": {
                "enableCrossAccountVisibility": self.cross_account_visibility,
            },
            "associatedPolicyList": policy_arns
                .iter()
                .take(self.policies)
                .map(|arn| serde_json::json!({ "policyIdentifier": arn }))
                .collect::<Vec<_>>(),
            "associatedScopeList": [{ "scopeIdentifier": scope_arn }],
        });
        if let Some(description) = self.description {
            body["deploymentDescription"] = serde_json::json!(description);
        }
        if let Some(published) = self.is_published {
            body["isPublished"] = serde_json::json!(published);
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
        }
        if let Some(token) = self.client_token {
            body["clientToken"] = serde_json::json!(token);
        }
        body
    }

    /// The `EntityStatus` the model prescribes: ACTIVE when published, DRAFT otherwise,
    /// with `isPublished` defaulting to true.
    fn expected_status(&self) -> &'static str {
        if self.is_published.unwrap_or(true) {
            "ACTIVE"
        } else {
            "DRAFT"
        }
    }
}

/// The create table: rows differ along publish state (including the defaulted absence),
/// description presence, configuration flag, association count, tag presence and whether
/// an `@idempotencyToken` was sent.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            deployment_name: "published-one-policy",
            description: None,
            is_published: None,
            cross_account_visibility: false,
            policies: 1,
            tags: None,
            client_token: None,
        },
        CreateCase {
            deployment_name: "draft with description",
            description: Some("a draft deployment"),
            is_published: Some(false),
            cross_account_visibility: true,
            policies: 1,
            tags: None,
            client_token: Some("deployment-token-1"),
        },
        CreateCase {
            deployment_name: "two.policies/published",
            description: Some("both policies"),
            is_published: Some(true),
            cross_account_visibility: false,
            policies: 2,
            tags: Some(serde_json::json!({ "team": "network", "env": "test" })),
            client_token: None,
        },
    ]
}

/// RULE: a created deployment is read back, by its ARN and by its bare id, exactly as the
/// create returned it — and it is the listing's own source of truth.
#[tokio::test]
async fn a_created_deployment_is_read_back_by_arn_and_by_id() {
    let emulator = Emulator::new();
    let first = emulator.create_policy("deployment-policy-one").await;
    let second = emulator.create_policy("deployment-policy-two").await;
    let policy_arns = vec![first.string_at("policyArn"), second.string_at("policyArn")];
    let scope_arn = resource_arn("scope", "deploymentscope1");

    let mut created: Vec<(CreateCase, serde_json::Value)> = Vec::new();
    for case in create_cases() {
        let label = case.deployment_name;
        let reply = emulator
            .create_deployment(case.body(&policy_arns, &scope_arn))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {label}: expected 201, got {} {:?}",
            reply.status, reply.json
        );

        let deployment_id = reply.string_at("deploymentId");
        assert!(
            is_deployment_id(&deployment_id),
            "case {label}: deploymentId must match ^[a-z0-9]{{1,64}}$, got {deployment_id:?}"
        );
        assert_eq!(
            reply.string_at("deploymentArn"),
            resource_arn("deployment", &deployment_id),
            "case {label}: unexpected deploymentArn"
        );
        assert_eq!(
            reply.string_at("deploymentName"),
            case.deployment_name,
            "case {label}: unexpected deploymentName"
        );
        assert_eq!(
            reply.string_at("status"),
            case.expected_status(),
            "case {label}: unexpected status"
        );
        let version = reply.string_at("version");
        assert!(
            is_entity_version(&version),
            "case {label}: version must match ^[1-9][0-9]*$, got {version:?}"
        );
        assert_eq!(
            reply.json["associatedPolicyList"].as_array().map(Vec::len),
            Some(case.policies),
            "case {label}: unexpected associatedPolicyList, got {:?}",
            reply.json["associatedPolicyList"]
        );
        assert_eq!(
            reply.json["associatedScopeList"][0]["scopeArn"]
                .as_str()
                .map(str::to_string),
            Some(scope_arn.clone()),
            "case {label}: unexpected associatedScopeList, got {:?}",
            reply.json["associatedScopeList"]
        );

        // The read path: both documented forms of the identifier resolve to the record
        // the create persisted, and the read is repeatable.
        for (form, identifier) in [
            ("its ARN", reply.string_at("deploymentArn")),
            ("its bare id", deployment_id.clone()),
        ] {
            for probe in 0..2 {
                let read = emulator.get_deployment(&identifier).await;
                assert_eq!(
                    read.status, 200,
                    "case {label} read by {form} (probe {probe}): expected 200, got {} {:?}",
                    read.status, read.json
                );
                for member in [
                    "deploymentId",
                    "deploymentArn",
                    "deploymentName",
                    "status",
                    "version",
                ] {
                    assert_eq!(
                        read.json[member], reply.json[member],
                        "case {label} read by {form} (probe {probe}): {member} must match the \
                         create's, got {:?}",
                        read.json[member]
                    );
                }
                assert_eq!(
                    read.json["associatedPolicyList"], reply.json["associatedPolicyList"],
                    "case {label} read by {form} (probe {probe}): unexpected associatedPolicyList"
                );
                assert_eq!(
                    read.json["associatedScopeList"], reply.json["associatedScopeList"],
                    "case {label} read by {form} (probe {probe}): unexpected associatedScopeList"
                );
                assert_eq!(
                    read.json["deploymentDescription"], reply.json["deploymentDescription"],
                    "case {label} read by {form} (probe {probe}): unexpected \
                     deploymentDescription"
                );
                assert_eq!(
                    read.json["deploymentConfiguration"]["enableCrossAccountVisibility"],
                    serde_json::json!(case.cross_account_visibility),
                    "case {label} read by {form} (probe {probe}): unexpected \
                     deploymentConfiguration"
                );
            }
        }
        created.push((case, reply.json.clone()));
    }

    // Every created deployment, and nothing else, is what the listing reports.
    let listed = emulator.list_all_deployments(None, None).await;
    let mut expected: Vec<String> = created
        .iter()
        .map(|(_, json)| json["deploymentId"].as_str().expect("deploymentId").to_string())
        .collect();
    expected.sort();
    let mut actual = listed.clone();
    actual.sort();
    assert_eq!(
        actual, expected,
        "case listing every deployment: the listing must report exactly what was created"
    );

    // The status filter reports the rows that carry that status, and only those.
    for status in ["ACTIVE", "DRAFT"] {
        let mut filtered = emulator.list_all_deployments(None, Some(status)).await;
        filtered.sort();
        let mut wanted: Vec<String> = created
            .iter()
            .filter(|(case, _)| case.expected_status() == status)
            .map(|(_, json)| json["deploymentId"].as_str().expect("deploymentId").to_string())
            .collect();
        wanted.sort();
        assert_eq!(
            filtered, wanted,
            "case listing status={status}: unexpected deployments"
        );
    }

    // One row per page: the token resumes where the previous page stopped rather than
    // repeating or dropping a row.
    let paged = emulator.list_all_deployments(Some(1), None).await;
    assert_eq!(
        paged, listed,
        "case listing one row per page: paging must report the same rows in the same order"
    );
}

/// RULE: an identifier no stored deployment answers to is refused with the modelled
/// `ResourceNotFoundException` — HTTP 404, that error code, and the error class's message
/// naming the resource type and the id the identifier carries — whatever form the
/// identifier takes.
#[tokio::test]
async fn an_unknown_deployment_identifier_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let policy = emulator.create_policy("not-found-probe-policy").await;
    let policy_arn = policy.string_at("policyArn");
    let policy_id = policy.string_at("policyId");
    let scope_arn = resource_arn("scope", "notfoundscope1");

    // One real deployment exists, so a miss is a miss about THIS identifier rather than
    // about an empty store.
    let live = emulator
        .create_deployment(serde_json::json!({
            "deploymentName": "live-deployment",
            "deploymentConfiguration": { "enableCrossAccountVisibility": false },
            "associatedPolicyList": [{ "policyIdentifier": policy_arn }],
            "associatedScopeList": [{ "scopeIdentifier": scope_arn }],
        }))
        .await;
    assert_eq!(
        live.status, 201,
        "case the live deployment: expected 201, got {} {:?}",
        live.status, live.json
    );
    let live_id = live.string_at("deploymentId");

    // The rows differ along the FORM of the identifier: a well-formed deployment ARN
    // nothing minted, a bare id nothing minted, an ARN of another resource type, an
    // existing resource's id and ARN from another entity's store, the one-character
    // minimum of `DeploymentIdentifier`'s `@length(1..1010)`, and an identifier that
    // differs from a live one by a single character.
    let rows: Vec<(&str, String)> = vec![
        (
            "a deployment ARN that was never minted",
            resource_arn("deployment", "neverminteddeployment1"),
        ),
        ("a bare id that was never minted", "nosuchdeployment1".to_string()),
        ("an ARN of another resource type", resource_arn("scope", "notadeployment1")),
        ("an existing policy's id", policy_id.clone()),
        ("an existing policy's ARN", policy_arn.clone()),
        ("the shortest identifier the model admits", "x".to_string()),
        (
            "a live deployment's id with one character changed",
            format!("{}z", &live_id[..live_id.len() - 1]),
        ),
    ];

    for (label, identifier) in rows {
        let reply = emulator.get_deployment(&identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected 404, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.json["message"].as_str(),
            Some(
                format!(
                    "{NOT_FOUND_RESOURCE_TYPE} not found: {}",
                    entity_id_in(&identifier)
                )
                .as_str()
            ),
            "case {label:?}: unexpected message, got {:?}",
            reply.json["message"]
        );
        assert_eq!(
            reply.json["resourceId"].as_str(),
            Some(entity_id_in(&identifier)),
            "case {label:?}: resourceId must name the deployment the identifier carried, got {:?}",
            reply.json["resourceId"]
        );
        assert_eq!(
            reply.json["resourceType"].as_str(),
            Some("Deployment"),
            "case {label:?}: unexpected resourceType, got {:?}",
            reply.json["resourceType"]
        );
    }

    // A refused read left the store alone: the one live deployment is still the only one
    // listed, and it still reads back.
    assert_eq!(
        emulator.list_all_deployments(None, None).await,
        vec![live_id.clone()],
        "case after every refused read: the listing must still report only the live deployment"
    );
    assert_eq!(
        emulator.get_deployment(&live_id).await.status,
        200,
        "case after every refused read: the live deployment must still read back"
    );
}

/// RULE: `clientToken` is `@idempotencyToken` — a retry with the same token answers with
/// the first call's deployment instead of minting a second one.
#[tokio::test]
async fn a_replayed_client_token_mints_one_deployment() {
    let emulator = Emulator::new();
    let policy_arn = emulator
        .create_policy("idempotent-deployment-policy")
        .await
        .string_at("policyArn");
    let body = serde_json::json!({
        "clientToken": "replayed-deployment-token",
        "deploymentName": "replayed-deployment",
        "deploymentConfiguration": { "enableCrossAccountVisibility": false },
        "associatedPolicyList": [{ "policyIdentifier": policy_arn }],
        "associatedScopeList": [{ "scopeIdentifier": resource_arn("scope", "idempotentscope1") }],
    });

    let first = emulator.create_deployment(body.clone()).await;
    assert_eq!(
        first.status, 201,
        "case the first create: expected 201, got {} {:?}",
        first.status, first.json
    );
    let replay = emulator.create_deployment(body).await;
    assert_eq!(
        replay.status, 201,
        "case the replay: expected 201, got {} {:?}",
        replay.status, replay.json
    );
    assert_eq!(
        replay.json, first.json,
        "case the replay: it must answer with the first call's deployment"
    );
    assert_eq!(
        emulator.list_all_deployments(None, None).await.len(),
        1,
        "case the replay: exactly one deployment must exist"
    );
}

/// RULE: a reference member documented as a resource's ARN is refused when it is not one
/// — the association it would become is modelled `@length(20..1010)` with the ARN
/// `@pattern`, so a record nothing could answer with is never stored.
#[tokio::test]
async fn a_reference_that_is_not_an_arn_is_refused() {
    let emulator = Emulator::new();
    let policy_arn = emulator
        .create_policy("reference-check-policy")
        .await
        .string_at("policyArn");
    let scope_arn = resource_arn("scope", "referencescope1");

    // The rows differ along which list carries the bad reference and how it is wrong: a
    // bare id, an ARN too short for the modelled minimum, and a value that is not an ARN
    // at all.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "a policy reference that is a bare id",
            serde_json::json!({
                "deploymentName": "bad-policy-reference",
                "deploymentConfiguration": { "enableCrossAccountVisibility": false },
                "associatedPolicyList": [{ "policyIdentifier": "abc123" }],
                "associatedScopeList": [{ "scopeIdentifier": scope_arn }],
            }),
        ),
        (
            "a scope reference that is not an ARN at all",
            serde_json::json!({
                "deploymentName": "bad-scope-reference",
                "deploymentConfiguration": { "enableCrossAccountVisibility": false },
                "associatedPolicyList": [{ "policyIdentifier": policy_arn }],
                "associatedScopeList": [{ "scopeIdentifier": "not an arn" }],
            }),
        ),
        (
            "a scope reference shorter than the modelled ARN minimum",
            serde_json::json!({
                "deploymentName": "short-scope-reference",
                "deploymentConfiguration": { "enableCrossAccountVisibility": false },
                "associatedPolicyList": [{ "policyIdentifier": policy_arn }],
                "associatedScopeList": [{ "scopeIdentifier": "arn:aws:a:b:c:d" }],
            }),
        ),
    ];

    for (label, body) in rows {
        let reply = emulator.create_deployment(body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {label:?}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.json["reason"].as_str(),
            Some("FIELD_VALIDATION_FAILED"),
            "case {label:?}: unexpected reason, got {:?}",
            reply.json["reason"]
        );
    }

    assert!(
        emulator.list_all_deployments(None, None).await.is_empty(),
        "case after every refused create: no deployment may have been stored"
    );
}
