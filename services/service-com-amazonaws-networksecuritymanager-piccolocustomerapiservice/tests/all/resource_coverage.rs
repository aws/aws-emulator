// SPDX-License-Identifier: CC0-1.0

//! The coverage listings: `ListEntityCoverages` and `ListProtectedResourceCoverages`.
//!
//! RULE under test: a protected resource is covered by a policy when a PUBLISHED deployment
//! applies that published policy to a scope whose resource selection names the resource
//! explicitly, and the two listings are two questions about that one derivation — the first
//! asks which resources one entity covers, the second which entities cover one resource, so
//! each row they report carries the same facts: the covering policy (`entityType` `Policy`,
//! `entityArn`), the resource and the `ScopeResourceType` the scope selected it under, the
//! policy's `firewallType`, the ARNs of every deployment through which the policy reaches
//! the resource, the account the resource's own ARN names, and a `coverageStatus` of
//! `IN_EFFECT` for a firewall type that combines policies (AWS WAF, AWS Shield Advanced) or,
//! for one that applies a single policy, `IN_EFFECT` on the highest-priority policy — the
//! lowest `priority` — and `SUPERSEDED` on the others.
//!
//! The rows differ along what the rule generalises over: which `ScopeResourceType` selects
//! the resource, whether the resource's ARN names an owning account, how many deployments
//! connect a policy to a resource, which firewall type the policy enforces, how many
//! policies contest a resource, whether the deployment and the policy are published, how a
//! scope's resource set selects (explicit ARNs, every resource of a type, an exclusion), and
//! the page size the listing is walked with.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a client sees.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An application load balancer owned by one account: the `resourceOwnerAccountId` an ARN
/// with a 12-digit account segment reports.
const ALB_A: &str = "arn:aws:elasticloadbalancing:us-east-1:111122223333:loadbalancer/app/web/0123456789abcdef";

/// A second load balancer, owned by ANOTHER account, so the owner filter has two values to
/// separate.
const ALB_B: &str = "arn:aws:elasticloadbalancing:us-east-1:444455556666:loadbalancer/app/edge/fedcba9876543210";

/// An API Gateway stage whose ARN carries NO account segment, so the row it produces
/// reports no `resourceOwnerAccountId` at all.
const STAGE: &str = "arn:aws:apigateway:us-east-1::/restapis/a1b2c3d4e5/stages/prod";

/// A resource no scope of these fixtures names, for the rows that assert nothing covers it.
const UNCOVERED: &str = "arn:aws:cloudfront::000000000000:distribution/E1UNCOVERED";

/// `ScopeResourceType` for an application load balancer
/// (`smithy_query(query="shape",shape="ScopeResourceType")`).
const TYPE_ALB: &str = "AWS::ElasticLoadBalancingV2::LoadBalancer::application";

/// `ScopeResourceType` for an API Gateway stage, from the same value set.
const TYPE_STAGE: &str = "AWS::ApiGateway::Stage";

/// `ScopeResourceType` for an Elastic IP, from the same value set: the type the fixtures
/// select with `includeAll`.
const TYPE_EIP: &str = "AWS::EC2::EIP";

/// What a caller sees: the HTTP facts it branches on plus the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
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

    /// Create one entity, asserting the create itself succeeded, and answer its reply.
    async fn create(&self, collection: &str, label: &str, body: serde_json::Value) -> Reply {
        let reply = self
            .call("POST", &format!("/{collection}"), Some(body))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {label}: creating in {collection} expected 201, got {} {:?}",
            reply.status, reply.json
        );
        reply
    }

    /// One page of a coverage listing, as the caller asked for it.
    async fn coverages(&self, path: &str, query: &[(&str, &str)]) -> Reply {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", escape(value)))
            .collect();
        self.call("GET", &format!("{path}?{}", rendered.join("&")), None)
            .await
    }

    /// Walk a coverage listing to its end with `max_results` per page, returning every row
    /// it reported in page order.
    async fn all_coverages(
        &self,
        path: &str,
        query: &[(&str, &str)],
        max_results: Option<u32>,
    ) -> Vec<serde_json::Value> {
        let mut collected: Vec<serde_json::Value> = Vec::new();
        let mut next: Option<String> = None;
        // Generous: every page carries at least one row, so a terminating walk is far
        // inside this bound and a listing that never terminates fails instead of hanging.
        for _ in 0..512 {
            let mut page_query: Vec<(&str, String)> = query
                .iter()
                .map(|(name, value)| (*name, (*value).to_string()))
                .collect();
            if let Some(limit) = max_results {
                page_query.push(("maxResults", limit.to_string()));
            }
            if let Some(token) = &next {
                page_query.push(("nextToken", token.clone()));
            }
            let borrowed: Vec<(&str, &str)> = page_query
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            let reply = self.coverages(path, &borrowed).await;
            assert_eq!(
                reply.status, 200,
                "case walking {path} with maxResults={max_results:?}: expected 200, got {} {:?}",
                reply.status, reply.json
            );
            let page = reply.json["resourceCoverages"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case walking {path}: resourceCoverages is required, got {:?}",
                        reply.json
                    )
                });
            collected.extend(page.iter().cloned());
            match reply.json["nextToken"].as_str() {
                Some(token) => next = Some(token.to_string()),
                None => return collected,
            }
        }
        panic!("case walking {path}: the walk never terminated, collected {collected:?}");
    }
}

/// `value` as one query-parameter value: percent-encoded by the maintained encoder, so an
/// ARN's colons and slashes reach the service as sent.
fn escape(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

/// A `CreatePolicy` body: a policy of `firewall_type` at `priority`, published or not.
fn policy_body(
    name: &str,
    firewall_type: &str,
    priority: u32,
    published: bool,
) -> serde_json::Value {
    let mut configuration = serde_json::json!({
        "remediationEnabled": true,
        "resourcesCleanUp": true,
    });
    // The two members `WafConfig` declares `@required` are owed by a WAF policy.
    if firewall_type == "WAF" {
        configuration["wafConfig"] = serde_json::json!({
            "existingCustomerWebACLResolution": "RETROFIT",
            "conflictResolution": "MERGE_WHERE_APPLICABLE",
        });
    }
    let mut body = serde_json::json!({
        "policyName": name,
        "priority": priority,
        "firewallType": firewall_type,
        "isPublished": published,
        "policyConfiguration": configuration,
    });
    // "For AWS WAF policies, specify 1 to 100 templates or rules. For AWS Shield Advanced
    // policies, this list must be empty."
    if firewall_type == "WAF" {
        body["associatedTemplateAndRuleList"] = serde_json::json!([{
            "ruleIdentifier": engine::emulator_arn(
                "network-security-manager",
                "us-east-1",
                "rule/coveragerule",
            ),
        }]);
    }
    body
}

/// A `CreateScope` body selecting exactly `resource_scopes`.
fn scope_body(name: &str, published: bool, resource_scopes: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "scopeName": name,
        "isPublished": published,
        "scopeConfiguration": { "resourceScopes": resource_scopes },
    })
}

/// A `CreateDeployment` body applying `policy_arns` to `scope_arn`.
fn deployment_body(
    name: &str,
    published: bool,
    policy_arns: &[&str],
    scope_arn: &str,
) -> serde_json::Value {
    serde_json::json!({
        "deploymentName": name,
        "isPublished": published,
        "deploymentConfiguration": { "enableCrossAccountVisibility": false },
        "associatedPolicyList": policy_arns
            .iter()
            .map(|arn| serde_json::json!({ "policyIdentifier": arn }))
            .collect::<Vec<_>>(),
        "associatedScopeList": [{ "scopeIdentifier": scope_arn }],
    })
}

/// A resource scope selecting the given ARNs explicitly.
fn include_arns(arns: &[&str]) -> serde_json::Value {
    serde_json::json!({ "include": { "explicitArns": arns } })
}

/// The one row of `rows` that names `resource_arn` and `entity_arn`.
fn row_for<'a>(
    rows: &'a [serde_json::Value],
    label: &str,
    resource_arn: &str,
    entity_arn: &str,
) -> &'a serde_json::Value {
    let matching: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|row| {
            row["resourceArn"].as_str() == Some(resource_arn)
                && row["entityArn"].as_str() == Some(entity_arn)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {label}: expected exactly one row for {resource_arn} / {entity_arn}, got \
         {matching:?} among {rows:?}"
    );
    matching[0]
}

/// The fixture: one published scope selecting three resources, two published AWS WAF
/// policies at different priorities, and the deployments that apply them.
struct Fixture {
    emulator: Emulator,
    waf_high: String,
    waf_low: String,
    first_deployment: String,
    second_deployment: String,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::new();
        // One scope selecting two load balancers and a stage explicitly, plus every
        // Elastic IP — a selection that names no resource this service holds.
        let scope = emulator
            .create(
                "scopes",
                "the covered scope",
                scope_body(
                    "covered-scope",
                    true,
                    serde_json::json!({
                        TYPE_ALB: include_arns(&[ALB_A, ALB_B]),
                        TYPE_STAGE: include_arns(&[STAGE]),
                        TYPE_EIP: { "includeAll": true },
                    }),
                ),
            )
            .await
            .string_at("scopeArn");
        // A second scope naming only the first load balancer, so one policy reaches that
        // resource through two deployments.
        let narrow_scope = emulator
            .create(
                "scopes",
                "the narrow scope",
                scope_body(
                    "narrow-scope",
                    true,
                    serde_json::json!({ TYPE_ALB: include_arns(&[ALB_A]) }),
                ),
            )
            .await
            .string_at("scopeArn");

        let waf_high = emulator
            .create(
                "policies",
                "the higher-priority WAF policy",
                policy_body("waf-high", "WAF", 1, true),
            )
            .await
            .string_at("policyArn");
        let waf_low = emulator
            .create(
                "policies",
                "the lower-priority WAF policy",
                policy_body("waf-low", "WAF", 20, true),
            )
            .await
            .string_at("policyArn");

        let first_deployment = emulator
            .create(
                "deployments",
                "the deployment of both WAF policies",
                deployment_body(
                    "both-waf",
                    true,
                    &[waf_high.as_str(), waf_low.as_str()],
                    &scope,
                ),
            )
            .await
            .string_at("deploymentArn");
        let second_deployment = emulator
            .create(
                "deployments",
                "the second deployment of the higher-priority policy",
                deployment_body("narrow-waf", true, &[waf_high.as_str()], &narrow_scope),
            )
            .await
            .string_at("deploymentArn");

        Self {
            emulator,
            waf_high,
            waf_low,
            first_deployment,
            second_deployment,
        }
    }
}

/// RULE: an entity's coverage is the resources the scopes of its published deployments name,
/// each row carrying the resource's type, the policy's firewall type, the deployments the
/// coverage runs through and the account the resource ARN names.
#[tokio::test]
async fn an_entity_covers_the_resources_its_deployments_scopes_name() {
    let fixture = Fixture::new().await;

    /// One row: the resource covered, the type the scope selected it under, the account
    /// its ARN names, and the deployments through which the higher-priority policy
    /// reaches it.
    struct Row {
        label: &'static str,
        resource_arn: &'static str,
        resource_type: &'static str,
        owner: Option<&'static str>,
        deployments: usize,
    }

    let rows = vec![
        Row {
            label: "a load balancer reached through two deployments",
            resource_arn: ALB_A,
            resource_type: TYPE_ALB,
            owner: Some("111122223333"),
            deployments: 2,
        },
        Row {
            label: "a load balancer owned by another account",
            resource_arn: ALB_B,
            resource_type: TYPE_ALB,
            owner: Some("444455556666"),
            deployments: 1,
        },
        Row {
            label: "a stage whose ARN names no account",
            resource_arn: STAGE,
            resource_type: TYPE_STAGE,
            owner: None,
            deployments: 1,
        },
    ];

    let listed = fixture
        .emulator
        .all_coverages(
            "/resource-coverage/entities",
            &[("entityArn", fixture.waf_high.as_str())],
            None,
        )
        .await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "the higher-priority policy covers exactly the explicitly named resources, got \
         {listed:?}"
    );

    for row in &rows {
        let label = row.label;
        let reported = row_for(&listed, label, row.resource_arn, &fixture.waf_high);
        assert_eq!(
            reported["entityType"].as_str(),
            Some("Policy"),
            "case {label}: the covering entity is a policy, got {:?}",
            reported["entityType"]
        );
        assert_eq!(
            reported["resourceType"].as_str(),
            Some(row.resource_type),
            "case {label}: expected the ScopeResourceType the scope selected it under, got \
             {:?}",
            reported["resourceType"]
        );
        assert_eq!(
            reported["firewallType"].as_str(),
            Some("WAF"),
            "case {label}: expected the policy's firewall type, got {:?}",
            reported["firewallType"]
        );
        assert_eq!(
            reported["coverageStatus"].as_str(),
            Some("IN_EFFECT"),
            "case {label}: a firewall type that combines policies leaves each in effect, got \
             {:?}",
            reported["coverageStatus"]
        );
        assert_eq!(
            reported["resourceOwnerAccountId"].as_str(),
            row.owner,
            "case {label}: expected the account the resource ARN names, got {:?}",
            reported["resourceOwnerAccountId"]
        );
        let deployment_arns = reported["deploymentArns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: deploymentArns is a list, got {reported:?}"));
        assert_eq!(
            deployment_arns.len(),
            row.deployments,
            "case {label}: expected {} deployment(s), got {deployment_arns:?}",
            row.deployments
        );
        assert!(
            deployment_arns
                .iter()
                .any(|arn| arn.as_str() == Some(fixture.first_deployment.as_str())),
            "case {label}: the deployment that applies the policy must be named, got \
             {deployment_arns:?}"
        );
        if row.deployments == 2 {
            assert!(
                deployment_arns
                    .iter()
                    .any(|arn| arn.as_str() == Some(fixture.second_deployment.as_str())),
                "case {label}: both deployments must be named, got {deployment_arns:?}"
            );
        }
        assert!(
            reported["updatedAt"].is_string(),
            "case {label}: the evaluation time is a date-time string, got {:?}",
            reported["updatedAt"]
        );
    }

    // The lower-priority policy is applied by one deployment only, so it covers the three
    // resources that deployment's scope names.
    let listed_low = fixture
        .emulator
        .all_coverages(
            "/resource-coverage/entities",
            &[("entityArn", fixture.waf_low.as_str())],
            None,
        )
        .await;
    assert_eq!(
        listed_low.len(),
        3,
        "the lower-priority policy covers its one deployment's resources, got {listed_low:?}"
    );
    for resource_arn in [ALB_A, ALB_B, STAGE] {
        let reported = row_for(&listed_low, "the lower-priority policy", resource_arn, &fixture.waf_low);
        assert_eq!(
            reported["deploymentArns"],
            serde_json::json!([fixture.first_deployment]),
            "case the lower-priority policy: expected its one deployment, got {:?}",
            reported["deploymentArns"]
        );
    }
}

/// RULE: the entities covering one resource are the policies that reach it, and the
/// `entityType` filter reports them only when it asks for the one supported entity type.
#[tokio::test]
async fn a_resource_is_covered_by_every_policy_that_reaches_it() {
    let fixture = Fixture::new().await;

    let listed = fixture
        .emulator
        .all_coverages(
            "/resource-coverage/protected-resources",
            &[("resourceArn", ALB_A)],
            None,
        )
        .await;
    assert_eq!(
        listed.len(),
        2,
        "both policies of the deployment cover the load balancer, got {listed:?}"
    );
    for entity_arn in [&fixture.waf_high, &fixture.waf_low] {
        let reported = row_for(&listed, "the covered load balancer", ALB_A, entity_arn);
        assert_eq!(
            reported["resourceType"].as_str(),
            Some(TYPE_ALB),
            "case the covered load balancer: expected its ScopeResourceType, got {:?}",
            reported["resourceType"]
        );
    }

    // The rows differ along the `entityType` asked for: the one supported value reports
    // the same set, every other value reports none, and a resource no scope names is
    // covered by nothing at all.
    struct Row {
        label: &'static str,
        resource_arn: &'static str,
        entity_type: Option<&'static str>,
        expected: usize,
    }
    let rows = vec![
        Row {
            label: "entityType Policy, the only supported value",
            resource_arn: ALB_A,
            entity_type: Some("Policy"),
            expected: 2,
        },
        Row {
            label: "entityType Deployment",
            resource_arn: ALB_A,
            entity_type: Some("Deployment"),
            expected: 0,
        },
        Row {
            label: "entityType Scope",
            resource_arn: ALB_A,
            entity_type: Some("Scope"),
            expected: 0,
        },
        Row {
            label: "a stage covered by both policies",
            resource_arn: STAGE,
            entity_type: None,
            expected: 2,
        },
        Row {
            label: "a resource no scope names",
            resource_arn: UNCOVERED,
            entity_type: None,
            expected: 0,
        },
    ];

    for row in rows {
        let label = row.label;
        let mut query: Vec<(&str, &str)> = vec![("resourceArn", row.resource_arn)];
        if let Some(entity_type) = row.entity_type {
            query.push(("entityType", entity_type));
        }
        let reply = fixture
            .emulator
            .coverages("/resource-coverage/protected-resources", &query)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label}: expected 200, got {} {:?}",
            reply.status, reply.json
        );
        let reported = reply.json["resourceCoverages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: resourceCoverages is required, got {:?}", reply.json));
        assert_eq!(
            reported.len(),
            row.expected,
            "case {label}: expected {} row(s), got {reported:?}",
            row.expected
        );
    }
}

/// RULE: `coverageStatus` follows the firewall type — one that combines policies leaves
/// every covering policy in effect, one that applies a single policy leaves only the
/// highest-priority (lowest `priority`) policy in effect and supersedes the others.
#[tokio::test]
async fn coverage_status_follows_the_firewall_types_rule() {
    let emulator = Emulator::new();

    /// One row: the firewall type two policies contest a resource with, and the status
    /// each must report.
    struct Row {
        label: &'static str,
        firewall_type: &'static str,
        higher_status: &'static str,
        lower_status: &'static str,
    }

    // `PolicyFirewallType`'s values (`smithy_query(query="shape",shape="PolicyFirewallType")`):
    // AWS WAF and AWS Shield Advanced combine policies, IGW_FIREWALL applies a single one.
    let rows = vec![
        Row {
            label: "WAF, a firewall type that combines policies",
            firewall_type: "WAF",
            higher_status: "IN_EFFECT",
            lower_status: "IN_EFFECT",
        },
        Row {
            label: "SHIELD_ADVANCED, which also combines policies",
            firewall_type: "SHIELD_ADVANCED",
            higher_status: "IN_EFFECT",
            lower_status: "IN_EFFECT",
        },
        Row {
            label: "IGW_FIREWALL, a firewall type that applies a single policy",
            firewall_type: "IGW_FIREWALL",
            higher_status: "IN_EFFECT",
            lower_status: "SUPERSEDED",
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let label = row.label;
        // Each row contests a resource of its own, so no row's policies decide another
        // row's status.
        let resource_arn = format!(
            "arn:aws:elasticloadbalancing:us-east-1:111122223333:loadbalancer/app/contest{index}/0123456789abcdef"
        );
        let scope = emulator
            .create(
                "scopes",
                label,
                scope_body(
                    &format!("contested-scope-{index}"),
                    true,
                    serde_json::json!({ TYPE_ALB: include_arns(&[resource_arn.as_str()]) }),
                ),
            )
            .await
            .string_at("scopeArn");
        // The lower-priority policy is created FIRST, so the status cannot be following
        // creation order.
        let lower = emulator
            .create(
                "policies",
                label,
                policy_body(
                    &format!("contest-low-{index}"),
                    row.firewall_type,
                    30,
                    true,
                ),
            )
            .await
            .string_at("policyArn");
        let higher = emulator
            .create(
                "policies",
                label,
                policy_body(&format!("contest-high-{index}"), row.firewall_type, 2, true),
            )
            .await
            .string_at("policyArn");
        let deployment = emulator
            .create(
                "deployments",
                label,
                deployment_body(
                    &format!("contest-{index}"),
                    true,
                    &[lower.as_str(), higher.as_str()],
                    &scope,
                ),
            )
            .await
            .string_at("deploymentArn");

        let listed = emulator
            .all_coverages(
                "/resource-coverage/protected-resources",
                &[
                    ("resourceArn", resource_arn.as_str()),
                    ("entityType", "Policy"),
                ],
                None,
            )
            .await;
        assert_eq!(
            listed.len(),
            2,
            "case {label}: both contesting policies are reported, got {listed:?}"
        );
        for (entity_arn, expected) in [(&higher, row.higher_status), (&lower, row.lower_status)] {
            let reported = row_for(&listed, label, &resource_arn, entity_arn);
            assert_eq!(
                reported["coverageStatus"].as_str(),
                Some(expected),
                "case {label}: expected {expected} for {entity_arn}, got {:?}",
                reported["coverageStatus"]
            );
            assert_eq!(
                reported["deploymentArns"],
                serde_json::json!([deployment]),
                "case {label}: expected the contesting deployment, got {:?}",
                reported["deploymentArns"]
            );
            assert_eq!(
                reported["firewallType"].as_str(),
                Some(row.firewall_type),
                "case {label}: expected the contested firewall type, got {:?}",
                reported["firewallType"]
            );
        }
    }
}

/// RULE: an unpublished entity enforces nothing, so neither a draft deployment nor a draft
/// policy covers any resource, and a scope selection that names no resource of this service
/// — every resource of a type, or an exclusion — contributes no coverage.
#[tokio::test]
async fn an_unpublished_entity_or_an_unnamed_selection_covers_nothing() {
    let emulator = Emulator::new();

    let published_scope = emulator
        .create(
            "scopes",
            "the published scope",
            scope_body(
                "draft-probe-scope",
                true,
                serde_json::json!({ TYPE_ALB: include_arns(&[ALB_A]) }),
            ),
        )
        .await
        .string_at("scopeArn");
    let every_resource_scope = emulator
        .create(
            "scopes",
            "a scope selecting every resource of a type",
            scope_body(
                "include-all-scope",
                true,
                serde_json::json!({ TYPE_ALB: { "includeAll": true } }),
            ),
        )
        .await
        .string_at("scopeArn");
    let excluding_scope = emulator
        .create(
            "scopes",
            "a scope selecting by exclusion",
            scope_body(
                "excluding-scope",
                true,
                serde_json::json!({
                    TYPE_ALB: { "exclude": { "explicitArns": [ALB_B] } },
                }),
            ),
        )
        .await
        .string_at("scopeArn");

    let published_policy = emulator
        .create(
            "policies",
            "the published policy",
            policy_body("draft-probe-published", "WAF", 1, true),
        )
        .await
        .string_at("policyArn");
    let draft_policy = emulator
        .create(
            "policies",
            "the draft policy",
            policy_body("draft-probe-draft", "WAF", 1, false),
        )
        .await
        .string_at("policyArn");

    /// One row: the case, the deployment that carries it, and the policy whose coverage is
    /// asked about.
    struct Row {
        label: &'static str,
        deployment: serde_json::Value,
        entity: usize,
    }

    let rows = vec![
        Row {
            label: "a draft deployment of a published policy",
            deployment: deployment_body(
                "draft-deployment",
                false,
                &[published_policy.as_str()],
                &published_scope,
            ),
            entity: 0,
        },
        Row {
            label: "a published deployment of a draft policy",
            deployment: deployment_body(
                "published-draft-policy",
                true,
                &[draft_policy.as_str()],
                &published_scope,
            ),
            entity: 1,
        },
        Row {
            label: "a scope selecting every resource of a type",
            deployment: deployment_body(
                "include-all-deployment",
                true,
                &[published_policy.as_str()],
                &every_resource_scope,
            ),
            entity: 0,
        },
        Row {
            label: "a scope selecting by exclusion only",
            deployment: deployment_body(
                "excluding-deployment",
                true,
                &[published_policy.as_str()],
                &excluding_scope,
            ),
            entity: 0,
        },
    ];

    let entities = [published_policy.clone(), draft_policy.clone()];
    for row in rows {
        let label = row.label;
        emulator.create("deployments", label, row.deployment).await;
        let entity_arn = &entities[row.entity];
        let listed = emulator
            .all_coverages(
                "/resource-coverage/entities",
                &[("entityArn", entity_arn.as_str())],
                None,
            )
            .await;
        assert!(
            listed.is_empty(),
            "case {label}: nothing is covered, got {listed:?}"
        );
        let by_resource = emulator
            .all_coverages(
                "/resource-coverage/protected-resources",
                &[("resourceArn", ALB_A)],
                None,
            )
            .await;
        assert!(
            by_resource.is_empty(),
            "case {label}: the resource is covered by nothing, got {by_resource:?}"
        );
    }
}

/// RULE: `resourceOwnerAccountId` filters an entity's coverage to the resources that
/// account owns, and an `entityArn` naming no entity of this service is covered by nothing.
#[tokio::test]
async fn an_entitys_coverage_is_filtered_by_the_resource_owner() {
    let fixture = Fixture::new().await;

    // The rows differ along the account asked for: each of the two owners in the fixture,
    // an account that owns none of the covered resources, and the entity's whole coverage.
    let rows: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        (
            "the account owning the first load balancer",
            Some("111122223333"),
            vec![ALB_A],
        ),
        (
            "the account owning the second load balancer",
            Some("444455556666"),
            vec![ALB_B],
        ),
        ("an account owning none of them", Some("999988887777"), vec![]),
        ("no filter at all", None, vec![ALB_A, ALB_B, STAGE]),
    ];

    for (label, owner, expected) in rows {
        let mut query: Vec<(&str, &str)> = vec![("entityArn", fixture.waf_low.as_str())];
        if let Some(owner) = owner {
            query.push(("resourceOwnerAccountId", owner));
        }
        let listed = fixture
            .emulator
            .all_coverages("/resource-coverage/entities", &query, None)
            .await;
        let mut reported: Vec<&str> = listed
            .iter()
            .map(|row| {
                row["resourceArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label}: a row names its resource, got {row:?}"))
            })
            .collect();
        reported.sort_unstable();
        let mut wanted = expected.clone();
        wanted.sort_unstable();
        assert_eq!(
            reported, wanted,
            "case {label}: unexpected covered resources"
        );
    }

    // An ARN naming no entity of this service: a listing, not a refusal — the operation
    // models no not-found error.
    let listed = fixture
        .emulator
        .all_coverages(
            "/resource-coverage/entities",
            &[(
                "entityArn",
                "arn:aws:network-security-manager:us-east-1:000000000000:policy/absentpolicy",
            )],
            None,
        )
        .await;
    assert!(
        listed.is_empty(),
        "an entity this service does not hold covers nothing, got {listed:?}"
    );
}

/// RULE: an `entityArn` carrying the `:DRAFT` qualifier the model forbids is refused with
/// the modelled `ValidationException`, naming the member.
#[tokio::test]
async fn an_entity_arn_with_a_draft_qualifier_is_refused() {
    let fixture = Fixture::new().await;

    // The rows differ along which entity the ARN names and whether the entity exists: the
    // qualifier is refused before any lookup.
    let rows = vec![
        (
            "the fixture's own policy, draft-qualified",
            format!("{}:DRAFT", fixture.waf_high),
        ),
        (
            "a policy this service does not hold",
            "arn:aws:network-security-manager:us-east-1:000000000000:policy/absentpolicy:DRAFT"
                .to_string(),
        ),
        (
            "a deployment ARN, draft-qualified",
            format!("{}:DRAFT", fixture.first_deployment),
        ),
    ];

    for (label, entity_arn) in rows {
        let reply = fixture
            .emulator
            .coverages(
                "/resource-coverage/entities",
                &[("entityArn", entity_arn.as_str())],
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {label}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.json["message"].as_str(),
            Some("entityArn must not include a :DRAFT qualifier."),
            "case {label}: unexpected message, got {:?}",
            reply.json["message"]
        );
        assert_eq!(
            reply.json["reason"].as_str(),
            Some("FIELD_VALIDATION_FAILED"),
            "case {label}: unexpected reason, got {:?}",
            reply.json["reason"]
        );
        assert_eq!(
            reply.json["fieldList"],
            serde_json::json!([{
                "name": "entityArn",
                "message": "entityArn must not include a :DRAFT qualifier.",
            }]),
            "case {label}: unexpected fieldList, got {:?}",
            reply.json["fieldList"]
        );
    }
}

/// RULE: either listing pages with `maxResults` and an opaque `nextToken` that resumes
/// exactly where the previous page stopped, reporting every row once however it is walked —
/// and a `nextToken` this service did not mint is refused.
#[tokio::test]
async fn a_coverage_listing_pages_through_every_row_once() {
    let fixture = Fixture::new().await;

    let whole = fixture
        .emulator
        .all_coverages(
            "/resource-coverage/entities",
            &[("entityArn", fixture.waf_high.as_str())],
            None,
        )
        .await;
    assert_eq!(
        whole.len(),
        3,
        "the fixture covers three resources, got {whole:?}"
    );

    // The page sizes: one row per page, a page shorter than the row count, the exact
    // count, and `MaxResults`' modelled maximum (range 1..100).
    for max_results in [1, 2, 3, 100] {
        let walked = fixture
            .emulator
            .all_coverages(
                "/resource-coverage/entities",
                &[("entityArn", fixture.waf_high.as_str())],
                Some(max_results),
            )
            .await;
        assert_eq!(
            walked, whole,
            "case maxResults={max_results}: the walk must report the same rows in the same \
             order"
        );

        let by_resource = fixture
            .emulator
            .all_coverages(
                "/resource-coverage/protected-resources",
                &[("resourceArn", ALB_A)],
                Some(max_results),
            )
            .await;
        assert_eq!(
            by_resource.len(),
            2,
            "case maxResults={max_results}: both policies cover the load balancer, got \
             {by_resource:?}"
        );
    }

    // A token this service did not mint names no page: a refusal, not a silent restart.
    for path in [
        "/resource-coverage/entities",
        "/resource-coverage/protected-resources",
    ] {
        let member = if path.ends_with("entities") {
            ("entityArn", fixture.waf_high.as_str())
        } else {
            ("resourceArn", ALB_A)
        };
        let reply = fixture
            .emulator
            .coverages(path, &[member, ("nextToken", "9:deadbeef")])
            .await;
        assert_eq!(
            reply.status, 400,
            "case {path}: a foreign nextToken is refused, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {path}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.json["fieldList"][0]["name"].as_str(),
            Some("nextToken"),
            "case {path}: the refusal names the member, got {:?}",
            reply.json["fieldList"]
        );
    }
}
