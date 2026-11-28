// SPDX-License-Identifier: CC0-1.0

//! The firewall-type-specific members a policy's `policyConfiguration` has to carry.
//!
//! RULE under test: the members `WafConfig` declares `@required` — the model says of each
//! "Required for AWS WAF policies", and of the block that holds them "This is populated
//! only for AWS WAF policies" — are refused by the SERVICE and not by the protocol
//! boundary: a policy whose firewall type is WAF, or any policy that submits a `wafConfig`
//! at all, that does not supply one of them is answered HTTP 400 `ValidationException` with
//! `reason` `FIELD_VALIDATION_FAILED`, `message` "<member> is required for <firewallType>
//! policies", and one `fieldList` entry naming the first missing member: its `name` is that
//! member's dotted path from the operation's input and its `message` is "<member> must be
//! specified when firewallType is <firewallType>. Valid values: <the member's target enum,
//! in the order the model declares it>".
//!
//! The rows differ along what the rule generalises over: WHICH required member is missing
//! (either, or both), whether the block is submitted at all, which firewall type the
//! request names, and which operation submits the configuration (`CreatePolicy` and
//! `UpdatePolicy` both take one). A configuration that carries every member, and a
//! non-WAF policy that carries no `wafConfig`, are accepted.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `ExistingCustomerWebACLResolution`'s values in the order the MODEL declares them
/// (`smithy_query(query="shape", shape="ExistingCustomerWebACLResolution")`).
const WEB_ACL_RESOLUTION_DECLARED: &str = "RETROFIT, OVERRIDE_ASSOCIATION, NO_REMEDIATION";

/// `WAFConflictResolutionOptions`' values, declared as above
/// (`smithy_query(query="shape", shape="WAFConflictResolutionOptions")`).
const CONFLICT_RESOLUTION_DECLARED: &str = "MERGE_WHERE_APPLICABLE";

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
}

/// A `CreatePolicy` body carrying `configuration` as its `policyConfiguration`.
///
/// A WAF policy also carries one association, which the model requires of it ("For AWS
/// WAF policies, specify 1 to 100 templates or rules. For AWS Shield Advanced policies,
/// this list must be empty"), so the refusal a row is about is the configuration's.
fn create_body(
    name: &str,
    firewall_type: &str,
    configuration: serde_json::Value,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "policyName": name,
        "priority": 1,
        "firewallType": firewall_type,
        "policyConfiguration": configuration,
    });
    if firewall_type == "WAF" {
        body["associatedTemplateAndRuleList"] = serde_json::json!([{
            "ruleIdentifier": engine::emulator_arn(
                "network-security-manager",
                "us-east-1",
                "rule/r0",
            ),
        }]);
    }
    body
}

/// A `policyConfiguration` whose `wafConfig` carries exactly `waf_config`.
fn configuration(waf_config: Option<serde_json::Value>) -> serde_json::Value {
    flagged_configuration(true, true, waf_config)
}

/// A `policyConfiguration` with the two remediation flags a row varies, carrying exactly
/// `waf_config`.
fn flagged_configuration(
    remediation: bool,
    clean_up: bool,
    waf_config: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "remediationEnabled": remediation,
        "resourcesCleanUp": clean_up,
    });
    if let Some(waf_config) = waf_config {
        value["wafConfig"] = waf_config;
    }
    value
}

/// A complete `wafConfig`, so a row about another member is not refused for this one.
fn complete_waf_config() -> serde_json::Value {
    serde_json::json!({
        "existingCustomerWebACLResolution": "RETROFIT",
        "conflictResolution": "MERGE_WHERE_APPLICABLE",
    })
}

/// The `fieldList` entry the rule states for one omitted member.
fn field(member: &str, firewall_type: &str, valid_values: &str) -> serde_json::Value {
    serde_json::json!({
        "name": format!("policyConfiguration.wafConfig.{member}"),
        "message": format!(
            "{member} must be specified when firewallType is {firewall_type}. \
             Valid values: {valid_values}"
        ),
    })
}

/// Assert `reply` is the refusal the rule states for `missing`, the members the request did
/// not supply, in the order the model declares them.
fn assert_refused(label: &str, reply: &Reply, firewall_type: &str, missing: &[(&str, &str)]) {
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
    let (first, valid_values) = missing
        .first()
        .unwrap_or_else(|| panic!("case {label}: a refusal names at least one member"));
    assert_eq!(
        reply.json["message"].as_str(),
        Some(format!("{first} is required for {firewall_type} policies").as_str()),
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
        serde_json::json!([field(first, firewall_type, valid_values)]),
        "case {label}: unexpected fieldList, got {:?}",
        reply.json["fieldList"]
    );
}

/// RULE: `CreatePolicy` refuses a configuration that does not carry a member the policy's
/// firewall type requires, and stores nothing.
#[tokio::test]
async fn a_create_whose_waf_config_omits_a_required_member_is_refused() {
    let emulator = Emulator::new();

    /// One row: the case, the firewall type the request names, the `wafConfig` it sends
    /// (`None` sends no block at all), and the members the rule reports as missing.
    struct Row {
        label: &'static str,
        firewall_type: &'static str,
        waf_config: Option<serde_json::Value>,
        missing: Vec<(&'static str, &'static str)>,
    }

    let rows = vec![
        Row {
            label: "a WAF policy with no wafConfig at all, the recorded case",
            firewall_type: "WAF",
            waf_config: None,
            missing: vec![(
                "existingCustomerWebACLResolution",
                WEB_ACL_RESOLUTION_DECLARED,
            )],
        },
        Row {
            label: "existingCustomerWebACLResolution omitted from a submitted block",
            firewall_type: "WAF",
            waf_config: Some(
                serde_json::json!({ "conflictResolution": "MERGE_WHERE_APPLICABLE" }),
            ),
            missing: vec![(
                "existingCustomerWebACLResolution",
                WEB_ACL_RESOLUTION_DECLARED,
            )],
        },
        Row {
            label: "the other required member omitted",
            firewall_type: "WAF",
            waf_config: Some(
                serde_json::json!({ "existingCustomerWebACLResolution": "RETROFIT" }),
            ),
            missing: vec![("conflictResolution", CONFLICT_RESOLUTION_DECLARED)],
        },
        Row {
            label: "an empty block, so both required members are missing",
            firewall_type: "WAF",
            waf_config: Some(serde_json::json!({})),
            missing: vec![
                (
                    "existingCustomerWebACLResolution",
                    WEB_ACL_RESOLUTION_DECLARED,
                ),
                ("conflictResolution", CONFLICT_RESOLUTION_DECLARED),
            ],
        },
        Row {
            // The firewall type the message names is the one the policy is for, so the
            // other `PolicyFirewallType` value states itself.
            label: "a shield-advanced policy submitting an incomplete wafConfig",
            firewall_type: "SHIELD_ADVANCED",
            waf_config: Some(
                serde_json::json!({ "conflictResolution": "MERGE_WHERE_APPLICABLE" }),
            ),
            missing: vec![(
                "existingCustomerWebACLResolution",
                WEB_ACL_RESOLUTION_DECLARED,
            )],
        },
    ];

    for row in rows {
        let label = row.label;
        let reply = emulator
            .call(
                "POST",
                "/policies",
                Some(create_body(
                    "waf-config-probe",
                    row.firewall_type,
                    configuration(row.waf_config.clone()),
                )),
            )
            .await;
        assert_refused(label, &reply, row.firewall_type, &row.missing);

        // The refusal happens BEFORE any state mutation: nothing was stored, so the name
        // is still free and a later list reports no policy.
        let listed = emulator.call("GET", "/policies", None).await;
        assert_eq!(
            listed.status, 200,
            "case {label}: listing expected 200, got {} {:?}",
            listed.status, listed.json
        );
        assert_eq!(
            listed.json["policies"],
            serde_json::json!([]),
            "case {label}: a refused create stores nothing, got {:?}",
            listed.json["policies"]
        );
    }
}

/// RULE: a configuration that carries every member `wafConfig` requires is accepted and
/// echoed back, and so is a non-WAF policy that carries no `wafConfig` — the model says the
/// block "is populated only for AWS WAF policies".
#[tokio::test]
async fn a_create_whose_waf_config_is_complete_is_accepted() {
    let emulator = Emulator::new();

    // The rows differ along the configuration submitted: a complete `wafConfig` on two of
    // the enum's resolutions, and no `wafConfig` at all on the firewall type that needs
    // none.
    let rows: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a complete wafConfig",
            "WAF",
            configuration(Some(serde_json::json!({
                "existingCustomerWebACLResolution": "RETROFIT",
                "conflictResolution": "MERGE_WHERE_APPLICABLE",
            }))),
        ),
        (
            "a complete wafConfig naming another resolution",
            "WAF",
            configuration(Some(serde_json::json!({
                "existingCustomerWebACLResolution": "NO_REMEDIATION",
                "conflictResolution": "MERGE_WHERE_APPLICABLE",
            }))),
        ),
        ("no wafConfig at all", "SHIELD_ADVANCED", configuration(None)),
    ];

    for (index, (label, firewall_type, submitted)) in rows.into_iter().enumerate() {
        let name = format!("waf-config-accepted-{index}");
        let reply = emulator
            .call(
                "POST",
                "/policies",
                Some(create_body(&name, firewall_type, submitted.clone())),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case {label}: expected 201, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.json["policyConfiguration"], submitted,
            "case {label}: the answer must echo the submitted policyConfiguration, got {:?}",
            reply.json["policyConfiguration"]
        );
    }
}

/// RULE: a policy configuration that turns resource cleanup off is refused — on either
/// firewall type, with or without remediation, and on both operations that submit one.
#[tokio::test]
async fn a_configuration_that_disables_resource_cleanup_is_refused() {
    let emulator = Emulator::new();

    /// The refusal the rule states for `resourcesCleanUp`: no condition, because the
    /// requirement holds for every configuration.
    fn assert_clean_up_refused(label: &str, reply: &Reply) {
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
            Some("resourcesCleanUp must be true"),
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
                "name": "policyConfiguration.resourcesCleanUp",
                "message": "resourcesCleanUp must be true.",
            }]),
            "case {label}: unexpected fieldList, got {:?}",
            reply.json["fieldList"]
        );
    }

    // The rows differ along the firewall type, whether remediation is enabled, and whether
    // the `wafConfig` block is carried at all.
    struct Row {
        label: &'static str,
        firewall_type: &'static str,
        configuration: serde_json::Value,
        accepted: bool,
    }
    let rows = vec![
        Row {
            label: "a WAF policy with cleanup off and remediation off, the recorded case",
            firewall_type: "WAF",
            configuration: flagged_configuration(false, false, Some(complete_waf_config())),
            accepted: false,
        },
        Row {
            label: "a WAF policy with cleanup off and remediation on",
            firewall_type: "WAF",
            configuration: flagged_configuration(true, false, Some(complete_waf_config())),
            accepted: false,
        },
        Row {
            label: "a shield-advanced policy with cleanup off",
            firewall_type: "SHIELD_ADVANCED",
            configuration: flagged_configuration(true, false, None),
            accepted: false,
        },
        Row {
            // Both flags are `@required`, so a configuration that OMITS one is refused by
            // the boundary (a different class); this row sends both and turns cleanup off.
            label: "a shield-advanced policy with cleanup off and remediation off",
            firewall_type: "SHIELD_ADVANCED",
            configuration: flagged_configuration(false, false, None),
            accepted: false,
        },
        Row {
            label: "cleanup on and remediation off is accepted",
            firewall_type: "SHIELD_ADVANCED",
            configuration: flagged_configuration(false, true, None),
            accepted: true,
        },
        Row {
            label: "cleanup on and remediation on is accepted",
            firewall_type: "WAF",
            configuration: flagged_configuration(true, true, Some(complete_waf_config())),
            accepted: true,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let label = row.label;
        let name = format!("clean-up-probe-{index}");
        let reply = emulator
            .call(
                "POST",
                "/policies",
                Some(create_body(&name, row.firewall_type, row.configuration)),
            )
            .await;
        if row.accepted {
            assert_eq!(
                reply.status, 201,
                "case {label}: expected 201, got {} {:?}",
                reply.status, reply.json
            );
        } else {
            assert_clean_up_refused(label, &reply);
        }
    }

    // The same rule on `UpdatePolicy`, which submits the same shape: the stored policy is
    // left alone.
    let created = emulator
        .call(
            "POST",
            "/policies",
            Some(create_body(
                "clean-up-update",
                "WAF",
                configuration(Some(complete_waf_config())),
            )),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the create expected 201, got {} {:?}",
        created.status, created.json
    );
    let policy_id = created.json["policyId"]
        .as_str()
        .expect("the create answers a policyId")
        .to_string();
    let reply = emulator
        .call(
            "PATCH",
            &format!("/policies/{policy_id}"),
            Some(serde_json::json!({
                "updateToken": created.json["updateToken"],
                "isPublished": true,
                "policyConfiguration": flagged_configuration(
                    true,
                    false,
                    Some(complete_waf_config()),
                ),
            })),
        )
        .await;
    assert_clean_up_refused("an update disabling cleanup", &reply);
    let read = emulator
        .call("GET", &format!("/policies/{policy_id}"), None)
        .await;
    assert_eq!(
        read.json["policyConfiguration"]["resourcesCleanUp"].as_bool(),
        Some(true),
        "a refused update must leave the stored configuration alone, got {:?}",
        read.json["policyConfiguration"]
    );
}

/// RULE: the same refusal holds for `UpdatePolicy`, which takes the same configuration —
/// and the update is refused before the stored policy changes.
#[tokio::test]
async fn an_update_whose_waf_config_omits_a_required_member_is_refused() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "POST",
            "/policies",
            Some(create_body(
                "waf-config-update",
                "WAF",
                configuration(Some(serde_json::json!({
                    "existingCustomerWebACLResolution": "RETROFIT",
                    "conflictResolution": "MERGE_WHERE_APPLICABLE",
                }))),
            )),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the create expected 201, got {} {:?}",
        created.status, created.json
    );
    let policy_id = created.json["policyId"]
        .as_str()
        .expect("the create answers a policyId")
        .to_string();
    let update_token = created.json["updateToken"]
        .as_str()
        .expect("the create answers an updateToken")
        .to_string();

    /// One row: the case, the `wafConfig` the update submits (`None` submits a
    /// configuration carrying no block), and the members the rule reports as missing.
    struct UpdateRow {
        label: &'static str,
        waf_config: Option<serde_json::Value>,
        missing: Vec<(&'static str, &'static str)>,
    }

    // The rows differ along which required member the update leaves out.
    let rows = vec![
        UpdateRow {
            label: "an update omitting existingCustomerWebACLResolution",
            waf_config: Some(
                serde_json::json!({ "conflictResolution": "MERGE_WHERE_APPLICABLE" }),
            ),
            missing: vec![(
                "existingCustomerWebACLResolution",
                WEB_ACL_RESOLUTION_DECLARED,
            )],
        },
        UpdateRow {
            label: "an update omitting conflictResolution",
            waf_config: Some(serde_json::json!({
                "existingCustomerWebACLResolution": "OVERRIDE_ASSOCIATION",
            })),
            missing: vec![("conflictResolution", CONFLICT_RESOLUTION_DECLARED)],
        },
        UpdateRow {
            // An update REPLACES the configuration, so dropping the block is dropping the
            // members.
            label: "an update replacing the configuration with one carrying no wafConfig",
            waf_config: None,
            missing: vec![(
                "existingCustomerWebACLResolution",
                WEB_ACL_RESOLUTION_DECLARED,
            )],
        },
    ];

    for row in rows {
        let label = row.label;
        let reply = emulator
            .call(
                "PATCH",
                &format!("/policies/{policy_id}"),
                Some(serde_json::json!({
                    "updateToken": update_token,
                    "isPublished": true,
                    "policyConfiguration": configuration(row.waf_config),
                })),
            )
            .await;
        // The firewall type an update's refusal names comes from the STORED policy: no
        // modelled member of `UpdatePolicy` carries one.
        assert_refused(label, &reply, "WAF", &row.missing);

        // Refused before any mutation: the policy still reads at version 1 with the
        // configuration the create stored, and the token the caller holds still matches.
        let read = emulator
            .call("GET", &format!("/policies/{policy_id}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {label}: the read expected 200, got {} {:?}",
            read.status, read.json
        );
        assert_eq!(
            read.json["version"].as_str(),
            Some("1"),
            "case {label}: a refused update must not bump the version, got {:?}",
            read.json["version"]
        );
        assert_eq!(
            read.json["updateToken"].as_str(),
            Some(update_token.as_str()),
            "case {label}: a refused update must not rotate the updateToken"
        );
        assert_eq!(
            read.json["policyConfiguration"]["wafConfig"],
            serde_json::json!({
                "existingCustomerWebACLResolution": "RETROFIT",
                "conflictResolution": "MERGE_WHERE_APPLICABLE",
            }),
            "case {label}: a refused update must not change the stored configuration"
        );
    }
}
