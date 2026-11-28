// SPDX-License-Identifier: CC0-1.0

//! Removing a scope, and the read that must no longer resolve it.
//!
//! RULE under test: a scope this service has removed is gone from the entity store, so
//! every later read of it misses — `GetScope` answers the modelled HTTP 404
//! `ResourceNotFoundException` with the `x-amzn-errortype` header naming that shape and
//! `message` "Scope not found: <the scope id the identifier names>", and `ListScopes` no
//! longer reports the record. `DeleteScope` is `@idempotent` and models no
//! `ResourceNotFoundException`, so an identifier naming no scope is a 204 with nothing to
//! remove; while another resource still associates the scope the delete is refused with
//! the modelled `ConflictException` and the scope stays readable.
//!
//! The rows differ along what the rules generalise over: which form of `scopeIdentifier`
//! the delete and the read name the scope by (its ARN or its bare `scopeId`), which
//! configuration the removed scope carried, whether the scope was published, whether it
//! ever existed, and whether a deployment still references it.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// What a caller sees: the HTTP facts it branches on plus the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    json: serde_json::Value,
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

    async fn create_scope(&self, body: serde_json::Value) -> Reply {
        self.call("POST", "/scopes", Some(body)).await
    }

    async fn get_scope(&self, identifier: &str) -> Reply {
        self.call("GET", &format!("/scopes/{}", path_label(identifier)), None)
            .await
    }

    async fn delete_scope(&self, identifier: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/scopes/{}", path_label(identifier)),
            None,
        )
        .await
    }

    /// Every stored `scopeId`, so a removal can be shown to have dropped the record.
    async fn listed_scope_ids(&self) -> Vec<String> {
        let reply = self.call("GET", "/scopes", None).await;
        assert_eq!(
            reply.status, 200,
            "ListScopes expected 200, got {} {:?}",
            reply.status, reply.json
        );
        reply.json["scopes"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["scopeId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// An identifier in a `@httpLabel` position: a scope ARN carries `:` and `/`, which the
/// label must percent-encode to reach the handler whole.
fn path_label(identifier: &str) -> String {
    identifier
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// An ARN of one of this service's resource types, built from the same helper the handlers
/// mint ARNs with rather than from a hardcoded literal.
fn resource_arn(resource_type: &str, id: &str) -> String {
    engine::emulator_arn(
        "network-security-manager",
        "us-east-1",
        &format!("{resource_type}/{id}"),
    )
}

/// A `CreateScope` body selecting `resource_scopes`.
fn scope_body(
    name: &str,
    published: bool,
    resource_scopes: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "scopeName": name,
        "isPublished": published,
        "scopeConfiguration": { "resourceScopes": resource_scopes },
    })
}

/// Assert `reply` is the miss the rule states for `scope_id`.
fn assert_scope_miss(label: &str, reply: &Reply, scope_id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {label}: expected 404, got {} {:?}",
        reply.status, reply.json
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case {label}: unexpected x-amzn-errortype, got {:?}",
        reply.error_code
    );
    assert_eq!(
        reply.json["message"].as_str(),
        Some(format!("Scope not found: {scope_id}").as_str()),
        "case {label}: unexpected message, got {:?}",
        reply.json["message"]
    );
    assert_eq!(
        reply.json["resourceId"].as_str(),
        Some(scope_id),
        "case {label}: the miss names the scope it resolved, got {:?}",
        reply.json["resourceId"]
    );
    assert_eq!(
        reply.json["resourceType"].as_str(),
        Some("Scope"),
        "case {label}: unexpected resourceType, got {:?}",
        reply.json["resourceType"]
    );
    // A miss carries the error members and nothing of the scope body.
    assert!(
        reply.json["scopeArn"].is_null() && reply.json["scopeConfiguration"].is_null(),
        "case {label}: a miss answers no scope body, got {:?}",
        reply.json
    );
}

/// RULE: a removed scope no longer resolves — the read that follows is the modelled 404
/// stating "Scope not found: <scopeId>", whichever identifier form names it, and the
/// listing no longer reports the record.
#[tokio::test]
async fn a_removed_scope_no_longer_resolves() {
    /// One row: the scope to create, which identifier form the delete names it by and
    /// which the read that follows uses.
    struct Row {
        label: &'static str,
        name: &'static str,
        published: bool,
        resource_scopes: serde_json::Value,
        /// Whether the DELETE names the scope by its ARN rather than its `scopeId`.
        delete_by_arn: bool,
        /// Whether the GET that follows names it by its ARN rather than its `scopeId`.
        read_by_arn: bool,
    }

    // The resource types come from `ScopeResourceType`
    // (`smithy_query(query="shape", shape="ScopeResourceType")`).
    let rows = vec![
        Row {
            label: "deleted by ARN, read back by ARN, the recorded case",
            name: "removed-by-arn",
            published: true,
            resource_scopes: serde_json::json!({ "AWS::EC2::EIP": { "includeAll": true } }),
            delete_by_arn: true,
            read_by_arn: true,
        },
        Row {
            label: "deleted by ARN, read back by scopeId",
            name: "removed-read-by-id",
            published: true,
            resource_scopes: serde_json::json!({
                "AWS::CloudFront::Distribution": {
                    "include": {
                        "explicitArns": [
                            "arn:aws:cloudfront::000000000000:distribution/E1EXAMPLE",
                        ],
                    },
                },
            }),
            delete_by_arn: true,
            read_by_arn: false,
        },
        Row {
            label: "a draft scope deleted by scopeId, read back by ARN",
            name: "removed-draft",
            published: false,
            resource_scopes: serde_json::json!({
                "AWS::ApiGateway::Stage": { "includeAll": true },
            }),
            delete_by_arn: false,
            read_by_arn: true,
        },
        Row {
            label: "deleted by scopeId, read back by scopeId",
            name: "removed-by-id",
            published: true,
            resource_scopes: serde_json::json!({
                "AWS::ElasticLoadBalancing::LoadBalancer": { "includeAll": true },
            }),
            delete_by_arn: false,
            read_by_arn: false,
        },
    ];

    for row in rows {
        // A fresh emulator per row, so each row removes the only scope stored.
        let emulator = Emulator::new();
        let created = emulator
            .create_scope(scope_body(row.name, row.published, row.resource_scopes))
            .await;
        assert_eq!(
            created.status, 201,
            "case {}: CreateScope expected 201, got {} {:?}",
            row.label, created.status, created.json
        );
        let scope_id = created.json["scopeId"]
            .as_str()
            .expect("the create answers a scopeId")
            .to_string();
        let scope_arn = created.json["scopeArn"]
            .as_str()
            .expect("the create answers a scopeArn")
            .to_string();

        // Before the removal the read resolves.
        let read = emulator.get_scope(&scope_arn).await;
        assert_eq!(
            read.status, 200,
            "case {}: the read before the removal expected 200, got {} {:?}",
            row.label, read.status, read.json
        );
        assert_eq!(
            emulator.listed_scope_ids().await,
            vec![scope_id.clone()],
            "case {}: the created scope must be listed",
            row.label
        );

        let deleted = emulator
            .delete_scope(if row.delete_by_arn { &scope_arn } else { &scope_id })
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {}: DeleteScope expected 204, got {} {:?}",
            row.label, deleted.status, deleted.json
        );
        assert_eq!(
            deleted.json,
            serde_json::Value::Null,
            "case {}: a 204 carries an empty body, got {:?}",
            row.label, deleted.json
        );

        let missed = emulator
            .get_scope(if row.read_by_arn { &scope_arn } else { &scope_id })
            .await;
        assert_scope_miss(row.label, &missed, &scope_id);

        let listed = emulator.listed_scope_ids().await;
        assert!(
            listed.is_empty(),
            "case {}: a removed scope is no longer listed, got {listed:?}",
            row.label
        );
    }
}

/// RULE: `DeleteScope` is `@idempotent` and models no `ResourceNotFoundException`, so an
/// identifier naming no scope is a 204 with nothing to remove — and the read of such an
/// identifier states the same miss as the read of a removed one.
#[tokio::test]
async fn deleting_a_scope_that_is_not_stored_is_a_success() {
    let emulator = Emulator::new();
    let created = emulator
        .create_scope(scope_body(
            "survivor",
            true,
            serde_json::json!({ "AWS::EC2::EIP": { "includeAll": true } }),
        ))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateScope expected 201, got {} {:?}",
        created.status, created.json
    );
    let live_id = created.json["scopeId"]
        .as_str()
        .expect("the create answers a scopeId")
        .to_string();
    let live_arn = created.json["scopeArn"]
        .as_str()
        .expect("the create answers a scopeArn")
        .to_string();

    // The rows differ along the form of an identifier no scope answers to: a bare id, an
    // id one character off a live one, an ARN of this service's own shape, and an ARN in
    // another region.
    let unknown: Vec<(&str, String)> = vec![
        ("a bare id nothing minted", "abc123".to_string()),
        (
            "a live id with its last character changed",
            format!("{}x", &live_id[..live_id.len() - 1]),
        ),
        ("an ARN nothing minted", resource_arn("scope", "deadbeef")),
        (
            "an ARN in another region",
            engine::emulator_arn("network-security-manager", "us-west-2", "scope/abc123"),
        ),
    ];

    for (label, identifier) in &unknown {
        let deleted = emulator.delete_scope(identifier).await;
        assert_eq!(
            deleted.status, 204,
            "case {label}: an idempotent delete expected 204, got {} {:?}",
            deleted.status, deleted.json
        );
        let missed = emulator.get_scope(identifier).await;
        // The id the miss names is the one the identifier carries: the resource segment of
        // an ARN, or the value itself when it is a bare id.
        let expected_id = identifier.rsplit('/').next().unwrap_or(identifier);
        assert_scope_miss(label, &missed, expected_id);
    }

    // The scope that was there is still there: a delete of another identifier removed
    // nothing.
    let read = emulator.get_scope(&live_arn).await;
    assert_eq!(
        read.status, 200,
        "the stored scope must still read back, got {} {:?}",
        read.status, read.json
    );
    assert_eq!(
        emulator.listed_scope_ids().await,
        vec![live_id],
        "the stored scope must still be listed"
    );

    // And deleting the same scope twice is still a success.
    for attempt in 1..=2 {
        let deleted = emulator.delete_scope(&live_arn).await;
        assert_eq!(
            deleted.status, 204,
            "attempt {attempt}: DeleteScope expected 204, got {} {:?}",
            deleted.status, deleted.json
        );
    }
}

/// RULE: while another resource still associates the scope the removal is refused with the
/// modelled `ConflictException`, and the scope stays readable.
#[tokio::test]
async fn a_scope_a_deployment_associates_cannot_be_removed() {
    let emulator = Emulator::new();
    let scope = emulator
        .create_scope(scope_body(
            "referenced-scope",
            true,
            serde_json::json!({ "AWS::EC2::EIP": { "includeAll": true } }),
        ))
        .await;
    assert_eq!(
        scope.status, 201,
        "CreateScope expected 201, got {} {:?}",
        scope.status, scope.json
    );
    let scope_id = scope.json["scopeId"]
        .as_str()
        .expect("the create answers a scopeId")
        .to_string();
    let scope_arn = scope.json["scopeArn"]
        .as_str()
        .expect("the create answers a scopeArn")
        .to_string();

    let policy = emulator
        .call(
            "POST",
            "/policies",
            Some(serde_json::json!({
                "policyName": "referenced-scope-policy",
                "priority": 1,
                "firewallType": "WAF",
                "policyConfiguration": {
                    "remediationEnabled": true,
                    "resourcesCleanUp": true,
                    "wafConfig": {
                        "existingCustomerWebACLResolution": "RETROFIT",
                        "conflictResolution": "MERGE_WHERE_APPLICABLE",
                    },
                },
                // "For AWS WAF policies, specify 1 to 100 templates or rules."
                "associatedTemplateAndRuleList": [{
                    "ruleIdentifier": resource_arn("rule", "scoperule"),
                }],
            })),
        )
        .await;
    assert_eq!(
        policy.status, 201,
        "CreatePolicy expected 201, got {} {:?}",
        policy.status, policy.json
    );
    let policy_arn = policy.json["policyArn"]
        .as_str()
        .expect("the create answers a policyArn")
        .to_string();

    let deployment = emulator
        .call(
            "POST",
            "/deployments",
            Some(serde_json::json!({
                "deploymentName": "referencing-deployment",
                "isPublished": true,
                "deploymentConfiguration": { "enableCrossAccountVisibility": false },
                "associatedPolicyList": [{ "policyIdentifier": policy_arn }],
                "associatedScopeList": [{ "scopeIdentifier": scope_arn }],
            })),
        )
        .await;
    assert_eq!(
        deployment.status, 201,
        "CreateDeployment expected 201, got {} {:?}",
        deployment.status, deployment.json
    );
    let deployment_id = deployment.json["deploymentId"]
        .as_str()
        .expect("the create answers a deploymentId");
    assert!(
        !deployment_id.is_empty(),
        "the create answers a deploymentId, got {:?}",
        deployment.json["deploymentId"]
    );

    // Both identifier forms are refused alike while the reference is held.
    for (label, identifier) in [
        ("by ARN", scope_arn.clone()),
        ("by scopeId", scope_id.clone()),
    ] {
        let refused = emulator.delete_scope(&identifier).await;
        assert_eq!(
            refused.status, 409,
            "case {label}: expected 409, got {} {:?}",
            refused.status, refused.json
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {label}: unexpected x-amzn-errortype, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.json["resourceId"].as_str(),
            Some(scope_id.as_str()),
            "case {label}: the refusal names the scope, got {:?}",
            refused.json["resourceId"]
        );
        assert_eq!(
            refused.json["resourceType"].as_str(),
            Some("Scope"),
            "case {label}: unexpected resourceType, got {:?}",
            refused.json["resourceType"]
        );
        let read = emulator.get_scope(&scope_arn).await;
        assert_eq!(
            read.status, 200,
            "case {label}: a refused delete leaves the scope readable, got {} {:?}",
            read.status, read.json
        );
    }

    // A second scope, which no deployment names, is removable while the referenced one is
    // not: the refusal is the reference, not the store.
    let other = emulator
        .create_scope(scope_body(
            "unreferenced-scope",
            true,
            serde_json::json!({ "AWS::ApiGateway::Stage": { "includeAll": true } }),
        ))
        .await;
    assert_eq!(
        other.status, 201,
        "CreateScope expected 201, got {} {:?}",
        other.status, other.json
    );
    let other_id = other.json["scopeId"]
        .as_str()
        .expect("the create answers a scopeId")
        .to_string();
    let other_arn = other.json["scopeArn"]
        .as_str()
        .expect("the create answers a scopeArn")
        .to_string();
    let deleted = emulator.delete_scope(&other_arn).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteScope of an unreferenced scope expected 204, got {} {:?}",
        deleted.status, deleted.json
    );
    assert_scope_miss("the unreferenced scope", &emulator.get_scope(&other_arn).await, &other_id);
    assert_eq!(
        emulator.listed_scope_ids().await,
        vec![scope_id],
        "only the referenced scope remains stored"
    );
}
