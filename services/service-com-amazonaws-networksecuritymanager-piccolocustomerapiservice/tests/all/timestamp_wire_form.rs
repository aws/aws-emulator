// SPDX-License-Identifier: CC0-1.0

//! The wire form this service carries a timestamp in.
//!
//! RULE under test: every timestamp member of this model targets `DateTimestamp`, which
//! declares `@timestampFormat("date-time")`
//! (`smithy_query(query="shape", shape="DateTimestamp")`), so each one is carried as an
//! RFC 3339 date-time STRING — never as the epoch-seconds number the restJson1 protocol
//! defaults to for a member whose schema reports no format. Every generated SDK reads such
//! a member with a date-time parser, so a bare number is a value no client of the modelled
//! API can read.
//!
//! The rule belongs to the SHAPE, so it holds for every member targeting it: the rows below
//! span each entity this emulator mints (policy, rule, template, scope and the deployment
//! that references them), the reads that answer the same record back, and the list whose
//! summary carries its own copy of the member.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// What a caller sees: the HTTP facts it branches on plus the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    json: serde_json::Value,
}

impl Reply {
    fn string_at(&self, member: &str) -> String {
        self.json[member]
            .as_str()
            .unwrap_or_else(|| panic!("expected {member} in {:?}", self.json))
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
        let raw = response.into_body();
        let json = if raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null)
        };
        Reply { status, json }
    }
}

/// Whether `value` is an RFC 3339 date-time as the `date-time` form writes one:
/// `YYYY-MM-DDThh:mm:ss[.sss]Z`, the shape every AWS SDK's date-time parser accepts.
fn is_date_time(value: &serde_json::Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    let bytes = text.as_bytes();
    bytes.len() >= 20
        && text.ends_with('Z')
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[..4].iter().all(u8::is_ascii_digit)
}

/// Assert `reply`'s `member` is carried as a date-time string, and so is not the
/// epoch-seconds number the protocol would default to.
fn assert_date_time(case: &str, member: &str, reply: &Reply) {
    let value = &reply.json[member];
    assert!(
        is_date_time(value),
        "case {case}: expected {member} as an RFC 3339 date-time string, got {value:?}"
    );
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

/// RULE: every `DateTimestamp` member this service answers is a date-time string — on each
/// entity's create, on the reads that answer the same record, and in a list's summary.
#[tokio::test]
async fn every_timestamp_member_is_carried_as_a_date_time_string() {
    let emulator = Emulator::new();

    let policy = emulator
        .call(
            "POST",
            "/policies",
            Some(serde_json::json!({
                "policyName": "timestamp-policy",
                "priority": 1,
                "firewallType": "WAF",
                // "For AWS WAF policies, specify 1 to 100 templates or rules."
                "associatedTemplateAndRuleList": [{
                    "ruleIdentifier": engine::emulator_arn(
                        "network-security-manager",
                        "us-east-1",
                        "rule/timestamprule",
                    ),
                }],
                "policyConfiguration": {
                    "remediationEnabled": true,
                    "resourcesCleanUp": true,
                    "wafConfig": {
                        "existingCustomerWebACLResolution": "RETROFIT",
                        "conflictResolution": "MERGE_WHERE_APPLICABLE",
                    },
                },
            })),
        )
        .await;
    assert_eq!(
        policy.status, 201,
        "CreatePolicy expected 201, got {} {:?}",
        policy.status, policy.json
    );
    assert_date_time("CreatePolicy", "updatedAt", &policy);

    let rule = emulator
        .call(
            "POST",
            "/rules",
            Some(serde_json::json!({
                "ruleName": "timestamp-rule",
                "firewallType": "WAF",
                "ruleType": "CONFIGURATION",
                "configuration": { "DefaultAction": "ALLOW" },
            })),
        )
        .await;
    assert_eq!(
        rule.status, 201,
        "CreateRule expected 201, got {} {:?}",
        rule.status, rule.json
    );
    assert_date_time("CreateRule", "updatedAt", &rule);

    let template = emulator
        .call(
            "POST",
            "/templates",
            Some(serde_json::json!({
                "templateName": "timestamp-template",
                "firewallType": "WAF",
                "associatedRuleList": [{ "ruleIdentifier": rule.string_at("ruleArn") }],
            })),
        )
        .await;
    assert_eq!(
        template.status, 201,
        "CreateTemplate expected 201, got {} {:?}",
        template.status, template.json
    );
    assert_date_time("CreateTemplate", "updatedAt", &template);

    let scope = emulator
        .call(
            "POST",
            "/scopes",
            Some(serde_json::json!({
                "scopeName": "timestamp-scope",
                "scopeConfiguration": {
                    "resourceScopes": { "AWS::EC2::EIP": { "includeAll": true } },
                },
            })),
        )
        .await;
    assert_eq!(
        scope.status, 201,
        "CreateScope expected 201, got {} {:?}",
        scope.status, scope.json
    );
    assert_date_time("CreateScope", "updatedAt", &scope);

    let deployment = emulator
        .call(
            "POST",
            "/deployments",
            Some(serde_json::json!({
                "deploymentName": "timestamp-deployment",
                "deploymentConfiguration": { "enableCrossAccountVisibility": false },
                "associatedPolicyList": [
                    { "policyIdentifier": policy.string_at("policyArn") },
                ],
                "associatedScopeList": [
                    { "scopeIdentifier": scope.string_at("scopeArn") },
                ],
            })),
        )
        .await;
    assert_eq!(
        deployment.status, 201,
        "CreateDeployment expected 201, got {} {:?}",
        deployment.status, deployment.json
    );
    assert_date_time("CreateDeployment", "updatedAt", &deployment);

    // The reads answer the stored record, so they carry the member in the same form.
    let reads = [
        (
            "GetPolicy",
            format!("/policies/{}", policy.string_at("policyId")),
        ),
        // `ruleIdentifier` is documented to be the rule's ARN, and only that form resolves
        // (the identifier-form refusal is pinned by `rule_identifier_form.rs`), so the
        // rule read is addressed by the ARN the create answered with.
        (
            "GetRule",
            format!(
                "/rules/{}",
                rule.string_at("ruleArn").replace('/', "%2F")
            ),
        ),
        (
            "GetTemplate",
            format!("/templates/{}", template.string_at("templateId")),
        ),
        (
            "GetScope",
            format!("/scopes/{}", scope.string_at("scopeId")),
        ),
        (
            "GetDeployment",
            format!("/deployments/{}", deployment.string_at("deploymentId")),
        ),
    ];
    for (case, path) in reads {
        let read = emulator.call("GET", &path, None).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200, got {} {:?}",
            read.status, read.json
        );
        assert_date_time(case, "updatedAt", &read);
    }

    // A summary carries its own copy of the member, so the list states it too.
    for (case, path, member) in [
        ("ListPolicies", "/policies", "policies"),
        ("ListTemplates", "/templates", "templates"),
        ("ListDeployments", "/deployments", "deployments"),
    ] {
        let listed = emulator.call("GET", path, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200, got {} {:?}",
            listed.status, listed.json
        );
        let entries = listed.json[member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected {member} in {:?}", listed.json));
        assert!(
            !entries.is_empty(),
            "case {case}: the listing must report what was created, got {:?}",
            listed.json[member]
        );
        for entry in entries {
            assert!(
                is_date_time(&entry["updatedAt"]),
                "case {case}: expected a summary updatedAt as an RFC 3339 date-time string, \
                 got {:?}",
                entry["updatedAt"]
            );
        }
    }

    // The ARN helper is what the handlers mint with, so a read by ARN resolves too — and
    // its answer carries the same member form.
    let by_arn = emulator
        .call(
            "GET",
            &format!(
                "/rules/{}",
                resource_arn("rule", &rule.string_at("ruleId")).replace(':', "%3A").replace('/', "%2F")
            ),
            None,
        )
        .await;
    assert_eq!(
        by_arn.status, 200,
        "case GetRule by ARN: expected 200, got {} {:?}",
        by_arn.status, by_arn.json
    );
    assert_date_time("GetRule by ARN", "updatedAt", &by_arn);
}
