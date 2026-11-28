// SPDX-License-Identifier: CC0-1.0

//! The association list a policy's firewall type prescribes, and the sentence a refused
//! member's `fieldList` entry states.
//!
//! RULE under test: a documented prose rule about `associatedTemplateAndRuleList` that no
//! constraint trait expresses is enforced by the SERVICE — the model says of the member
//! "For AWS WAF policies, specify 1 to 100 templates or rules, of which at most 2 can be
//! templates. For AWS Shield Advanced policies, this list must be empty" — so a policy
//! whose list falls outside what its `firewallType` prescribes is answered HTTP 400
//! `ValidationException` with `reason` `FIELD_VALIDATION_FAILED` and one `fieldList` entry
//! naming the member, and nothing is stored. "1 to 100" is a floor as much as a ceiling:
//! an AWS WAF policy that associates nothing (an absent list or an empty one) is refused
//! with `message` "<member> is required for <firewallType> policies", exactly as an omitted
//! member of `wafConfig` is.
//!
//! RULE under test, second: a `ValidationExceptionField`'s `message` — documented "A
//! message describing the validation error for the field" — is that member's own complete
//! sentence, terminated like one and stated independently of the top-level `message`
//! rather than cloned from it.
//!
//! The rows differ along what the rules generalise over: which firewall type the request
//! names (every `PolicyFirewallType` value), how the empty list is expressed (absent or
//! explicit), how many entries the list carries and of which kind, which clause the list
//! violates, and which operation submits it (`CreatePolicy` and `UpdatePolicy` both take
//! one).

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The member every refusal below names, as the model spells it.
const LIST_MEMBER: &str = "associatedTemplateAndRuleList";

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

    async fn create(&self, body: serde_json::Value) -> Reply {
        self.call("POST", "/policies", Some(body)).await
    }

    async fn update(&self, policy_id: &str, body: serde_json::Value) -> Reply {
        self.call("PATCH", &format!("/policies/{policy_id}"), Some(body))
            .await
    }

    async fn get(&self, policy_id: &str) -> Reply {
        self.call("GET", &format!("/policies/{policy_id}"), None)
            .await
    }

    /// Every stored `policyId`, so a refusal can be shown to have stored nothing.
    async fn listed_ids(&self) -> Vec<String> {
        let reply = self.call("GET", "/policies", None).await;
        assert_eq!(
            reply.status, 200,
            "ListPolicies expected 200, got {} {:?}",
            reply.status, reply.json
        );
        reply.json["policies"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["policyId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// An ARN of one of this service's other resource types, built from the same helper the
/// handlers mint ARNs with rather than from a hardcoded literal.
fn reference_arn(resource_type: &str, id: &str) -> String {
    engine::emulator_arn(
        "network-security-manager",
        "us-east-1",
        &format!("{resource_type}/{id}"),
    )
}

/// A rule reference for the association list.
fn rule_reference(id: &str) -> serde_json::Value {
    serde_json::json!({ "ruleIdentifier": reference_arn("rule", id) })
}

/// A template reference for the association list.
fn template_reference(id: &str) -> serde_json::Value {
    serde_json::json!({ "templateIdentifier": reference_arn("template", id) })
}

/// A `policyConfiguration` complete for `firewall_type`: `WafConfig`'s two members are
/// `@required` of an AWS WAF policy, and the block "is populated only for AWS WAF
/// policies".
fn configuration(firewall_type: &str) -> serde_json::Value {
    let mut value = serde_json::json!({
        "remediationEnabled": true,
        "resourcesCleanUp": true,
    });
    if firewall_type == "WAF" {
        value["wafConfig"] = serde_json::json!({
            "existingCustomerWebACLResolution": "RETROFIT",
            "conflictResolution": "MERGE_WHERE_APPLICABLE",
        });
    }
    value
}

/// A `CreatePolicy` body for `firewall_type` carrying exactly `associations` (`None`
/// carries no list member at all).
fn create_body(
    name: &str,
    firewall_type: &str,
    associations: Option<Vec<serde_json::Value>>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "policyName": name,
        "priority": 1,
        "firewallType": firewall_type,
        "policyConfiguration": configuration(firewall_type),
    });
    if let Some(associations) = associations {
        body[LIST_MEMBER] = serde_json::json!(associations);
    }
    body
}

/// Assert `reply` is the modelled `ValidationException` naming the association list, and
/// hand back its top-level `message` and its one `fieldList` entry's `message`.
fn assert_list_refusal(label: &str, reply: &Reply) -> (String, String) {
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
        reply.json["reason"].as_str(),
        Some("FIELD_VALIDATION_FAILED"),
        "case {label}: unexpected reason, got {:?}",
        reply.json["reason"]
    );
    assert_eq!(
        reply.json["fieldList"].as_array().map(Vec::len),
        Some(1),
        "case {label}: a refusal of the list names exactly that member, got {:?}",
        reply.json["fieldList"]
    );
    assert_eq!(
        reply.json["fieldList"][0]["name"].as_str(),
        Some(LIST_MEMBER),
        "case {label}: the refusal must name the member, got {:?}",
        reply.json["fieldList"]
    );
    let message = reply.json["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: message is @required, got {:?}", reply.json))
        .to_string();
    let field_message = reply.json["fieldList"][0]["message"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {label}: the entry's message is @required, got {:?}",
                reply.json["fieldList"]
            )
        })
        .to_string();
    (message, field_message)
}

/// RULE: an AWS WAF policy that associates nothing is refused — the list's documented
/// minimum of one template or rule — and nothing is stored.
#[tokio::test]
async fn a_waf_policy_associating_nothing_is_refused() {
    let emulator = Emulator::new();

    // The rows differ along how "no associations" is expressed and what else the request
    // carries; the recorded case is the first.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "no list member at all, the recorded case",
            create_body("waf-absent-list", "WAF", None),
        ),
        (
            "an explicitly empty list",
            create_body("waf-empty-list", "WAF", Some(Vec::new())),
        ),
        ("a draft policy with an empty list", {
            let mut body = create_body("waf-draft-empty-list", "WAF", Some(Vec::new()));
            body["isPublished"] = serde_json::json!(false);
            body["policyDescription"] = serde_json::json!("a draft that associates nothing");
            body["tags"] = serde_json::json!({ "team": "network" });
            body
        }),
        ("a policy at another priority with an empty list", {
            let mut body = create_body("waf-priority-empty-list", "WAF", Some(Vec::new()));
            body["priority"] = serde_json::json!(2_147_483_647_i64);
            body
        }),
    ];

    for (label, body) in rows {
        let reply = emulator.create(body).await;
        let (message, field_message) = assert_list_refusal(label, &reply);
        assert_eq!(
            message,
            format!("{LIST_MEMBER} is required for WAF policies"),
            "case {label}: unexpected message, got {message:?}"
        );
        assert_eq!(
            field_message,
            format!(
                "{LIST_MEMBER} must contain at least one template or rule when firewallType \
                 is WAF."
            ),
            "case {label}: unexpected fieldList message, got {field_message:?}"
        );

        // The refusal happens before any mutation: nothing is stored, so the name is still
        // free and the listing is still empty.
        let listed = emulator.listed_ids().await;
        assert!(
            listed.is_empty(),
            "case {label}: a refused create must store nothing, got {listed:?}"
        );
    }
}

/// RULE: when a request violates both the configuration's required members and the list's
/// documented minimum, the refusal states the CONFIGURATION member — the order the service
/// reports the two prose rules in.
#[tokio::test]
async fn an_incomplete_configuration_is_reported_before_the_association_list() {
    let emulator = Emulator::new();

    /// A `policyConfiguration` carrying exactly `waf_config` (`None` carries no block).
    fn partial(waf_config: Option<serde_json::Value>) -> serde_json::Value {
        let mut value = serde_json::json!({
            "remediationEnabled": true,
            "resourcesCleanUp": true,
        });
        if let Some(waf_config) = waf_config {
            value["wafConfig"] = waf_config;
        }
        value
    }

    // The rows differ along which member of the configuration is missing and how the
    // (also refusable) empty list is expressed; the first is the recorded case.
    let rows: Vec<(&str, serde_json::Value, Option<Vec<serde_json::Value>>, &str)> = vec![
        (
            "no wafConfig and no list member, the recorded case",
            partial(None),
            None,
            "existingCustomerWebACLResolution",
        ),
        (
            "no wafConfig and an explicitly empty list",
            partial(None),
            Some(Vec::new()),
            "existingCustomerWebACLResolution",
        ),
        (
            "the other required member missing, with an empty list",
            partial(Some(
                serde_json::json!({ "existingCustomerWebACLResolution": "RETROFIT" }),
            )),
            Some(Vec::new()),
            "conflictResolution",
        ),
    ];

    for (index, (label, configuration, associations, reported)) in rows.into_iter().enumerate() {
        let mut body = serde_json::json!({
            "policyName": format!("ordered-{index}"),
            "priority": 1,
            "firewallType": "WAF",
            "policyConfiguration": configuration,
        });
        if let Some(associations) = associations {
            body[LIST_MEMBER] = serde_json::json!(associations);
        }
        let reply = emulator.create(body).await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.json["message"].as_str(),
            Some(format!("{reported} is required for WAF policies").as_str()),
            "case {label}: the configuration member is reported first, got {:?}",
            reply.json["message"]
        );
        assert_eq!(
            reply.json["fieldList"][0]["name"].as_str(),
            Some(format!("policyConfiguration.wafConfig.{reported}").as_str()),
            "case {label}: unexpected fieldList, got {:?}",
            reply.json["fieldList"]
        );
    }
}

/// RULE: the same minimum holds for `UpdatePolicy`, which replaces the list — an update
/// clearing an AWS WAF policy's associations is refused and the stored policy is left
/// alone.
#[tokio::test]
async fn an_update_clearing_a_waf_policys_associations_is_refused() {
    let emulator = Emulator::new();
    let created = emulator
        .create(create_body(
            "waf-update-clear",
            "WAF",
            Some(vec![rule_reference("r1")]),
        ))
        .await;
    assert_eq!(
        created.status, 201,
        "the setup create expected 201, got {} {:?}",
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

    let reply = emulator
        .update(
            &policy_id,
            serde_json::json!({
                "updateToken": update_token,
                "isPublished": true,
                "associatedTemplateAndRuleList": [],
            }),
        )
        .await;
    let (message, field_message) = assert_list_refusal("an update clearing the list", &reply);
    assert_eq!(
        message,
        format!("{LIST_MEMBER} is required for WAF policies"),
        "an update clearing the list: unexpected message, got {message:?}"
    );
    assert_eq!(
        field_message,
        format!(
            "{LIST_MEMBER} must contain at least one template or rule when firewallType is WAF."
        ),
        "an update clearing the list: unexpected fieldList message, got {field_message:?}"
    );

    // Refused before any mutation: the policy still reads at version 1 with the
    // association the create stored, and the token the caller holds still matches.
    let read = emulator.get(&policy_id).await;
    assert_eq!(
        read.status, 200,
        "the read expected 200, got {} {:?}",
        read.status, read.json
    );
    assert_eq!(
        read.json["version"].as_str(),
        Some("1"),
        "a refused update must not bump the version, got {:?}",
        read.json["version"]
    );
    assert_eq!(
        read.json["updateToken"].as_str(),
        Some(update_token.as_str()),
        "a refused update must not rotate the updateToken"
    );
    assert_eq!(
        read.json[LIST_MEMBER],
        serde_json::json!([{ "ruleArn": reference_arn("rule", "r1") }]),
        "a refused update must leave the stored associations alone, got {:?}",
        read.json[LIST_MEMBER]
    );
}

/// RULE: a list the firewall type prescribes is ACCEPTED and echoed back — including the
/// firewall types whose documented list is empty.
#[tokio::test]
async fn a_list_the_firewall_type_prescribes_is_accepted() {
    let emulator = Emulator::new();

    // The rows differ along firewall type (every `PolicyFirewallType` value, from
    // `smithy_query(query="shape", shape="PolicyFirewallType")`: WAF, SHIELD_ADVANCED,
    // IGW_FIREWALL), entry count and entry kind. The ten-entry row is a count the issue
    // does not cite, inside the modelled `@length(0..100)`.
    let rows: Vec<(&str, &str, Vec<serde_json::Value>)> = vec![
        ("WAF with one rule", "WAF", vec![rule_reference("r1")]),
        (
            "WAF with one template",
            "WAF",
            vec![template_reference("t1")],
        ),
        (
            "WAF with the two templates the ceiling allows and a rule",
            "WAF",
            vec![
                template_reference("t1"),
                template_reference("t2"),
                rule_reference("r1"),
            ],
        ),
        (
            "WAF with ten rules",
            "WAF",
            (0..10).map(|index| rule_reference(&format!("r{index}"))).collect(),
        ),
        ("SHIELD_ADVANCED with the empty list the model requires", "SHIELD_ADVANCED", Vec::new()),
        (
            "IGW_FIREWALL, a firewall type the prose states no list rule for",
            "IGW_FIREWALL",
            Vec::new(),
        ),
    ];

    for (index, (label, firewall_type, associations)) in rows.into_iter().enumerate() {
        let name = format!("accepted-{index}");
        let reply = emulator
            .create(create_body(&name, firewall_type, Some(associations.clone())))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {label}: expected 201, got {} {:?}",
            reply.status, reply.json
        );
        // The answer carries the associations under the output member for each reference's
        // kind: `templateArn` for a template, `ruleArn` for a rule.
        let expected: Vec<serde_json::Value> = associations
            .iter()
            .map(|reference| {
                if let Some(identifier) = reference["templateIdentifier"].as_str() {
                    serde_json::json!({ "templateArn": identifier })
                } else {
                    serde_json::json!({ "ruleArn": reference["ruleIdentifier"] })
                }
            })
            .collect();
        assert_eq!(
            reply.json[LIST_MEMBER],
            serde_json::json!(expected),
            "case {label}: unexpected associations, got {:?}",
            reply.json[LIST_MEMBER]
        );
    }
}

/// RULE: every other clause of the same prose is refused the same way, and each refusal's
/// `fieldList` entry states its own complete sentence about the member rather than the
/// top-level `message`.
#[tokio::test]
async fn each_list_refusal_states_its_own_sentence_for_the_field() {
    let emulator = Emulator::new();

    // One row per clause the prose states, differing along which clause is violated and
    // how far outside it the list falls.
    let rows: Vec<(&str, &str, Vec<serde_json::Value>)> = vec![
        (
            "SHIELD_ADVANCED associating a rule",
            "SHIELD_ADVANCED",
            vec![rule_reference("r1")],
        ),
        (
            "SHIELD_ADVANCED associating two entries",
            "SHIELD_ADVANCED",
            vec![rule_reference("r1"), template_reference("t1")],
        ),
        (
            "WAF with one template past the ceiling",
            "WAF",
            vec![
                template_reference("t1"),
                template_reference("t2"),
                template_reference("t3"),
            ],
        ),
        (
            "WAF whose rule reference is not an ARN",
            "WAF",
            vec![serde_json::json!({ "ruleIdentifier": "my-rule" })],
        ),
        (
            "WAF whose template reference is half an ARN",
            "WAF",
            vec![serde_json::json!({ "templateIdentifier": "arn:aws:a:b:c:d" })],
        ),
    ];

    for (index, (label, firewall_type, associations)) in rows.into_iter().enumerate() {
        let name = format!("refused-{index}");
        let reply = emulator
            .create(create_body(&name, firewall_type, Some(associations)))
            .await;
        let (message, field_message) = assert_list_refusal(label, &reply);
        assert!(
            field_message.ends_with('.'),
            "case {label}: the entry's message is a complete sentence, got {field_message:?}"
        );
        assert_ne!(
            field_message, message,
            "case {label}: the entry's message is stated separately from the top-level one, \
             got {field_message:?}"
        );

        let listed = emulator.listed_ids().await;
        assert!(
            listed.is_empty(),
            "case {label}: a refused create must store nothing, got {listed:?}"
        );
    }
}

/// RULE: the same separation holds for the `policyConfiguration` refusal the recordings
/// show it on — the top-level `message` and the per-field sentence differ, and the entry
/// carries the sentence.
#[tokio::test]
async fn the_resources_clean_up_refusal_states_its_own_sentence_for_the_field() {
    let emulator = Emulator::new();

    // The rows differ along the firewall type and the operation submitting the
    // configuration; what they share is `resourcesCleanUp` turned off.
    let created = emulator
        .create(create_body(
            "clean-up-seed",
            "WAF",
            Some(vec![rule_reference("r1")]),
        ))
        .await;
    assert_eq!(
        created.status, 201,
        "the setup create expected 201, got {} {:?}",
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

    let mut waf_create = create_body("clean-up-waf", "WAF", Some(vec![rule_reference("r2")]));
    waf_create["policyConfiguration"]["resourcesCleanUp"] = serde_json::json!(false);
    let mut shield_create = create_body("clean-up-shield", "SHIELD_ADVANCED", Some(Vec::new()));
    shield_create["policyConfiguration"]["resourcesCleanUp"] = serde_json::json!(false);

    let mut update_configuration = configuration("WAF");
    update_configuration["resourcesCleanUp"] = serde_json::json!(false);

    let replies = vec![
        (
            "a WAF create turning cleanup off, the recorded case",
            emulator.create(waf_create).await,
        ),
        (
            "a SHIELD_ADVANCED create turning cleanup off",
            emulator.create(shield_create).await,
        ),
        (
            "an update turning cleanup off",
            emulator
                .update(
                    &policy_id,
                    serde_json::json!({
                        "updateToken": update_token,
                        "isPublished": true,
                        "policyConfiguration": update_configuration,
                    }),
                )
                .await,
        ),
    ];

    for (label, reply) in replies {
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
            reply.json["fieldList"],
            serde_json::json!([{
                "name": "policyConfiguration.resourcesCleanUp",
                "message": "resourcesCleanUp must be true.",
            }]),
            "case {label}: unexpected fieldList, got {:?}",
            reply.json["fieldList"]
        );
    }
}
