// SPDX-License-Identifier: CC0-1.0

//! The Web Application Firewall of a Lightsail CDN distribution, and the
//! Lightsail subdomain a distribution is created reachable at, driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test: WAF is available only on the distribution plans that
//! offer it, under the rule groups that plan supports, and `wafRules` can be
//! specified only while WAF is enabled — so a stored distribution reports the
//! WAF configuration its create and its updates left behind; and a
//! `subdomainName` a create carries makes the subdomain and associates it with
//! the distribution, which the distribution then reports as the subdomain it is
//! reachable at.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    create_certificate, distribution, wait_for_distribution_status, wait_for_issued_certificate,
};
use crate::instance_lifecycle::{error_code, Emulator};

/// The one Region a distribution lives in and its API is served from.
const REGION: &str = "us-east-1";

/// The distribution plans WAF is and is not available on, from the modelled
/// `enableWaf` documentation: "WAF is available only for distributions that use
/// the `medium_1_0` or `large_1_0` bundle."
const SMALL_BUNDLE: &str = "small_1_0";
const MEDIUM_BUNDLE: &str = "medium_1_0";
const LARGE_BUNDLE: &str = "large_1_0";

/// Rule groups of the set the smaller WAF plan supports, from the modelled
/// `wafRules` documentation.
const COMMON_RULE_SET: &str = "AWSManagedRulesCommonRuleSet";
const ANONYMOUS_IP_LIST: &str = "AWSManagedRulesAnonymousIpList";
/// A rule group only the larger WAF plan supports, from the same documentation.
const SQLI_RULE_SET: &str = "AWSManagedRulesSQLiRuleSet";
/// The rule group whose own configuration is "required when the `name` is
/// `AWSManagedRulesATPRuleSet`" and may be defined only then.
const ATP_RULE_SET: &str = "AWSManagedRulesATPRuleSet";

/// The `status` a distribution settles into once its configuration is rolled
/// out.
const DEPLOYED: &str = "Deployed";

/// The `LightsailDistributionSubdomainStateEnum` values this suite asserts.
const SUBDOMAIN_CREATING: &str = "Creating";
const SUBDOMAIN_ATTACHED: &str = "Attached";

/// The `SubdomainState` a subdomain associated with a resource reports.
const SUBDOMAIN_STATE_ATTACHED: &str = "ATTACHED";

/// The origin every distribution in this suite pulls from.
const ORIGIN: &str = "waf-origin";

/// Create a distribution named `name` on `bundle_id` with the extra members
/// `extra` carries, answering the whole response.
async fn create_distribution(
    emulator: &Emulator,
    name: &str,
    bundle_id: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({
        "distributionName": name,
        "origin": {"name": ORIGIN, "regionName": REGION},
        "defaultCacheBehavior": {"behavior": "cache"},
        "bundleId": bundle_id,
    });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    emulator.call("CreateDistribution", body).await
}

/// Poll `GetSubdomains` until the subdomain `name` reports `want`, answering the
/// whole record.
async fn wait_for_subdomain_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, body) = emulator
            .call("GetSubdomains", json!({"subdomainName": name}))
            .await;
        let reported = body["subdomains"]
            .as_array()
            .and_then(|subdomains| subdomains.first())
            .cloned()
            .unwrap_or(Value::Null);
        if reported["state"].as_str() == Some(want) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for subdomain {name}: expected '{want}' within 10s, got {status} \
             {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The WAF configuration a create records
// ---------------------------------------------------------------------------

/// One `CreateDistribution` request's WAF members, and what the rule says the
/// stored distribution then reports.
///
/// The rows differ along everything the documented rules generalise over: the
/// plan (so whether WAF is available at all and which rule groups it offers),
/// whether WAF is asked for, how many rule groups are named, which plan's set
/// they come from, and whether a rule group that takes a configuration of its
/// own carries it.
struct Row {
    case: &'static str,
    bundle_id: &'static str,
    /// The `enableWaf` / `wafRules` members the request carries.
    waf: fn() -> Value,
    /// Whether the stored distribution must report WAF inspecting its viewer
    /// requests.
    enabled: bool,
    /// `Some` with the rule-group names the stored distribution must report, or
    /// `None` when the request must be refused.
    expected: Option<&'static [&'static str]>,
}

const ROWS: &[Row] = &[
    Row {
        case: "no WAF member at all, on a plan that offers WAF",
        bundle_id: MEDIUM_BUNDLE,
        waf: || json!({}),
        enabled: false,
        expected: Some(&[]),
    },
    Row {
        case: "WAF enabled with no rule group",
        bundle_id: MEDIUM_BUNDLE,
        waf: || json!({"enableWaf": true}),
        enabled: true,
        expected: Some(&[]),
    },
    Row {
        case: "WAF enabled with two rule groups of the medium set",
        bundle_id: MEDIUM_BUNDLE,
        waf: || json!({"enableWaf": true, "wafRules": [{"name": COMMON_RULE_SET}, {"name": ANONYMOUS_IP_LIST}]}),
        enabled: true,
        expected: Some(&[COMMON_RULE_SET, ANONYMOUS_IP_LIST]),
    },
    Row {
        case: "WAF enabled with a rule group only the large plan offers",
        bundle_id: LARGE_BUNDLE,
        waf: || json!({"enableWaf": true, "wafRules": [{"name": SQLI_RULE_SET}]}),
        enabled: true,
        expected: Some(&[SQLI_RULE_SET]),
    },
    Row {
        case: "the account takeover rule group with its own configuration",
        bundle_id: LARGE_BUNDLE,
        waf: || {
            json!({
                "enableWaf": true,
                "wafRules": [{
                    "name": ATP_RULE_SET,
                    "atpConfig": {"loginPath": "/login", "payloadType": "JSON"},
                }],
            })
        },
        enabled: true,
        expected: Some(&[ATP_RULE_SET]),
    },
    Row {
        case: "WAF on a plan it is not available for",
        bundle_id: SMALL_BUNDLE,
        waf: || json!({"enableWaf": true}),
        enabled: false,
        expected: None,
    },
    Row {
        case: "a rule group the plan does not offer",
        bundle_id: MEDIUM_BUNDLE,
        waf: || json!({"enableWaf": true, "wafRules": [{"name": SQLI_RULE_SET}]}),
        enabled: false,
        expected: None,
    },
    Row {
        case: "a rule group name no plan offers",
        bundle_id: LARGE_BUNDLE,
        waf: || json!({"enableWaf": true, "wafRules": [{"name": "AWSManagedRulesInvented"}]}),
        enabled: false,
        expected: None,
    },
    Row {
        case: "rule groups while WAF is not enabled",
        bundle_id: MEDIUM_BUNDLE,
        waf: || json!({"wafRules": [{"name": COMMON_RULE_SET}]}),
        enabled: false,
        expected: None,
    },
    Row {
        case: "the account takeover rule group without its configuration",
        bundle_id: LARGE_BUNDLE,
        waf: || json!({"enableWaf": true, "wafRules": [{"name": ATP_RULE_SET}]}),
        enabled: false,
        expected: None,
    },
    Row {
        case: "another rule group carrying the account takeover configuration",
        bundle_id: LARGE_BUNDLE,
        waf: || {
            json!({
                "enableWaf": true,
                "wafRules": [{
                    "name": COMMON_RULE_SET,
                    "atpConfig": {"loginPath": "/login", "payloadType": "JSON"},
                }],
            })
        },
        enabled: false,
        expected: None,
    },
    Row {
        case: "the same rule group named twice",
        bundle_id: MEDIUM_BUNDLE,
        waf: || {
            json!({
                "enableWaf": true,
                "wafRules": [{"name": COMMON_RULE_SET}, {"name": COMMON_RULE_SET}],
            })
        },
        enabled: false,
        expected: None,
    },
    Row {
        case: "more rule groups than the plan offers",
        bundle_id: MEDIUM_BUNDLE,
        waf: || {
            json!({
                "enableWaf": true,
                "wafRules": [
                    {"name": COMMON_RULE_SET},
                    {"name": ANONYMOUS_IP_LIST},
                    {"name": "AWSManagedRulesKnownBadInputsRuleSet"},
                    {"name": COMMON_RULE_SET},
                ],
            })
        },
        enabled: false,
        expected: None,
    },
    Row {
        case: "more rule groups than this service offers at all",
        bundle_id: LARGE_BUNDLE,
        waf: || {
            // Past `crate::state::MAX_WAF_RULES_PER_DISTRIBUTION`, the WAF rule
            // groups the published sets hold between them.
            let rules: Vec<Value> = (0..40).map(|_| json!({"name": COMMON_RULE_SET})).collect();
            json!({"enableWaf": true, "wafRules": rules})
        },
        enabled: false,
        expected: None,
    },
];

/// RULE: a distribution reports the WAF configuration its create recorded —
/// enabled only on a plan that offers WAF and under rule groups that plan
/// supports, with rule groups named only while WAF is enabled — and a request
/// outside those rules is refused with the modelled `InvalidInputException` and
/// creates nothing.
#[tokio::test]
async fn a_created_distribution_reports_the_waf_configuration_its_rules_allow() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN, &format!("{REGION}a")).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("waf-cdn-{index}");
        let (status, answered) =
            create_distribution(&emulator, &name, row.bundle_id, (row.waf)()).await;

        let Some(expected) = row.expected else {
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidInputException",
                "case {case}: expected the modelled input refusal, got {answered:?}"
            );
            // A refused create stored nothing.
            let (status, body) = emulator
                .call("GetDistributions", json!({"distributionName": name}))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "NotFoundException".to_string()),
                "case {case}: a refused create stores no distribution, got {body:?}"
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {answered:?}"
        );
        // The create's own answer, and the stored record a later read reports,
        // carry the same configuration.
        for (source, reported) in [
            ("the create", answered["distribution"].clone()),
            ("a later read", distribution(&emulator, &name).await),
        ] {
            let waf = &reported["waf"];
            assert_eq!(
                waf["isWafEnabled"].as_bool(),
                Some(row.enabled),
                "case {case}: {source} reports whether WAF inspects requests, got {reported:?}"
            );
            let names: Vec<&str> = waf["wafRules"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: {source} must report a wafRules array, got {reported:?}")
                })
                .iter()
                .filter_map(|rule| rule["name"].as_str())
                .collect();
            assert_eq!(
                names, expected,
                "case {case}: {source} reports the rule groups the create named, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: an update replaces the WAF configuration exactly as its members say —
/// the array it carries replaces the rule groups, disabling WAF removes them,
/// and a configuration the plan does not allow is refused and changes nothing.
#[tokio::test]
async fn an_update_replaces_the_waf_configuration() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN, &format!("{REGION}a")).await;
    let (status, created) = create_distribution(
        &emulator,
        "waf-update-cdn",
        LARGE_BUNDLE,
        json!({"enableWaf": true, "wafRules": [{"name": COMMON_RULE_SET}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the fixture distribution: expected HTTP 200, got {created:?}"
    );
    wait_for_distribution_status(&emulator, "waf-update-cdn", DEPLOYED).await;

    // Each step of the update rule, applied in order to the one distribution:
    // the array replaces the rule groups; an empty array disables them all;
    // enabling WAF without naming rule groups leaves the ones it has; and
    // disabling WAF removes them.
    for (case, update, enabled, expected) in [
        (
            "the array replaces the rule groups",
            json!({"enableWaf": true, "wafRules": [{"name": SQLI_RULE_SET}, {"name": ANONYMOUS_IP_LIST}]}),
            true,
            vec![SQLI_RULE_SET, ANONYMOUS_IP_LIST],
        ),
        (
            "an update that names no WAF member keeps the configuration",
            json!({"isEnabled": true}),
            true,
            vec![SQLI_RULE_SET, ANONYMOUS_IP_LIST],
        ),
        (
            "an empty array disables every rule group",
            json!({"wafRules": []}),
            true,
            vec![],
        ),
        (
            "enabling WAF again leaves the rule groups it has",
            json!({"enableWaf": true, "wafRules": [{"name": COMMON_RULE_SET}]}),
            true,
            vec![COMMON_RULE_SET],
        ),
        (
            "disabling WAF removes its rule groups",
            json!({"enableWaf": false}),
            false,
            vec![],
        ),
    ] {
        let mut body = update.clone();
        body["distributionName"] = json!("waf-update-cdn");
        let answered = emulator.ok("UpdateDistribution", body).await;
        assert!(
            answered["operation"]["operationType"].as_str() == Some("UpdateDistribution"),
            "case {case}: expected one update log entry, got {answered:?}"
        );
        let reported = distribution(&emulator, "waf-update-cdn").await;
        let names: Vec<&str> = reported["waf"]["wafRules"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a wafRules array, got {reported:?}"))
            .iter()
            .filter_map(|rule| rule["name"].as_str())
            .collect();
        assert_eq!(
            (reported["waf"]["isWafEnabled"].as_bool(), names),
            (Some(enabled), expected.clone()),
            "case {case}: expected the configuration the update asked for, got {reported:?}"
        );
    }

    // A rule group the plan does not offer, and rule groups while WAF is off,
    // are refused and change nothing.
    for (case, update, error) in [
        (
            "a rule group name no plan offers",
            json!({"wafRules": [{"name": "AWSManagedRulesInvented"}], "enableWaf": true}),
            "InvalidInputException",
        ),
        (
            "rule groups while WAF stays disabled",
            json!({"wafRules": [{"name": COMMON_RULE_SET}]}),
            "InvalidInputException",
        ),
    ] {
        let before = distribution(&emulator, "waf-update-cdn").await;
        let mut body = update.clone();
        body["distributionName"] = json!("waf-update-cdn");
        let (status, answered) = emulator.call("UpdateDistribution", body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, error.to_string()),
            "case {case}: expected the modelled refusal, got {status} {answered:?}"
        );
        let after = distribution(&emulator, "waf-update-cdn").await;
        assert_eq!(
            after["waf"], before["waf"],
            "case {case}: a refused update changes nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The subdomain a create asks for
// ---------------------------------------------------------------------------

/// RULE: a `subdomainName` a create carries makes that subdomain for the
/// account and associates it with the distribution, so the distribution reports
/// it being created and then associated, and the subdomain's own record names
/// the distribution it routes traffic to.
#[tokio::test]
async fn a_created_distribution_is_reachable_at_the_subdomain_it_asked_for() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN, &format!("{REGION}a")).await;

    let (status, created) = create_distribution(
        &emulator,
        "subdomain-cdn",
        MEDIUM_BUNDLE,
        json!({"subdomainName": "shopfront"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case creating a distribution with a subdomain: expected HTTP 200, got {created:?}"
    );
    // The create's own answer: the subdomain is being created for it.
    assert_eq!(
        (
            created["distribution"]["subdomain"]["name"].as_str(),
            created["distribution"]["subdomain"]["state"].as_str(),
        ),
        (Some("shopfront"), Some(SUBDOMAIN_CREATING)),
        "case creating a distribution with a subdomain: expected it being created, got {created:?}"
    );

    // The clock creates the subdomain and then associates it, so the
    // distribution ends up reachable at it and the subdomain names the
    // distribution.
    let settled = wait_for_subdomain_state(&emulator, "shopfront", SUBDOMAIN_STATE_ATTACHED).await;
    let distribution_arn = created["distribution"]["arn"]
        .as_str()
        .expect("a created distribution carries an ARN");
    assert_eq!(
        (
            settled["attachedResource"]["arn"].as_str(),
            settled["attachedResource"]["name"].as_str(),
            settled["fqdn"].as_str(),
        ),
        (
            Some(distribution_arn),
            Some("subdomain-cdn"),
            Some("shopfront.amazonlightsail.com"),
        ),
        "case the subdomain of a distribution: expected the distribution on its record, got \
         {settled:?}"
    );
    let reported = distribution(&emulator, "subdomain-cdn").await;
    assert_eq!(
        (
            reported["subdomain"]["name"].as_str(),
            reported["subdomain"]["state"].as_str(),
        ),
        (Some("shopfront"), Some(SUBDOMAIN_ATTACHED)),
        "case the distribution of a subdomain: expected the subdomain it is reachable at, got \
         {reported:?}"
    );

    // A name this account already holds a subdomain under creates neither the
    // subdomain nor the distribution, and a name no Lightsail subdomain can
    // carry is refused the same way.
    for (case, subdomain_name) in [
        ("a subdomain name the account already holds", "shopfront"),
        ("a name no subdomain can carry", "Shop_Front"),
    ] {
        let (status, answered) = create_distribution(
            &emulator,
            "second-subdomain-cdn",
            MEDIUM_BUNDLE,
            json!({"subdomainName": subdomain_name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "InvalidInputException".to_string()),
            "case {case}: expected the modelled input refusal, got {status} {answered:?}"
        );
        let (status, body) = emulator
            .call(
                "GetDistributions",
                json!({"distributionName": "second-subdomain-cdn"}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "NotFoundException".to_string()),
            "case {case}: a refused create stores no distribution, got {body:?}"
        );
    }
}

/// RULE: a distribution that serves a custom domain — one with a Lightsail
/// certificate attached — cannot be associated with a subdomain, and a resource
/// already associated with one cannot take another.
#[tokio::test]
async fn a_distribution_with_a_custom_domain_takes_no_subdomain() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN, &format!("{REGION}a")).await;
    create_certificate(&emulator, "waf-cert", "cdn.example.com", &[]).await;
    wait_for_issued_certificate(&emulator, "waf-cert").await;

    let (status, created) = create_distribution(
        &emulator,
        "custom-domain-cdn",
        MEDIUM_BUNDLE,
        json!({"certificateName": "waf-cert"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the fixture distribution: expected HTTP 200, got {created:?}"
    );
    let arn = created["distribution"]["arn"]
        .as_str()
        .expect("a created distribution carries an ARN")
        .to_string();
    wait_for_distribution_status(&emulator, "custom-domain-cdn", DEPLOYED).await;

    emulator
        .ok("CreateSubdomain", json!({"subdomainName": "custom"}))
        .await;
    // The subdomain has to be ready before it can be associated with anything,
    // so the case tests the custom-domain rule rather than the state guard.
    wait_for_subdomain_state(&emulator, "custom", "UNATTACHED").await;
    let (status, answered) = emulator
        .call(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "custom",
                "action": "ASSOCIATE",
                "targetResourceArn": arn,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered)),
        (400, "OperationFailureException".to_string()),
        "case associating with a custom-domain distribution: expected the modelled operation \
         failure, got {status} {answered:?}"
    );
    let reported = distribution(&emulator, "custom-domain-cdn").await;
    assert!(
        reported["subdomain"].is_null(),
        "case associating with a custom-domain distribution: it reports no subdomain, got \
         {reported:?}"
    );
}
