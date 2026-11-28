// SPDX-License-Identifier: CC0-1.0

//! `CreatePolicySnapshot`, end to end over HTTP, together with the read of the snapshot it
//! mints and the live policy it was taken of.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a client sees, not what a handler
//! returns.
//!
//! RULE under test: a snapshot is an immutable copy of the policy's PUBLISHED version. It
//! carries that version's number and that version's body — `policyConfiguration`,
//! `associatedTemplateAndRuleList`, `policyDescription` — frozen as the version published
//! them, however far the live policy moves on afterwards; its `updatedAt` is the time the
//! snapshot was created; and the members that belong to the policy as a whole rather than
//! to one of its versions (`policyName`, `priority`, `firewallType`) are reported from the
//! LIVE policy. A policy that has published no version has nothing to copy and is refused
//! with the modelled `ConflictException`.

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

    async fn create_policy(&self, body: serde_json::Value) -> Reply {
        self.call("POST", "/policies", Some(body)).await
    }

    async fn get_policy(&self, identifier: &str) -> Reply {
        self.call("GET", &format!("/policies/{}", label(identifier)), None)
            .await
    }

    async fn update_policy(&self, identifier: &str, body: serde_json::Value) -> Reply {
        self.call(
            "PATCH",
            &format!("/policies/{}", label(identifier)),
            Some(body),
        )
        .await
    }

    async fn delete_policy(&self, identifier: &str) -> Reply {
        self.call("DELETE", &format!("/policies/{}", label(identifier)), None)
            .await
    }

    async fn create_snapshot(&self, identifier: &str, body: serde_json::Value) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{}/snapshots", label(identifier)),
            Some(body),
        )
        .await
    }
}

/// `policyIdentifier` is a NON-greedy `@httpLabel`, so an identifier containing `/` — the
/// policy's ARN — reaches the server percent-encoded, exactly as an AWS SDK sends it.
fn label(identifier: &str) -> String {
    identifier.replace('/', "%2F")
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

/// `UpdateToken`'s modelled pattern, `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`.
fn is_update_token(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

/// A `policyConfiguration` for a WAF policy: `WafConfig`'s two members are `@required` and
/// `resourcesCleanUp` has to be true. `resolution` is one of
/// `ExistingCustomerWebACLResolution`'s modelled values (RETROFIT, OVERRIDE_ASSOCIATION,
/// NO_REMEDIATION) and `MERGE_WHERE_APPLICABLE` the only value
/// `WAFConflictResolutionOptions` declares.
fn waf_configuration(remediation: bool, resolution: &str) -> serde_json::Value {
    serde_json::json!({
        "remediationEnabled": remediation,
        "resourcesCleanUp": true,
        "wafConfig": {
            "existingCustomerWebACLResolution": resolution,
            "conflictResolution": "MERGE_WHERE_APPLICABLE",
        },
    })
}

/// A `policyConfiguration` with no AWS WAF section, for a `SHIELD_ADVANCED` policy: the
/// model says `wafConfig` "is populated only for AWS WAF policies".
fn plain_configuration(remediation: bool) -> serde_json::Value {
    serde_json::json!({
        "remediationEnabled": remediation,
        "resourcesCleanUp": true,
    })
}

/// One row of the snapshot table: a policy to publish, and the update applied to the live
/// policy after its published version has been snapshotted.
struct SnapshotCase {
    label: &'static str,
    firewall_type: &'static str,
    priority: i64,
    description: Option<&'static str>,
    associations: Vec<serde_json::Value>,
    configuration: serde_json::Value,
    tags: Option<serde_json::Value>,
    /// The priority the live policy is moved to after the snapshot.
    next_priority: i64,
    /// The configuration the live policy is moved to after the snapshot.
    next_configuration: serde_json::Value,
    /// The associations the live policy is moved to after the snapshot.
    next_associations: Vec<serde_json::Value>,
}

impl SnapshotCase {
    fn create_body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "policyName": self.label,
            "priority": self.priority,
            "firewallType": self.firewall_type,
            "policyConfiguration": self.configuration,
            "isPublished": true,
        });
        if let Some(description) = self.description {
            body["policyDescription"] = serde_json::json!(description);
        }
        if !self.associations.is_empty() {
            body["associatedTemplateAndRuleList"] = serde_json::json!(self.associations);
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
        }
        body
    }

    /// The update that moves the live policy on, as a draft (`isPublished: false`) or as a
    /// new published version.
    fn update_body(&self, update_token: &str, is_published: bool) -> serde_json::Value {
        serde_json::json!({
            "updateToken": update_token,
            "isPublished": is_published,
            "priority": self.next_priority,
            "policyConfiguration": self.next_configuration,
            "associatedTemplateAndRuleList": self.next_associations,
        })
    }
}

/// The table: the rows differ along firewall type, priority magnitude (including the
/// modelled `@range(min: 1)` floor), description presence, how many associations the policy
/// holds and of which kind, the configuration's shape, and tag presence.
fn snapshot_cases() -> Vec<SnapshotCase> {
    vec![
        SnapshotCase {
            label: "waf-one-rule",
            firewall_type: "WAF",
            // `Priority`'s modelled floor.
            priority: 1,
            description: None,
            associations: vec![serde_json::json!({
                "ruleIdentifier": resource_arn("rule", "snapshotrule1"),
            })],
            configuration: waf_configuration(true, "NO_REMEDIATION"),
            tags: None,
            next_priority: 7,
            next_configuration: waf_configuration(false, "RETROFIT"),
            next_associations: vec![serde_json::json!({
                "ruleIdentifier": resource_arn("rule", "snapshotrule2"),
            })],
        },
        SnapshotCase {
            label: "waf.two-templates-and-a-rule",
            firewall_type: "WAF",
            priority: 2_147_483_647,
            description: Some("a described policy"),
            associations: vec![
                serde_json::json!({
                    "templateIdentifier": resource_arn("template", "snapshottemplate1"),
                }),
                serde_json::json!({
                    "templateIdentifier": resource_arn("template", "snapshottemplate2"),
                }),
                serde_json::json!({
                    "ruleIdentifier": resource_arn("rule", "snapshotrule3"),
                }),
            ],
            configuration: waf_configuration(false, "OVERRIDE_ASSOCIATION"),
            tags: Some(serde_json::json!({ "team": "network" })),
            next_priority: 1,
            next_configuration: waf_configuration(true, "NO_REMEDIATION"),
            // An update REPLACES the list; a WAF policy keeps at least one association,
            // so the live policy moves to a single rule.
            next_associations: vec![serde_json::json!({
                "ruleIdentifier": resource_arn("rule", "snapshotrule4"),
            })],
        },
        SnapshotCase {
            label: "shield-advanced-no-associations",
            firewall_type: "SHIELD_ADVANCED",
            priority: 500,
            description: Some("shield advanced"),
            // "For AWS Shield Advanced policies, this list must be empty."
            associations: vec![],
            configuration: plain_configuration(true),
            tags: Some(serde_json::json!({ "tier": "gold" })),
            next_priority: 9_999,
            next_configuration: plain_configuration(false),
            next_associations: vec![],
        },
    ]
}

/// Assert every member a snapshot of `version` must carry, given the frozen body it was
/// taken of and the live policy it belongs to.
#[allow(clippy::too_many_arguments)]
fn assert_snapshot(
    case: &str,
    snapshot: &Reply,
    policy_id: &str,
    policy_arn: &str,
    policy_name: &str,
    firewall_type: &str,
    version: u32,
    live_priority: i64,
    frozen_description: Option<&str>,
    frozen_configuration: &serde_json::Value,
    frozen_associations: usize,
) {
    assert_eq!(
        snapshot.json["isSnapshot"].as_bool(),
        Some(true),
        "case {case}: a snapshot must say so, got {:?}",
        snapshot.json["isSnapshot"]
    );
    assert_eq!(
        snapshot.json["hasPublishedVersion"].as_bool(),
        Some(true),
        "case {case}: a snapshot is a copy of a published version, got {:?}",
        snapshot.json["hasPublishedVersion"]
    );
    assert_eq!(
        snapshot.json["policyId"].as_str(),
        Some(policy_id),
        "case {case}: a snapshot is a version of its policy, so it carries that policyId, got {:?}",
        snapshot.json["policyId"]
    );
    assert_eq!(
        snapshot.json["policyArn"].as_str(),
        Some(format!("{policy_arn}:{version}").as_str()),
        "case {case}: unexpected policyArn, got {:?}",
        snapshot.json["policyArn"]
    );
    assert_eq!(
        snapshot.json["policyName"].as_str(),
        Some(policy_name),
        "case {case}: unexpected policyName, got {:?}",
        snapshot.json["policyName"]
    );
    assert_eq!(
        snapshot.json["firewallType"].as_str(),
        Some(firewall_type),
        "case {case}: unexpected firewallType, got {:?}",
        snapshot.json["firewallType"]
    );
    assert_eq!(
        snapshot.json["status"].as_str(),
        Some("ACTIVE"),
        "case {case}: a copy of a published version is in use, got {:?}",
        snapshot.json["status"]
    );
    assert_eq!(
        snapshot.json["version"].as_str(),
        Some(version.to_string().as_str()),
        "case {case}: a snapshot carries the version it copied, got {:?}",
        snapshot.json["version"]
    );
    assert_eq!(
        snapshot.json["priority"].as_i64(),
        Some(live_priority),
        "case {case}: priority is read from the live policy, got {:?}",
        snapshot.json["priority"]
    );
    assert_eq!(
        snapshot.json["policyDescription"].as_str(),
        frozen_description,
        "case {case}: policyDescription must be the published version's, got {:?}",
        snapshot.json["policyDescription"]
    );
    assert_eq!(
        &snapshot.json["policyConfiguration"], frozen_configuration,
        "case {case}: policyConfiguration must be the published version's, got {:?}",
        snapshot.json["policyConfiguration"]
    );
    assert_eq!(
        snapshot.json["associatedTemplateAndRuleList"]
            .as_array()
            .map(Vec::len),
        Some(frozen_associations),
        "case {case}: associatedTemplateAndRuleList must be the published version's, got {:?}",
        snapshot.json["associatedTemplateAndRuleList"]
    );
    assert!(
        is_update_token(&snapshot.string_at("updateToken")),
        "case {case}: unexpected updateToken, got {:?}",
        snapshot.json["updateToken"]
    );
    assert!(
        snapshot.json["updatedAt"].is_string(),
        "case {case}: updatedAt is the modelled date-time the snapshot was created at, got {:?}",
        snapshot.json["updatedAt"]
    );
}

/// RULE: the snapshot freezes the published version's body and reports the live policy's
/// `priority`, both in the response that creates it and in the read of the
/// version-qualified identifier it answers with — while a draft update moves the live
/// policy on without changing what is published.
#[tokio::test]
async fn a_snapshot_freezes_the_published_body_and_tracks_the_live_priority() {
    let emulator = Emulator::new();

    for case in snapshot_cases() {
        let label = case.label;
        let created = emulator.create_policy(case.create_body()).await;
        assert_eq!(
            created.status, 201,
            "case {label}: CreatePolicy expected 201, got {} {:?}",
            created.status, created.json
        );
        let policy_id = created.string_at("policyId");
        let policy_arn = created.string_at("policyArn");

        // The snapshot of the policy as published: version 1, with the body the create
        // published and the priority the create set.
        let snapshot = emulator
            .create_snapshot(&policy_arn, serde_json::json!({}))
            .await;
        assert_eq!(
            snapshot.status, 201,
            "case {label}: CreatePolicySnapshot expected 201, got {} {:?}",
            snapshot.status, snapshot.json
        );
        assert_snapshot(
            &format!("{label}, the snapshot of version 1"),
            &snapshot,
            &policy_id,
            &policy_arn,
            label,
            case.firewall_type,
            1,
            case.priority,
            case.description,
            &case.configuration,
            case.associations.len(),
        );
        let snapshot_arn = snapshot.string_at("policyArn");

        // A DRAFT update moves the live policy on: a different priority, a different
        // configuration and a different association list, at version 2.
        let updated = emulator
            .update_policy(
                &policy_arn,
                case.update_body(&created.string_at("updateToken"), false),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label}: the draft UpdatePolicy expected 200, got {} {:?}",
            updated.status, updated.json
        );
        assert_eq!(
            updated.json["status"].as_str(),
            Some("DRAFT"),
            "case {label}: the draft update leaves the policy unpublished, got {:?}",
            updated.json["status"]
        );

        // Read back through the identifier the snapshot answered with: the body is still
        // the published version's, and the priority is the live policy's new one.
        let read = emulator.get_policy(&snapshot_arn).await;
        assert_eq!(
            read.status, 200,
            "case {label}: GetPolicy of the snapshot expected 200, got {} {:?}",
            read.status, read.json
        );
        assert_snapshot(
            &format!("{label}, the snapshot read back after a draft update"),
            &read,
            &policy_id,
            &policy_arn,
            label,
            case.firewall_type,
            1,
            case.next_priority,
            case.description,
            &case.configuration,
            case.associations.len(),
        );

        // A second snapshot while the published version is unchanged is that same
        // immutable copy, so it is still the published body with the live priority.
        let again = emulator
            .create_snapshot(&policy_arn, serde_json::json!({}))
            .await;
        assert_eq!(
            again.status, 201,
            "case {label}: the second CreatePolicySnapshot expected 201, got {} {:?}",
            again.status, again.json
        );
        assert_snapshot(
            &format!("{label}, the snapshot taken again of the same published version"),
            &again,
            &policy_id,
            &policy_arn,
            label,
            case.firewall_type,
            1,
            case.next_priority,
            case.description,
            &case.configuration,
            case.associations.len(),
        );
        assert_eq!(
            again.json["updateToken"], read.json["updateToken"],
            "case {label}: an immutable copy is not minted twice, got {:?}",
            again.json["updateToken"]
        );

        // Publishing the live policy publishes a NEW version, which a snapshot taken now
        // copies — while the snapshot of version 1 stays exactly as it was.
        let published = emulator
            .update_policy(
                &policy_arn,
                case.update_body(&updated.string_at("updateToken"), true),
            )
            .await;
        assert_eq!(
            published.status, 200,
            "case {label}: the publishing UpdatePolicy expected 200, got {} {:?}",
            published.status, published.json
        );
        let third_version: u32 = published
            .string_at("version")
            .parse()
            .expect("EntityVersion is a decimal number");
        let fresh = emulator
            .create_snapshot(&policy_arn, serde_json::json!({}))
            .await;
        assert_eq!(
            fresh.status, 201,
            "case {label}: the snapshot of the new published version expected 201, got {} {:?}",
            fresh.status, fresh.json
        );
        assert_snapshot(
            &format!("{label}, the snapshot of the newly published version"),
            &fresh,
            &policy_id,
            &policy_arn,
            label,
            case.firewall_type,
            third_version,
            case.next_priority,
            case.description,
            &case.next_configuration,
            case.next_associations.len(),
        );
        let older = emulator.get_policy(&snapshot_arn).await;
        assert_eq!(
            older.status, 200,
            "case {label}: the older snapshot must still read back, got {} {:?}",
            older.status, older.json
        );
        assert_eq!(
            &older.json["policyConfiguration"], &case.configuration,
            "case {label}: the older snapshot is immutable, got {:?}",
            older.json["policyConfiguration"]
        );

        // The live policy itself was never changed by any of the snapshots: it is still
        // the draft-then-published record the updates left.
        let live = emulator.get_policy(&policy_arn).await;
        assert_eq!(
            live.json["isSnapshot"].as_bool(),
            Some(false),
            "case {label}: the live policy is not a snapshot, got {:?}",
            live.json["isSnapshot"]
        );
        assert_eq!(
            live.json["version"], published.json["version"],
            "case {label}: taking a snapshot must not move the live policy's version, got {:?}",
            live.json["version"]
        );
        assert_eq!(
            &live.json["policyConfiguration"], &case.next_configuration,
            "case {label}: the live policy keeps what the update wrote, got {:?}",
            live.json["policyConfiguration"]
        );
    }
}

/// RULE: this operation copies "the current published version", so a policy that has
/// published none is refused with the modelled `ConflictException` (HTTP 409) and nothing
/// is persisted — and the SAME call succeeds once a version is published.
#[tokio::test]
async fn a_policy_with_no_published_version_cannot_be_snapshotted() {
    let emulator = Emulator::new();

    // The rows differ along firewall type and configuration shape; what they share is
    // that the create published no version.
    let rows: Vec<(&str, &str, serde_json::Value)> = vec![
        ("waf-draft", "WAF", waf_configuration(true, "RETROFIT")),
        (
            "shield-advanced-draft",
            "SHIELD_ADVANCED",
            plain_configuration(false),
        ),
    ];

    for (name, firewall_type, configuration) in rows {
        let mut body = serde_json::json!({
            "policyName": name,
            "priority": 10,
            "firewallType": firewall_type,
            "policyConfiguration": configuration,
            "isPublished": false,
        });
        // "For AWS WAF policies, specify 1 to 100 templates or rules."
        if firewall_type == "WAF" {
            body["associatedTemplateAndRuleList"] = serde_json::json!([{
                "ruleIdentifier": resource_arn("rule", "draftrule"),
            }]);
        }
        let created = emulator.create_policy(body).await;
        assert_eq!(
            created.status, 201,
            "case {name}: CreatePolicy expected 201, got {} {:?}",
            created.status, created.json
        );
        let policy_arn = created.string_at("policyArn");

        let refused = emulator
            .create_snapshot(&policy_arn, serde_json::json!({}))
            .await;
        assert_eq!(
            refused.status, 409,
            "case {name}: expected 409, got {} {:?}",
            refused.status, refused.json
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {name}: unexpected x-amzn-errortype, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.json["resourceType"].as_str(),
            Some("Policy"),
            "case {name}: unexpected resourceType, got {:?}",
            refused.json["resourceType"]
        );
        assert_eq!(
            refused.json["resourceId"].as_str(),
            Some(created.string_at("policyId").as_str()),
            "case {name}: the refusal names the policy, got {:?}",
            refused.json["resourceId"]
        );

        // Nothing was frozen: version 1 of this policy resolves to no snapshot.
        let miss = emulator.get_policy(&format!("{policy_arn}:1")).await;
        assert_eq!(
            miss.status, 404,
            "case {name}: a refused snapshot persists nothing, got {} {:?}",
            miss.status, miss.json
        );

        // Publishing the policy gives the same call something to copy.
        let published = emulator
            .update_policy(
                &policy_arn,
                serde_json::json!({
                    "updateToken": created.string_at("updateToken"),
                    "isPublished": true,
                }),
            )
            .await;
        assert_eq!(
            published.status, 200,
            "case {name}: UpdatePolicy expected 200, got {} {:?}",
            published.status, published.json
        );
        let snapshot = emulator
            .create_snapshot(&policy_arn, serde_json::json!({}))
            .await;
        assert_eq!(
            snapshot.status, 201,
            "case {name}: the snapshot after publishing expected 201, got {} {:?}",
            snapshot.status, snapshot.json
        );
        assert_eq!(
            snapshot.json["version"], published.json["version"],
            "case {name}: the snapshot copies the version just published, got {:?}",
            snapshot.json["version"]
        );
        assert_eq!(
            emulator
                .get_policy(&snapshot.string_at("policyArn"))
                .await
                .status,
            200,
            "case {name}: the snapshot it froze must read back"
        );
    }
}

/// RULE: an identifier no stored policy answers to is refused with the modelled
/// `ResourceNotFoundException` — HTTP 404 and that error code — whatever form it takes,
/// and a policy's snapshots are removed with the policy.
#[tokio::test]
async fn an_unknown_identifier_is_refused_and_snapshots_go_with_their_policy() {
    let emulator = Emulator::new();

    // The rows differ along the FORM of the identifier: an ARN nothing minted, a bare id
    // nothing minted, an ARN of another resource type, a version-qualified ARN of a policy
    // nothing minted, and the one-character minimum of `PolicyIdentifier`'s
    // `@length(1..1010)`.
    let rows: Vec<(&str, String)> = vec![
        (
            "a policy ARN that was never minted",
            resource_arn("policy", "nevermintedpolicy1"),
        ),
        ("a bare id that was never minted", "nosuchpolicy1".to_string()),
        (
            "an ARN of another resource type",
            resource_arn("rule", "notapolicy1"),
        ),
        (
            "a version-qualified ARN of a policy that was never minted",
            format!("{}:3", resource_arn("policy", "nevermintedpolicy2")),
        ),
        ("the shortest identifier the model admits", "x".to_string()),
    ];

    for (name, identifier) in rows {
        let reply = emulator
            .create_snapshot(&identifier, serde_json::json!({}))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {name:?}: expected 404, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.json["resourceType"].as_str(),
            Some("Policy"),
            "case {name:?}: unexpected resourceType, got {:?}",
            reply.json["resourceType"]
        );
    }

    let created = emulator
        .create_policy(serde_json::json!({
            "policyName": "snapshotted-then-deleted",
            "priority": 3,
            "firewallType": "WAF",
            "policyConfiguration": waf_configuration(true, "NO_REMEDIATION"),
            "isPublished": true,
            "associatedTemplateAndRuleList": [{
                "ruleIdentifier": resource_arn("rule", "deletedrule"),
            }],
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "case the policy to delete: CreatePolicy expected 201, got {} {:?}",
        created.status, created.json
    );
    let policy_arn = created.string_at("policyArn");
    let snapshot_arn = emulator
        .create_snapshot(&policy_arn, serde_json::json!({}))
        .await
        .string_at("policyArn");
    assert_eq!(
        emulator.delete_policy(&policy_arn).await.status,
        204,
        "case the policy to delete: DeletePolicy expected 204"
    );
    for (name, identifier) in [
        ("the deleted policy", policy_arn),
        ("its snapshot", snapshot_arn),
    ] {
        let reply = emulator.get_policy(&identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {name}: a deleted policy takes its snapshots with it, got {} {:?}",
            reply.status, reply.json
        );
    }
}

/// RULE: `clientToken` is `@idempotencyToken` — a retry with the same token answers with
/// the first call's snapshot and freezes nothing a second time, while a request with no
/// token, or with another token, is answered from the stored snapshot of that published
/// version all the same.
#[tokio::test]
async fn a_retried_client_token_answers_with_the_first_snapshot() {
    let emulator = Emulator::new();
    let created = emulator
        .create_policy(serde_json::json!({
            "policyName": "idempotent-snapshots",
            "priority": 4,
            "firewallType": "WAF",
            "policyConfiguration": waf_configuration(true, "NO_REMEDIATION"),
            "isPublished": true,
            "associatedTemplateAndRuleList": [{
                "ruleIdentifier": resource_arn("rule", "idempotentrule"),
            }],
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "case the policy: CreatePolicy expected 201, got {} {:?}",
        created.status, created.json
    );
    let policy_arn = created.string_at("policyArn");

    let first = emulator
        .create_snapshot(
            &policy_arn,
            serde_json::json!({ "clientToken": "token-one", "tags": { "take": "first" } }),
        )
        .await;
    assert_eq!(
        first.status, 201,
        "case the first snapshot: expected 201, got {} {:?}",
        first.status, first.json
    );

    // The rows differ along what the retry carries: the same token, no token at all, and a
    // different token. Each answers with the one stored copy of version 1.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "the same clientToken",
            serde_json::json!({ "clientToken": "token-one" }),
        ),
        ("no clientToken", serde_json::json!({})),
        (
            "another clientToken",
            serde_json::json!({ "clientToken": "token-two" }),
        ),
    ];
    for (name, body) in rows {
        let retry = emulator.create_snapshot(&policy_arn, body).await;
        assert_eq!(
            retry.status, 201,
            "case {name}: expected 201, got {} {:?}",
            retry.status, retry.json
        );
        for member in ["policyArn", "version", "updateToken", "updatedAt"] {
            assert_eq!(
                retry.json[member], first.json[member],
                "case {name}: {member} must be the first snapshot's, got {:?}",
                retry.json[member]
            );
        }
    }
}
