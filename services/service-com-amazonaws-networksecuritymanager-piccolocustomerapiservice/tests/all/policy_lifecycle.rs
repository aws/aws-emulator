// SPDX-License-Identifier: CC0-1.0

//! The Policy lifecycle, end to end over HTTP: `CreatePolicy`, `GetPolicy`,
//! `ListPolicies`, `UpdatePolicy`, `DeletePolicy`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a client sees, not what a handler
//! returns. The rule under test is that each response is computed from the persisted
//! store: a create is proven by reading the resource back, an update by reading it back,
//! a delete by the read that follows failing.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::state::{
    ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState as PolicyStore,
    MAX_IDEMPOTENCY_RECORDS, MAX_POLICIES,
};
use smithy_server_runtime::MultiServiceRouter;

/// One policy summary/record field set, as JSON, plus the HTTP facts a caller branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error code (`x-amzn-errortype`), absent on success.
    error_code: Option<String>,
    json: serde_json::Value,
}

/// The emulator under test: this one service mounted on the production router.
struct Emulator {
    router: MultiServiceRouter,
    /// The same shared engine the router handed the service, so a test can assert what
    /// the store holds (and what it no longer holds) as well as what the wire returns.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The wire behaviour under test is the operations', not SigV4's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// How many `@idempotencyToken` records the store holds.
    fn idempotency_len(&self) -> usize {
        service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::state::service_state(
            &*self.engine,
        )
        .expect("the service state is reachable from the shared engine")
        .lock()
        .idempotency
        .len()
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

    async fn create(&self, body: serde_json::Value) -> Reply {
        self.call("POST", "/policies", Some(body)).await
    }

    async fn get(&self, identifier: &str) -> Reply {
        self.call("GET", &format!("/policies/{}", path_label(identifier)), None)
            .await
    }

    async fn update(&self, identifier: &str, body: serde_json::Value) -> Reply {
        self.call(
            "PATCH",
            &format!("/policies/{}", path_label(identifier)),
            Some(body),
        )
        .await
    }

    async fn delete(&self, identifier: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/policies/{}", path_label(identifier)),
            None,
        )
        .await
    }

    /// Follow `nextToken` to the end and return every `policyId` listed, in page order.
    async fn list_all(&self, max_results: Option<u32>, status: Option<&str>) -> Vec<String> {
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
                "/policies".to_string()
            } else {
                format!("/policies?{}", params.join("&"))
            };
            let reply = self.call("GET", &path, None).await;
            assert_eq!(
                reply.status, 200,
                "listing {path}: expected 200, got {} {:?}",
                reply.status, reply.json
            );
            let page = reply.json["policies"]
                .as_array()
                .unwrap_or_else(|| panic!("listing {path}: policies is required, got {:?}", reply.json))
                .clone();
            if let Some(limit) = max_results {
                assert!(
                    page.len() <= limit as usize,
                    "listing {path}: a page may hold at most maxResults={limit}, got {}",
                    page.len()
                );
            }
            for summary in &page {
                ids.push(policy_id_of(summary));
            }
            match reply.json["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        !page.is_empty(),
                        "listing {path}: a page that continues must not be empty"
                    );
                    next = Some(token.to_string());
                }
                None => return ids,
            }
        }
        panic!("nextToken never terminated the listing");
    }
}

/// `policyIdentifier` is a NON-greedy `@httpLabel`, so an identifier containing `/` — the
/// policy's ARN — reaches the server percent-encoded, exactly as an AWS SDK sends it.
fn path_label(identifier: &str) -> String {
    identifier.replace('/', "%2F")
}

fn policy_id_of(value: &serde_json::Value) -> String {
    value["policyId"]
        .as_str()
        .unwrap_or_else(|| panic!("policyId is required, got {value:?}"))
        .to_string()
}

fn string_at(value: &serde_json::Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} is required, got {value:?}"))
        .to_string()
}

/// `PolicyId`'s modelled pattern, `^[a-z0-9]{1,64}$`.
fn is_policy_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// `UpdateToken`'s modelled pattern, `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`.
fn is_update_token(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
}

/// The ARN the service must mint for `policy_id`, built from the same account/ARN
/// helpers the handlers use rather than from a hardcoded literal.
fn expected_policy_arn(policy_id: &str) -> String {
    engine::emulator_arn(
        "network-security-manager",
        "us-east-1",
        &format!("policy/{policy_id}"),
    )
}

/// A `policyConfiguration` with no AWS WAF section. For a `SHIELD_ADVANCED` policy: the
/// model says `wafConfig` "is populated only for AWS WAF policies", and a WAF policy has
/// to carry it (see [`waf_configuration_with`]).
fn plain_configuration(remediation: bool, clean_up: bool) -> serde_json::Value {
    serde_json::json!({
        "remediationEnabled": remediation,
        "resourcesCleanUp": clean_up,
    })
}

/// A `policyConfiguration` carrying `wafConfig`, with the remediation flags a row varies.
/// Every member `WafConfig` declares is supplied, which is what a WAF policy requires.
fn waf_configuration_with(
    remediation: bool,
    clean_up: bool,
    resolution: &str,
) -> serde_json::Value {
    serde_json::json!({
        "remediationEnabled": remediation,
        "resourcesCleanUp": clean_up,
        "wafConfig": {
            "existingCustomerWebACLResolution": resolution,
            "conflictResolution": "MERGE_WHERE_APPLICABLE",
        },
    })
}

/// A `policyConfiguration` carrying `wafConfig`. The two enum values come from
/// `ExistingCustomerWebACLResolution` / `WAFConflictResolutionOptions` in the model.
fn waf_configuration(resolution: &str) -> serde_json::Value {
    waf_configuration_with(true, true, resolution)
}

/// The association list every WAF create owes: the model says of
/// `associatedTemplateAndRuleList` "For AWS WAF policies, specify 1 to 100 templates or
/// rules", so a WAF policy that associates nothing is refused rather than created.
fn waf_associations(id: &str) -> serde_json::Value {
    serde_json::json!([{ "ruleIdentifier": reference_arn("rule", id) }])
}

/// An ARN of one of this service's other resource types, for an association reference.
fn reference_arn(resource_type: &str, id: &str) -> String {
    engine::emulator_arn(
        "network-security-manager",
        "us-east-1",
        &format!("{resource_type}/{id}"),
    )
}

/// One row of the create table: the inputs a create may vary along.
#[derive(Debug, Clone)]
struct CreateCase {
    policy_name: &'static str,
    firewall_type: &'static str,
    priority: i64,
    description: Option<&'static str>,
    /// `None` exercises the modelled `@default: true`.
    is_published: Option<bool>,
    associations: Vec<serde_json::Value>,
    configuration: serde_json::Value,
    tags: Option<serde_json::Value>,
}

impl CreateCase {
    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "policyName": self.policy_name,
            "priority": self.priority,
            "firewallType": self.firewall_type,
            "policyConfiguration": self.configuration,
        });
        if let Some(description) = self.description {
            body["policyDescription"] = serde_json::json!(description);
        }
        if let Some(published) = self.is_published {
            body["isPublished"] = serde_json::json!(published);
        }
        if !self.associations.is_empty() {
            body["associatedTemplateAndRuleList"] = serde_json::json!(self.associations);
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
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

/// The create table: rows differ along firewall type, publish state (including the
/// defaulted absence), priority magnitude, description presence, association count and
/// kind, configuration shape, and tag presence.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            policy_name: "waf-published-one-template",
            firewall_type: "WAF",
            priority: 1,
            description: None,
            is_published: None,
            associations: vec![serde_json::json!({
                "templateIdentifier": reference_arn("template", "t0"),
            })],
            configuration: waf_configuration_with(false, true, "RETROFIT"),
            tags: None,
        },
        CreateCase {
            policy_name: "waf draft with description",
            firewall_type: "WAF",
            priority: 5,
            description: Some("a draft policy"),
            is_published: Some(false),
            associations: vec![serde_json::json!({
                "ruleIdentifier": reference_arn("rule", "r0"),
            })],
            configuration: waf_configuration_with(true, true, "NO_REMEDIATION"),
            tags: None,
        },
        CreateCase {
            policy_name: "waf.one.rule",
            firewall_type: "WAF",
            priority: 10,
            description: Some("one rule only"),
            is_published: Some(true),
            associations: vec![serde_json::json!({
                "ruleIdentifier": reference_arn("rule", "r1"),
            })],
            configuration: waf_configuration("RETROFIT"),
            tags: None,
        },
        CreateCase {
            policy_name: "waf_two_templates_and_a_rule",
            firewall_type: "WAF",
            // The top of `Priority`'s modelled `@range(min: 1)` domain.
            priority: i32::MAX as i64,
            description: None,
            is_published: Some(true),
            associations: vec![
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t1") }),
                serde_json::json!({ "ruleIdentifier": reference_arn("rule", "r2") }),
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t2") }),
            ],
            configuration: waf_configuration("OVERRIDE_ASSOCIATION"),
            tags: Some(serde_json::json!({ "team": "network", "env": "test" })),
        },
        CreateCase {
            policy_name: "shield advanced published",
            firewall_type: "SHIELD_ADVANCED",
            priority: 3,
            description: Some("shield advanced",),
            is_published: Some(true),
            associations: Vec::new(),
            configuration: plain_configuration(true, true),
            tags: Some(serde_json::json!({ "env": "test" })),
        },
        CreateCase {
            policy_name: "shield advanced draft",
            firewall_type: "SHIELD_ADVANCED",
            priority: 2,
            description: None,
            is_published: Some(false),
            associations: Vec::new(),
            configuration: plain_configuration(false, true),
            tags: None,
        },
    ]
}

/// A create mints and persists the policy, and the record a following read returns is
/// byte-for-byte the record the create answered with — by `policyId` and by `policyArn`.
#[tokio::test]
async fn create_persists_a_policy_every_read_returns_unchanged() {
    let emulator = Emulator::new();

    for case in create_cases() {
        let created = emulator.create(case.body()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} {:?}",
            created.status, created.json
        );

        let policy_id = policy_id_of(&created.json);
        assert!(
            is_policy_id(&policy_id),
            "case {case:?}: expected a minted policyId matching ^[a-z0-9]{{1,64}}$, got {policy_id:?}"
        );
        assert_eq!(
            string_at(&created.json, "policyArn"),
            expected_policy_arn(&policy_id),
            "case {case:?}: expected the modelled policy ARN, got {:?}",
            created.json["policyArn"]
        );
        assert_eq!(
            string_at(&created.json, "policyName"),
            case.policy_name,
            "case {case:?}: expected the stored policyName, got {:?}",
            created.json["policyName"]
        );
        assert_eq!(
            created.json["policyDescription"].as_str(),
            case.description,
            "case {case:?}: expected the stored policyDescription, got {:?}",
            created.json["policyDescription"]
        );
        assert_eq!(
            string_at(&created.json, "status"),
            case.expected_status(),
            "case {case:?}: expected the status isPublished prescribes, got {:?}",
            created.json["status"]
        );
        assert_eq!(
            created.json["priority"].as_i64(),
            Some(case.priority),
            "case {case:?}: expected the stored priority, got {:?}",
            created.json["priority"]
        );
        assert_eq!(
            string_at(&created.json, "firewallType"),
            case.firewall_type,
            "case {case:?}: expected the stored firewallType, got {:?}",
            created.json["firewallType"]
        );
        assert_eq!(
            string_at(&created.json, "version"),
            "1",
            "case {case:?}: a new policy is version 1, got {:?}",
            created.json["version"]
        );
        let update_token = string_at(&created.json, "updateToken");
        assert!(
            is_update_token(&update_token),
            "case {case:?}: expected a UUID updateToken, got {update_token:?}"
        );
        assert_eq!(
            created.json["isSnapshot"].as_bool(),
            Some(false),
            "case {case:?}: a created policy is not a snapshot, got {:?}",
            created.json["isSnapshot"]
        );
        assert_eq!(
            created.json["hasPublishedVersion"].as_bool(),
            Some(case.is_published.unwrap_or(true)),
            "case {case:?}: expected hasPublishedVersion to follow isPublished, got {:?}",
            created.json["hasPublishedVersion"]
        );
        assert_eq!(
            created.json["policyConfiguration"], case.configuration,
            "case {case:?}: expected the stored policyConfiguration, got {:?}",
            created.json["policyConfiguration"]
        );
        // A `TemplateOrRuleReference` is answered as the matching
        // `AssociatedTemplateOrRule`: `templateIdentifier` -> `templateArn`,
        // `ruleIdentifier` -> `ruleArn`, in request order.
        let expected_associations: Vec<serde_json::Value> = case
            .associations
            .iter()
            .map(|reference| {
                if let Some(arn) = reference.get("templateIdentifier") {
                    serde_json::json!({ "templateArn": arn })
                } else {
                    serde_json::json!({ "ruleArn": reference["ruleIdentifier"] })
                }
            })
            .collect();
        assert_eq!(
            created.json["associatedTemplateAndRuleList"],
            serde_json::Value::Array(expected_associations),
            "case {case:?}: expected the stored associations, got {:?}",
            created.json["associatedTemplateAndRuleList"]
        );
        // `DateTimestamp` is rendered by the framework's restJson1 timestamp codec; what
        // matters here is that the member is populated from the stored instant and that
        // every later read returns the SAME value (asserted by the round trip below).
        assert!(
            !created.json["updatedAt"].is_null(),
            "case {case:?}: expected an updatedAt timestamp, got {:?}",
            created.json["updatedAt"]
        );

        // The round trip: both identifier forms resolve to the SAME persisted record.
        for identifier in [policy_id.clone(), expected_policy_arn(&policy_id)] {
            let read = emulator.get(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetPolicy {identifier} expected 200, got {} {:?}",
                read.status, read.json
            );
            assert_eq!(
                read.json, created.json,
                "case {case:?}: GetPolicy {identifier} must return the created record, got {:?}",
                read.json
            );
        }
    }
}

/// An identifier no policy answers to is a 404 `ResourceNotFoundException` — never a 501
/// and never an empty 200 — whatever form it takes.
#[tokio::test]
async fn an_unknown_identifier_is_resource_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create(create_cases()[0].body())
        .await;
    assert_eq!(created.status, 201, "setup create failed: {:?}", created.json);
    let live_id = policy_id_of(&created.json);

    let unknown: Vec<String> = vec![
        "abc123".to_string(),
        "0123456789abcdef0123456789abcdef1".to_string(),
        expected_policy_arn("deadbeef"),
        // A live id with its last character changed is still not a live id.
        format!("{}x", &live_id[..live_id.len() - 1]),
        engine::emulator_arn("network-security-manager", "us-west-2", "policy/abc123"),
    ];

    for identifier in unknown {
        let read = emulator.get(&identifier).await;
        assert_eq!(
            read.status, 404,
            "case {identifier:?}: expected 404, got {} {:?}",
            read.status, read.json
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {identifier:?}: expected ResourceNotFoundException, got {:?}",
            read.error_code
        );
    }
}

/// One row of the update table.
#[derive(Debug, Clone)]
struct UpdateCase {
    label: &'static str,
    description: Option<&'static str>,
    priority: Option<i64>,
    is_published: bool,
    associations: Option<Vec<serde_json::Value>>,
    configuration: Option<serde_json::Value>,
}

/// An update applies exactly the members it carries, leaves every absent one alone, bumps
/// the version by one, rotates the `updateToken`, and is observable through a read.
#[tokio::test]
async fn update_applies_only_the_members_it_carries() {
    let cases = vec![
        UpdateCase {
            label: "description only",
            description: Some("updated description"),
            priority: None,
            is_published: true,
            associations: None,
            configuration: None,
        },
        UpdateCase {
            label: "priority only",
            description: None,
            priority: Some(42),
            is_published: true,
            associations: None,
            configuration: None,
        },
        UpdateCase {
            label: "unpublish",
            description: None,
            priority: None,
            is_published: false,
            associations: None,
            configuration: None,
        },
        UpdateCase {
            label: "replace the associations",
            description: None,
            priority: None,
            is_published: true,
            associations: Some(vec![
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t9") }),
                serde_json::json!({ "ruleIdentifier": reference_arn("rule", "r9") }),
            ]),
            configuration: None,
        },
        UpdateCase {
            // An update REPLACES the list, so a one-entry list is a different list than
            // the seed's. Clearing it is refused on a WAF policy, which the association
            // rules suite covers.
            label: "replace the associations with a single template",
            description: None,
            priority: None,
            is_published: true,
            associations: Some(vec![
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t8") }),
            ]),
            configuration: None,
        },
        UpdateCase {
            label: "replace the configuration",
            description: None,
            priority: None,
            is_published: true,
            associations: None,
            configuration: Some(waf_configuration("NO_REMEDIATION")),
        },
        UpdateCase {
            label: "every member at once",
            description: Some("all of it"),
            priority: Some(7),
            is_published: false,
            associations: Some(vec![serde_json::json!({
                "ruleIdentifier": reference_arn("rule", "r7"),
            })]),
            configuration: Some(waf_configuration_with(true, true, "OVERRIDE_ASSOCIATION")),
        },
    ];

    for case in cases {
        // A fresh emulator per row, so each row updates a policy in a known state.
        let emulator = Emulator::new();
        let seed = CreateCase {
            policy_name: "seed policy",
            firewall_type: "WAF",
            priority: 4,
            description: Some("the seed description"),
            is_published: Some(true),
            associations: vec![serde_json::json!({
                "ruleIdentifier": reference_arn("rule", "seed"),
            })],
            configuration: waf_configuration("RETROFIT"),
            tags: None,
        }
        .body();
        let created = emulator.create(seed).await;
        assert_eq!(
            created.status, 201,
            "case {}: setup create failed with {:?}",
            case.label, created.json
        );
        let policy_id = policy_id_of(&created.json);
        let first_token = string_at(&created.json, "updateToken");

        let mut body = serde_json::json!({
            "updateToken": first_token,
            "isPublished": case.is_published,
        });
        if let Some(description) = case.description {
            body["policyDescription"] = serde_json::json!(description);
        }
        if let Some(priority) = case.priority {
            body["priority"] = serde_json::json!(priority);
        }
        if let Some(associations) = &case.associations {
            body["associatedTemplateAndRuleList"] = serde_json::json!(associations);
        }
        if let Some(configuration) = &case.configuration {
            body["policyConfiguration"] = configuration.clone();
        }

        let updated = emulator.update(&policy_id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            updated.status, updated.json
        );

        assert_eq!(
            string_at(&updated.json, "version"),
            "2",
            "case {case:?}: an update bumps EntityVersion to 2, got {:?}",
            updated.json["version"]
        );
        let second_token = string_at(&updated.json, "updateToken");
        assert!(
            is_update_token(&second_token) && second_token != first_token,
            "case {case:?}: an update must mint a NEW UUID updateToken, got {second_token:?}"
        );
        assert_eq!(
            string_at(&updated.json, "status"),
            if case.is_published { "ACTIVE" } else { "DRAFT" },
            "case {case:?}: status is recomputed from isPublished, got {:?}",
            updated.json["status"]
        );
        // Published once, published for ever: unpublishing a published policy does not
        // retract the published version.
        assert_eq!(
            updated.json["hasPublishedVersion"].as_bool(),
            Some(true),
            "case {case:?}: expected hasPublishedVersion to stay true, got {:?}",
            updated.json["hasPublishedVersion"]
        );

        // Provided members changed; absent members kept their stored value.
        let expected_description = case
            .description
            .map_or_else(|| created.json["policyDescription"].clone(), |text| serde_json::json!(text));
        assert_eq!(
            updated.json["policyDescription"], expected_description,
            "case {case:?}: expected policyDescription {expected_description:?}, got {:?}",
            updated.json["policyDescription"]
        );
        let expected_priority = case
            .priority
            .unwrap_or_else(|| created.json["priority"].as_i64().expect("a stored priority"));
        assert_eq!(
            updated.json["priority"].as_i64(),
            Some(expected_priority),
            "case {case:?}: expected priority {expected_priority}, got {:?}",
            updated.json["priority"]
        );
        let expected_configuration = case
            .configuration
            .clone()
            .unwrap_or_else(|| created.json["policyConfiguration"].clone());
        assert_eq!(
            updated.json["policyConfiguration"], expected_configuration,
            "case {case:?}: expected policyConfiguration {expected_configuration:?}, got {:?}",
            updated.json["policyConfiguration"]
        );
        let expected_associations = match &case.associations {
            None => created.json["associatedTemplateAndRuleList"].clone(),
            Some(references) => serde_json::Value::Array(
                references
                    .iter()
                    .map(|reference| {
                        if let Some(arn) = reference.get("templateIdentifier") {
                            serde_json::json!({ "templateArn": arn })
                        } else {
                            serde_json::json!({ "ruleArn": reference["ruleIdentifier"] })
                        }
                    })
                    .collect(),
            ),
        };
        assert_eq!(
            updated.json["associatedTemplateAndRuleList"], expected_associations,
            "case {case:?}: expected associations {expected_associations:?}, got {:?}",
            updated.json["associatedTemplateAndRuleList"]
        );
        // Identity never moves.
        assert_eq!(
            string_at(&updated.json, "policyId"),
            policy_id,
            "case {case:?}: an update must not re-mint the policyId"
        );
        assert_eq!(
            string_at(&updated.json, "policyArn"),
            string_at(&created.json, "policyArn"),
            "case {case:?}: an update must not re-mint the policyArn"
        );

        // The update is in the store, not just in the response.
        let read = emulator.get(&policy_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetPolicy after the update expected 200, got {}",
            read.status
        );
        assert_eq!(
            read.json, updated.json,
            "case {case:?}: GetPolicy must observe the update, got {:?}",
            read.json
        );

        // Replaying the same request now carries a stale token.
        let replay = emulator.update(&policy_id, body).await;
        assert_eq!(
            replay.status, 409,
            "case {case:?}: a stale updateToken expected 409, got {} {:?}",
            replay.status, replay.json
        );
        assert_eq!(
            replay.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            replay.error_code
        );
        // A refused update changed nothing.
        let after_conflict = emulator.get(&policy_id).await;
        assert_eq!(
            after_conflict.json, updated.json,
            "case {case:?}: a refused update must not change the stored record, got {:?}",
            after_conflict.json
        );
    }
}

/// Updating an identifier no policy answers to is a 404, and is checked before the token.
#[tokio::test]
async fn updating_an_unknown_policy_is_resource_not_found() {
    let emulator = Emulator::new();
    let reply = emulator
        .update(
            "abc123",
            serde_json::json!({
                "updateToken": "00000000-0000-0000-0000-000000000000",
                "isPublished": true,
            }),
        )
        .await;
    assert_eq!(
        reply.status, 404,
        "expected 404 for an unknown policy, got {} {:?}",
        reply.status, reply.json
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {:?}",
        reply.error_code
    );
}

/// Paging is an invariant: for any number of stored policies and any page size, following
/// `nextToken` to the end yields every policy exactly once, in one order, and the
/// unpaginated listing agrees.
#[tokio::test]
async fn paging_returns_every_policy_exactly_once_for_any_page_size() {
    for count in 0u32..=6 {
        let emulator = Emulator::new();
        let mut created: Vec<String> = Vec::new();
        for index in 0..count {
            let reply = emulator
                .create(serde_json::json!({
                    "policyName": format!("policy-{index}"),
                    "priority": index + 1,
                    "firewallType": "WAF",
                    "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
                    "associatedTemplateAndRuleList": waf_associations("r1"),
                }))
                .await;
            assert_eq!(
                reply.status, 201,
                "count {count}: setup create {index} failed with {:?}",
                reply.json
            );
            created.push(policy_id_of(&reply.json));
        }
        created.sort();

        let unpaginated = emulator.list_all(None, None).await;
        assert_eq!(
            unpaginated, created,
            "count {count}: an unpaginated listing must return every stored policy in \
             policyId order, got {unpaginated:?}"
        );

        for page_size in 1u32..=7 {
            let paged = emulator.list_all(Some(page_size), None).await;
            assert_eq!(
                paged, created,
                "count {count}, maxResults {page_size}: paging must yield every policy \
                 exactly once in the same order, got {paged:?}"
            );
        }
    }
}

/// `ListPolicies` honours the `status` filter, and `policies` is always present.
#[tokio::test]
async fn listing_filters_by_status() {
    let emulator = Emulator::new();
    let mut active: Vec<String> = Vec::new();
    let mut draft: Vec<String> = Vec::new();
    for (index, published) in [true, false, true, false, false].into_iter().enumerate() {
        let reply = emulator
            .create(serde_json::json!({
                "policyName": format!("status-{index}"),
                "priority": index + 1,
                "firewallType": "WAF",
                "isPublished": published,
                "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
                "associatedTemplateAndRuleList": waf_associations("r1"),
            }))
            .await;
        assert_eq!(reply.status, 201, "setup create failed: {:?}", reply.json);
        let id = policy_id_of(&reply.json);
        if published {
            active.push(id);
        } else {
            draft.push(id);
        }
    }
    active.sort();
    draft.sort();

    // Every `EntityStatusFilter` value from the model, including the one no policy has.
    for (filter, expected) in [
        ("ACTIVE", active.clone()),
        ("DRAFT", draft.clone()),
        ("DISABLED", Vec::new()),
    ] {
        for page_size in [None, Some(1), Some(2)] {
            let listed = emulator.list_all(page_size, Some(filter)).await;
            assert_eq!(
                listed, expected,
                "filter {filter}, maxResults {page_size:?}: expected {expected:?}, got {listed:?}"
            );
        }
    }
}

/// A `nextToken` this service did not issue is refused as a `ValidationException`, not
/// silently treated as the first page.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let emulator = Emulator::new();
    for token in ["abc", "2:6162", "1:zz", "1:", "notatoken=="] {
        let reply = emulator
            .call("GET", &format!("/policies?nextToken={token}"), None)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {token:?}: expected ValidationException, got {:?}",
            reply.error_code
        );
    }
}

/// A delete removes the record: the read that follows is a 404 and the listing no longer
/// carries it. `@idempotent`, so deleting an absent policy is still a 204.
#[tokio::test]
async fn delete_removes_the_record_and_is_idempotent() {
    let emulator = Emulator::new();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..3 {
        let reply = emulator
            .create(serde_json::json!({
                "policyName": format!("doomed-{index}"),
                "priority": index + 1,
                "firewallType": "WAF",
                "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
                "associatedTemplateAndRuleList": waf_associations("r1"),
            }))
            .await;
        assert_eq!(reply.status, 201, "setup create failed: {:?}", reply.json);
        ids.push(policy_id_of(&reply.json));
    }

    // Delete by id, by ARN, and twice over — every form answers 204 with an empty body.
    let targets = [
        ids[0].clone(),
        expected_policy_arn(&ids[1]),
        ids[2].clone(),
        ids[2].clone(),
        "abc123".to_string(),
    ];
    let mut remaining: Vec<String> = ids.clone();
    for target in targets {
        let reply = emulator.delete(&target).await;
        assert_eq!(
            reply.status, 204,
            "case {target:?}: expected 204, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.json,
            serde_json::Value::Null,
            "case {target:?}: a 204 carries an empty body, got {:?}",
            reply.json
        );
        remaining.retain(|id| *id != target && expected_policy_arn(id) != target);

        let mut expected = remaining.clone();
        expected.sort();
        let listed = emulator.list_all(Some(2), None).await;
        assert_eq!(
            listed, expected,
            "case {target:?}: expected the listing to be {expected:?}, got {listed:?}"
        );
    }

    for id in ids {
        let read = emulator.get(&id).await;
        assert_eq!(
            read.status, 404,
            "case {id:?}: a deleted policy reads as 404, got {} {:?}",
            read.status, read.json
        );
    }
}

/// `clientToken` is `@idempotencyToken`: a replayed create returns the first call's
/// policy, and a different token mints a different policy.
#[tokio::test]
async fn a_replayed_client_token_returns_the_same_policy() {
    let emulator = Emulator::new();
    let body_with = |token: &str, name: &str| {
        serde_json::json!({
            "clientToken": token,
            "policyName": name,
            "priority": 1,
            "firewallType": "WAF",
            "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
            "associatedTemplateAndRuleList": waf_associations("r1"),
        })
    };

    // Tokens spanning `IdempotencyToken`'s modelled `^[\x21-\x7E]+$` character set; the
    // policy name is kept independent of the token, which has a wider character set than
    // `PolicyName` allows.
    for (index, token) in ["tok-1", "A!~token", "0"].into_iter().enumerate() {
        let name = format!("named-{index}");
        let first = emulator.create(body_with(token, &name)).await;
        assert_eq!(
            first.status, 201,
            "case {token:?}: expected 201, got {} {:?}",
            first.status, first.json
        );
        let replay = emulator.create(body_with(token, &name)).await;
        assert_eq!(
            replay.status, 201,
            "case {token:?}: a replay expected 201, got {} {:?}",
            replay.status, replay.json
        );
        assert_eq!(
            replay.json, first.json,
            "case {token:?}: a replayed clientToken must return the first result, got {:?}",
            replay.json
        );
    }

    // Three tokens, three policies — the replays minted nothing extra.
    let listed = emulator.list_all(None, None).await;
    assert_eq!(
        listed.len(),
        3,
        "expected exactly one policy per distinct clientToken, got {listed:?}"
    );
}

/// A second policy cannot take a `policyName` already in use.
#[tokio::test]
async fn a_duplicate_policy_name_is_a_conflict() {
    let emulator = Emulator::new();
    let body = serde_json::json!({
        "policyName": "only-one-of-these",
        "priority": 1,
        "firewallType": "WAF",
        "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
        "associatedTemplateAndRuleList": waf_associations("r1"),
    });
    let first = emulator.create(body.clone()).await;
    assert_eq!(first.status, 201, "setup create failed: {:?}", first.json);

    let second = emulator.create(body.clone()).await;
    assert_eq!(
        second.status, 409,
        "expected 409 for a duplicate policyName, got {} {:?}",
        second.status, second.json
    );
    assert_eq!(
        second.error_code.as_deref(),
        Some("ConflictException"),
        "expected ConflictException, got {:?}",
        second.error_code
    );

    // The name is released with the policy.
    let deleted = emulator.delete(&policy_id_of(&first.json)).await;
    assert_eq!(deleted.status, 204, "setup delete failed: {:?}", deleted.json);
    let reused = emulator.create(body).await;
    assert_eq!(
        reused.status, 201,
        "expected the name to be reusable after a delete, got {} {:?}",
        reused.status, reused.json
    );
}

/// The association rules the model states in prose are refused as `ValidationException`:
/// a Shield Advanced policy associates nothing, a policy carries at most two templates,
/// and a reference is an ARN.
#[tokio::test]
async fn documented_association_rules_are_refused_as_validation_errors() {
    let emulator = Emulator::new();
    let cases = [
        (
            "shield advanced with a rule",
            "SHIELD_ADVANCED",
            vec![serde_json::json!({ "ruleIdentifier": reference_arn("rule", "r1") })],
        ),
        (
            "shield advanced with a template",
            "SHIELD_ADVANCED",
            vec![serde_json::json!({ "templateIdentifier": reference_arn("template", "t1") })],
        ),
        (
            "three templates",
            "WAF",
            vec![
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t1") }),
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t2") }),
                serde_json::json!({ "templateIdentifier": reference_arn("template", "t3") }),
            ],
        ),
        (
            "a rule reference that is not an ARN",
            "WAF",
            vec![serde_json::json!({ "ruleIdentifier": "my-rule" })],
        ),
        (
            "a template reference that is not an ARN",
            "WAF",
            vec![serde_json::json!({ "templateIdentifier": "my-template" })],
        ),
        // `AssociatedTemplateOrRule`'s members are `@length(20..1010)`, so a string that
        // looks like an ARN but is shorter than the modelled minimum could not be
        // returned as the association either.
        (
            "a reference shorter than the modelled ARN minimum",
            "WAF",
            vec![serde_json::json!({ "ruleIdentifier": "arn:aws:a:b:c:d" })],
        ),
        (
            "a reference whose ARN grammar does not match the modelled pattern",
            "WAF",
            vec![serde_json::json!({ "ruleIdentifier": "arn:AWS:network-security-manager:::rule/r1" })],
        ),
    ];

    for (label, firewall_type, associations) in cases {
        // The configuration is COMPLETE for the row's firewall type, so what the refusal
        // reports is the association list rather than a member of the configuration.
        let configuration = if firewall_type == "WAF" {
            waf_configuration("RETROFIT")
        } else {
            plain_configuration(false, true)
        };
        let reply = emulator
            .create(serde_json::json!({
                "policyName": format!("refused-{}", label.replace(' ', "-")),
                "priority": 1,
                "firewallType": firewall_type,
                "policyConfiguration": configuration,
                "associatedTemplateAndRuleList": associations,
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {label:?}: expected ValidationException, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.json["reason"].as_str(),
            Some("FIELD_VALIDATION_FAILED"),
            "case {label:?}: expected the field-validation reason, got {:?}",
            reply.json["reason"]
        );
        assert_eq!(
            reply.json["fieldList"][0]["name"].as_str(),
            Some("associatedTemplateAndRuleList"),
            "case {label:?}: the refusal must name the member, got {:?}",
            reply.json["fieldList"]
        );
    }

    // Nothing was stored by any refused create.
    let listed = emulator.list_all(None, None).await;
    assert!(
        listed.is_empty(),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// The policy store is BOUNDED: once it holds `MAX_POLICIES`, a further create is
/// refused with the modelled `ServiceQuotaExceededException` (HTTP 402) instead of
/// growing the store, the refusal stores nothing, and freeing a policy frees capacity.
#[tokio::test]
async fn creating_past_the_store_bound_is_refused() {
    let emulator = Emulator::new();
    let filler = |name: &str| {
        serde_json::json!({
            "policyName": name,
            "priority": 1,
            "firewallType": "WAF",
            "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
            "associatedTemplateAndRuleList": waf_associations("r1"),
        })
    };

    for index in 0..MAX_POLICIES {
        let name = format!("filler-{index:04}");
        let reply = emulator.create(filler(&name)).await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: a create inside the bound expected 201, got {} {:?}",
            reply.status, reply.json
        );
    }

    // Rows past the bound differ in the shape of the request, so the refusal is the
    // store's capacity and not something about one body: a plain create, one carrying an
    // idempotency token, and one carrying associations and tags.
    let past_bound: Vec<(&str, serde_json::Value)> = vec![
        ("plain", filler("over-plain")),
        (
            "with a clientToken",
            serde_json::json!({
                "clientToken": "over-token",
                "policyName": "over-token",
                "priority": 7,
                "firewallType": "WAF",
                "policyConfiguration": waf_configuration_with(true, true, "RETROFIT"),
                "associatedTemplateAndRuleList": waf_associations("r1"),
            }),
        ),
        (
            "with associations and tags",
            serde_json::json!({
                "policyName": "over-associated",
                "priority": 42,
                "firewallType": "WAF",
                "policyConfiguration": waf_configuration("RETROFIT"),
                "associatedTemplateAndRuleList": [
                    { "templateIdentifier": reference_arn("template", "t1") },
                    { "ruleIdentifier": reference_arn("rule", "r1") },
                ],
                "tags": { "team": "net" },
            }),
        ),
    ];
    for (label, body) in &past_bound {
        let reply = emulator.create(body.clone()).await;
        assert_eq!(
            reply.status, 402,
            "case {label:?}: a create past the bound expected 402, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {label:?}: expected ServiceQuotaExceededException, got {:?}",
            reply.error_code
        );
    }

    let listed = emulator.list_all(None, None).await;
    assert_eq!(
        listed.len(),
        MAX_POLICIES,
        "a refused create must not grow the store: expected {MAX_POLICIES} policies, got {}",
        listed.len()
    );

    // The bound is a capacity, not a permanent wall: deleting one admits one.
    let released = listed[0].clone();
    let deleted = emulator.delete(&released).await;
    assert_eq!(
        deleted.status, 204,
        "case {released:?}: the delete freeing capacity expected 204, got {} {:?}",
        deleted.status, deleted.json
    );
    let admitted = emulator.create(filler("admitted-after-delete")).await;
    assert_eq!(
        admitted.status, 201,
        "a create after freeing capacity expected 201, got {} {:?}",
        admitted.status, admitted.json
    );
}

/// Minted ids and tokens: every one matches the shape its model declares, and no two mints
/// collide. The id and the token are minted by the store rather than by a UUID crate, so
/// this pins both properties across many mints rather than one.
#[test]
fn minted_ids_and_tokens_are_well_formed_and_distinct() {
    use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::state::mint_update_token;

    let mut store = PolicyStore::default();
    let mut ids = std::collections::BTreeSet::new();
    let mut tokens = std::collections::BTreeSet::new();
    for case in 0..512 {
        let id = store.mint_policy_id();
        assert!(
            is_policy_id(&id),
            "case {case}: {id:?} does not match PolicyId's ^[a-z0-9]{{1,64}}$"
        );
        assert!(ids.insert(id.clone()), "case {case}: {id:?} was minted twice");

        let token = mint_update_token();
        assert!(
            is_update_token(&token),
            "case {case}: {token:?} does not match UpdateToken's pattern"
        );
        assert!(
            token.split('-').nth(2).is_some_and(|group| group.starts_with('4')),
            "case {case}: {token:?} is not a version-4 UUID"
        );
        assert!(
            tokens.insert(token.clone()),
            "case {case}: {token:?} was minted twice"
        );
    }
}

/// The idempotency store is bounded too: past `MAX_IDEMPOTENCY_RECORDS` the OLDEST
/// record is evicted, so a caller sending fresh `clientToken`s cannot grow it without
/// limit, and re-using one key replaces its record instead of adding another.
///
/// Asserted against the store itself: the bound is not observable on the wire until a
/// token older than the window is replayed, and the rule is about what the store holds.
#[test]
fn the_idempotency_store_is_bounded() {
    // Rows span below, at, just past and far past the bound.
    for written in [
        1usize,
        MAX_IDEMPOTENCY_RECORDS - 1,
        MAX_IDEMPOTENCY_RECORDS,
        MAX_IDEMPOTENCY_RECORDS + 1,
        MAX_IDEMPOTENCY_RECORDS * 3,
    ] {
        let mut store = PolicyStore::default();
        for index in 0..written {
            store.remember_idempotent_call(
                format!("CreatePolicy:token-{index:08}"),
                format!("policy{index}"),
                1,
            );
        }
        assert_eq!(
            store.idempotency.len(),
            written.min(MAX_IDEMPOTENCY_RECORDS),
            "case {written}: the idempotency store must hold at most \
             {MAX_IDEMPOTENCY_RECORDS} records, got {}",
            store.idempotency.len()
        );
        let newest = format!("CreatePolicy:token-{:08}", written - 1);
        assert!(
            store.idempotent_call(&newest).is_some(),
            "case {written}: the newest record {newest:?} must survive eviction"
        );
        if written > MAX_IDEMPOTENCY_RECORDS {
            let evicted = format!(
                "CreatePolicy:token-{:08}",
                written - MAX_IDEMPOTENCY_RECORDS - 1
            );
            assert!(
                store.idempotent_call(&evicted).is_none(),
                "case {written}: the oldest record {evicted:?} must be the one evicted"
            );
        }
    }

    // One key, many writes: the record is replaced, not accumulated.
    let mut store = PolicyStore::default();
    for version in 1..=5u32 {
        store.remember_idempotent_call(
            "UpdatePolicy:same-token".to_string(),
            "policy1".to_string(),
            version,
        );
    }
    assert_eq!(
        store.idempotency.len(),
        1,
        "re-using one idempotency key must replace its record, got {}",
        store.idempotency.len()
    );
    assert_eq!(
        store.idempotent_call("UpdatePolicy:same-token").map(|call| call.version),
        Some(5),
        "the retained record must be the latest, got {:?}",
        store.idempotent_call("UpdatePolicy:same-token")
    );
}

/// Deleting a policy releases what creating and updating it allocated: its idempotency
/// records go with it, and a token that produced it no longer replays it.
#[tokio::test]
async fn deleting_a_policy_releases_its_idempotency_records() {
    let emulator = Emulator::new();
    let body = serde_json::json!({
        "clientToken": "create-token",
        "policyName": "released-with-the-policy",
        "priority": 3,
        "firewallType": "WAF",
        "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
        "associatedTemplateAndRuleList": waf_associations("r1"),
    });
    let created = emulator.create(body.clone()).await;
    assert_eq!(
        created.status, 201,
        "setup create expected 201, got {} {:?}",
        created.status, created.json
    );
    let policy_id = policy_id_of(&created.json);
    let updated = emulator
        .update(
            &policy_id,
            serde_json::json!({
                "clientToken": "update-token",
                "updateToken": string_at(&created.json, "updateToken"),
                "isPublished": true,
                "policyDescription": "updated once",
            }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "setup update expected 200, got {} {:?}",
        updated.status, updated.json
    );
    assert_eq!(
        emulator.idempotency_len(),
        2,
        "the create and the update each leave one idempotency record, got {}",
        emulator.idempotency_len()
    );

    let deleted = emulator.delete(&policy_id).await;
    assert_eq!(
        deleted.status, 204,
        "the delete expected 204, got {} {:?}",
        deleted.status, deleted.json
    );
    assert_eq!(
        emulator.idempotency_len(),
        0,
        "a delete must release the deleted policy's idempotency records, got {}",
        emulator.idempotency_len()
    );

    // And the create token now acts as a fresh request rather than replaying a policy
    // nothing can read.
    let recreated = emulator.create(body).await;
    assert_eq!(
        recreated.status, 201,
        "a create replaying the token of a deleted policy expected 201, got {} {:?}",
        recreated.status, recreated.json
    );
    assert_ne!(
        policy_id_of(&recreated.json),
        policy_id,
        "the deleted policy must not come back: got {:?}",
        recreated.json
    );
}

/// `UpdatePolicy`'s `clientToken` replay returns the result of the update it already
/// applied — pinned to the version that update produced, so an intervening update is
/// never reported as this call's result (the replay then carries a stale `updateToken`
/// and is refused with `ConflictException`).
#[tokio::test]
async fn a_replayed_update_token_returns_the_update_it_applied() {
    let emulator = Emulator::new();
    let created = emulator
        .create(serde_json::json!({
            "policyName": "replayed-update",
            "priority": 1,
            "firewallType": "WAF",
            "policyConfiguration": waf_configuration_with(false, true, "RETROFIT"),
            "associatedTemplateAndRuleList": waf_associations("r1"),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "setup create expected 201, got {} {:?}",
        created.status, created.json
    );
    let policy_id = policy_id_of(&created.json);
    let patch = serde_json::json!({
        "clientToken": "patch-once",
        "updateToken": string_at(&created.json, "updateToken"),
        "isPublished": false,
        "priority": 9,
    });

    let first = emulator.update(&policy_id, patch.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first update expected 200, got {} {:?}",
        first.status, first.json
    );
    let replay = emulator.update(&policy_id, patch.clone()).await;
    assert_eq!(
        replay.json, first.json,
        "a replayed clientToken must return the update it applied, got {:?}",
        replay.json
    );
    assert_eq!(
        string_at(&replay.json, "version"),
        "2",
        "a replay must not bump the version again, got {:?}",
        replay.json
    );

    // An intervening update moves the record on; the replay's own `updateToken` is now
    // stale, so it is refused rather than answered with the newer record.
    let moved_on = emulator
        .update(
            &policy_id,
            serde_json::json!({
                "updateToken": string_at(&first.json, "updateToken"),
                "isPublished": true,
            }),
        )
        .await;
    assert_eq!(
        moved_on.status, 200,
        "the intervening update expected 200, got {} {:?}",
        moved_on.status, moved_on.json
    );
    let stale_replay = emulator.update(&policy_id, patch).await;
    assert_eq!(
        (stale_replay.status, stale_replay.error_code.as_deref()),
        (409, Some("ConflictException")),
        "a replay whose updateToken is stale must be a 409 ConflictException, got {} {:?}",
        stale_replay.status,
        stale_replay.json
    );
    let read_back = emulator.get(&policy_id).await;
    assert_eq!(
        string_at(&read_back.json, "version"),
        "3",
        "the refused replay must not have changed the record, got {:?}",
        read_back.json
    );
}
