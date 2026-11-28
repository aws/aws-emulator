// SPDX-License-Identifier: CC0-1.0

//! A web ACL's CLIENT-APPLICATION integration, end to end over the wire: the
//! `AssociationConfig` a save carries and the `ApplicationIntegrationURL` a read reports.
//!
//! Two rules are asserted here.
//!
//! 1. A web ACL's `AssociationConfig` is state of the web ACL, so every setting a save
//!    carries in it is stored and answered back by `GetWebACL` — including
//!    `ChallengeJavascriptSdkInjection`, which asks WAF to inject the challenge JavaScript
//!    SDK into the responses of the resource types it names (recorded behaviour: the
//!    member is accepted by a save and echoed under
//!    `$.WebACL.AssociationConfig.ChallengeJavascriptSdkInjection` with the resource-type
//!    keys the save supplied; it is declared in this crate's own model from those recorded
//!    responses). A read-modify-write that hands back what it read must therefore keep the
//!    setting rather than strip it.
//! 2. `GetWebACLResponse$ApplicationIntegrationURL` is "the URL to use in SDK integrations
//!    with Amazon Web Services managed rule groups … This is only populated if you are
//!    using a rule group in your web ACL that integrates with your applications in this
//!    way" (`smithy_query(query="doc",shape="GetWebACLResponse",member="ApplicationIntegrationURL")`),
//!    so the read reports the account's integration URL for a web ACL that integrates with
//!    the application it protects — one that has WAF inject the SDK, or one whose rules run
//!    a managed rule group of the intelligent-threat-mitigation kind — and leaves the
//!    member out for one that does not. The URL itself is published at the address this
//!    emulator answers on, because a URL a handler hands a caller has to lead back here.
//!
//! Each rule is asserted over a TABLE spanning the class rather than the one request that
//! exposed it: the association rows differ in WHICH resource types the injection names, in
//! how many, in whether the configuration also carries `RequestBody`, in the empty map that
//! names no resource type at all, and in the web ACL's scope; the integration-URL rows are
//! the WHOLE published managed-rule-group catalogue, one row per group, so the groups no
//! recorded request mentions are covered beside the one it does.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ManagedRuleGroupDescriptor, MANAGED_RULE_GROUP_CATALOGUE,
};

use crate::managed_rule_groups::managed_statement;
use crate::web_acl_lifecycle::{visibility, Waf};

/// How many lowercase hex characters the account's integration id carries — the width the
/// recorded `ApplicationIntegrationURL` publishes it at, spelled out here independently of
/// the service's own constant.
const INTEGRATION_ID_LENGTH: usize = 12;

/// The base address the emulator of [`a_web_acl_integration_url_is_published_at_this_emulators_address`]
/// answers on, which the URL it publishes must be composed from.
const SERVED_BASE: &str = "https://wafv2-integration.invalid";

/// One `AssociationConfig` to save and read back.
struct AssociationRow {
    case: &'static str,
    scope: &'static str,
    /// The `AssociationConfig` the save carries, or `None` to leave the member out.
    association_config: Option<Value>,
    /// Whether the web ACL that carries it integrates with the application, and so whether
    /// the read must report the integration URL.
    integrates: bool,
}

/// The rows: no `AssociationConfig` at all, one carrying only `RequestBody`, injection for
/// one resource type and for several, and injection beside `RequestBody`.
///
/// An injection map that names NO resource type is not among them: the model bounds
/// `ChallengeJavascriptSdkInjection` at `length(min 1)`
/// (`smithy_query(query="shape",shape="ChallengeJavascriptSdkInjection")`), so a save
/// carrying one is refused rather than stored
/// ([`an_injection_map_naming_no_resource_type_is_refused`]).
///
/// The resource-type keys come from `AssociatedResourceType`
/// (`smithy_query(query="shape",shape="AssociatedResourceType")` → `CLOUDFRONT`,
/// `API_GATEWAY`, `COGNITO_USER_POOL`, `APP_RUNNER_SERVICE`, `VERIFIED_ACCESS_INSTANCE`,
/// `AGENTCORE_GATEWAY`), and the `RequestBody` limits from `SizeInspectionLimit`
/// (`smithy_query(query="shape",shape="SizeInspectionLimit")` → `KB_16`, `KB_32`, `KB_48`,
/// `KB_64`).
fn association_rows() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            case: "no AssociationConfig at all",
            scope: "REGIONAL",
            association_config: None,
            integrates: false,
        },
        AssociationRow {
            case: "an AssociationConfig carrying only RequestBody",
            scope: "REGIONAL",
            association_config: Some(json!({
                "RequestBody": { "API_GATEWAY": { "DefaultSizeInspectionLimit": "KB_48" } },
            })),
            integrates: false,
        },
        AssociationRow {
            case: "SDK injection for one resource type, CLOUDFRONT scope",
            scope: "CLOUDFRONT",
            association_config: Some(json!({
                "ChallengeJavascriptSdkInjection": { "CLOUDFRONT": {} },
            })),
            integrates: true,
        },
        AssociationRow {
            case: "SDK injection for several resource types",
            scope: "REGIONAL",
            association_config: Some(json!({
                "ChallengeJavascriptSdkInjection": {
                    "API_GATEWAY": {},
                    "COGNITO_USER_POOL": {},
                    "APP_RUNNER_SERVICE": {},
                },
            })),
            integrates: true,
        },
        AssociationRow {
            case: "SDK injection beside a RequestBody limit",
            scope: "REGIONAL",
            association_config: Some(json!({
                "ChallengeJavascriptSdkInjection": { "VERIFIED_ACCESS_INSTANCE": {} },
                "RequestBody": {
                    "VERIFIED_ACCESS_INSTANCE": { "DefaultSizeInspectionLimit": "KB_64" },
                },
            })),
            integrates: true,
        },
    ]
}

/// A `CreateWebACL` request for `name` in `scope`, carrying `association_config` and
/// `rules`.
fn create_request(
    name: &str,
    scope: &str,
    association_config: Option<&Value>,
    rules: Value,
) -> Value {
    let mut request = json!({
        "Name": name,
        "Scope": scope,
        "DefaultAction": { "Allow": {} },
        "VisibilityConfig": visibility(name),
        "Rules": rules,
    });
    if let Some(association_config) = association_config {
        request["AssociationConfig"] = association_config.clone();
    }
    request
}

/// A rule that RUNS a rule group: such a rule carries `OverrideAction` rather than `Action`
/// ("you can only use this for rule statements that reference a rule group, like
/// `RuleGroupReferenceStatement` and `ManagedRuleGroupStatement`", `OverrideAction`'s own
/// doc).
fn managed_rule(name: &str, statement: Value) -> Value {
    json!({
        "Name": name,
        "Priority": 1,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// The web ACL and its lock token, as `GetWebACL` answers them, together with the
/// `ApplicationIntegrationURL` it reported.
async fn get_web_acl(waf: &Waf, scope: &str, name: &str, id: &str) -> Value {
    waf.ok(
        "GetWebACL",
        json!({ "Name": name, "Scope": scope, "Id": id }),
    )
    .await
}

/// The integration id the URL `url` publishes, asserting the form: the address this
/// emulator answers on, then the account's integration id as the single path segment, then
/// the trailing `/`.
fn asserted_integration_id(url: &str, base: &str, case: &str) -> String {
    let path = url.strip_prefix(base).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the integration URL must be published at the address this \
             emulator answers on ({base}), got {url:?}"
        )
    });
    let id = path
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the path must be the integration id and a trailing slash, \
                 got {path:?} in {url:?}"
            )
        });
    assert!(
        id.chars().all(|digit| matches!(digit, '0'..='9' | 'a'..='f')),
        "case {case:?}: the integration id must be lowercase hex, got {id:?}"
    );
    assert_eq!(
        id.chars().count(),
        INTEGRATION_ID_LENGTH,
        "case {case:?}: the integration id must be {INTEGRATION_ID_LENGTH} characters wide, \
         got {id:?} in {url:?}"
    );
    id.to_string()
}

/// An injection map that names no resource type is refused, and nothing is stored.
///
/// `ChallengeJavascriptSdkInjection` is bounded at `length(min 1)`
/// (`smithy_query(query="shape",shape="ChallengeJavascriptSdkInjection")`) and the member's
/// own doc states what an absent setting means instead — "if you omit this setting,
/// CloudFront does not insert the SDK" — so the empty map is a member-constraint failure,
/// refused in the request validator's own envelope before any web ACL is written.
#[tokio::test]
async fn an_injection_map_naming_no_resource_type_is_refused() {
    let case = "CreateWebACL with an injection map naming no resource type";
    let waf = Waf::new();
    let name = "empty-injection-acl";
    waf.refused(
        "CreateWebACL",
        create_request(
            name,
            "REGIONAL",
            Some(&json!({ "ChallengeJavascriptSdkInjection": {} })),
            json!([]),
        ),
        "ValidationException",
        case,
    )
    .await;
    let listed = waf
        .ok("ListWebACLs", json!({ "Scope": "REGIONAL" }))
        .await;
    assert_eq!(
        listed["WebACLs"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: a refused save must store no web ACL, got {listed}"
    );
}

/// Every `AssociationConfig` a save carries reads back exactly as it was saved, and the
/// integration URL is reported for a web ACL that has WAF inject the SDK and left out for
/// one that does not.
#[tokio::test]
async fn an_association_config_reads_back_as_it_was_saved() {
    for (index, row) in association_rows().into_iter().enumerate() {
        let waf = Waf::new();
        let name = format!("association-acl-{index}");
        let case = row.case;
        let created = waf
            .ok(
                "CreateWebACL",
                create_request(&name, row.scope, row.association_config.as_ref(), json!([])),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create must answer an Id, got {created}"))
            .to_string();

        let read = get_web_acl(&waf, row.scope, &name, &id).await;
        assert_eq!(
            read["WebACL"]["AssociationConfig"],
            row.association_config
                .clone()
                .unwrap_or(Value::Null),
            "case {case:?}: the AssociationConfig must read back as it was saved, got {read}"
        );
        assert_eq!(
            read["ApplicationIntegrationURL"].is_null(),
            !row.integrates,
            "case {case:?}: a web ACL that {} the application must {} an integration URL, \
             got {read}",
            if row.integrates {
                "integrates with"
            } else {
                "does not integrate with"
            },
            if row.integrates { "carry" } else { "carry no" },
        );
        if row.integrates {
            let url = read["ApplicationIntegrationURL"]
                .as_str()
                .unwrap_or_default();
            asserted_integration_id(url, ::smithy_server_runtime::UNSERVED_URL_BASE, case);
        }
    }
}

/// A read-modify-write that hands back the `AssociationConfig` it read keeps every setting
/// in it, so the SDK-injection setting survives the cycle a reconciling client makes.
#[tokio::test]
async fn a_read_modify_write_keeps_the_sdk_injection_setting() {
    let case = "GetWebACL then UpdateWebACL with what was read";
    let waf = Waf::new();
    let name = "read-modify-write-acl";
    let scope = "REGIONAL";
    let association_config = json!({
        "ChallengeJavascriptSdkInjection": { "API_GATEWAY": {}, "COGNITO_USER_POOL": {} },
    });
    let created = waf
        .ok(
            "CreateWebACL",
            create_request(name, scope, Some(&association_config), json!([])),
        )
        .await;
    let id = created["Summary"]["Id"].as_str().expect("Summary.Id").to_string();

    let read = get_web_acl(&waf, scope, name, &id).await;
    assert_eq!(
        read["WebACL"]["AssociationConfig"], association_config,
        "case {case:?}: the setting must be there to be handed back, got {read}"
    );

    // The write a client makes from what it read: the configuration it was answered,
    // unchanged, together with the lock token that read reported.
    let mut update = create_request(name, scope, Some(&read["WebACL"]["AssociationConfig"]), json!([]));
    update["Id"] = json!(id);
    update["LockToken"] = read["LockToken"].clone();
    update["Description"] = json!("reconciled from what the read answered");
    waf.ok("UpdateWebACL", update).await;

    let again = get_web_acl(&waf, scope, name, &id).await;
    assert_eq!(
        again["WebACL"]["AssociationConfig"], association_config,
        "case {case:?}: the write must keep the setting it was handed, got {again}"
    );
    assert!(
        !again["ApplicationIntegrationURL"].is_null(),
        "case {case:?}: the web ACL still integrates with the application, so the read must \
         still carry an integration URL, got {again}"
    );
}

/// A web ACL whose rules run a managed rule group of the intelligent-threat-mitigation kind
/// integrates with the application through that group's SDK, so `GetWebACL` reports the
/// integration URL for it — and for a group of any other kind it does not.
///
/// The table is the WHOLE published catalogue
/// (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`, whose
/// `is_advanced_managed_rule_set` is the "advanced set of protections, such as the … rule
/// groups that are used for WAF intelligent threat mitigation" of
/// `ManagedProductDescriptor$IsAdvancedManagedRuleSet`), one row per group.
#[tokio::test]
async fn the_integration_url_follows_the_rule_groups_a_web_acl_runs() {
    let waf = Waf::new();
    let scope = "REGIONAL";
    for (index, group) in MANAGED_RULE_GROUP_CATALOGUE.iter().enumerate() {
        let ManagedRuleGroupDescriptor {
            vendor_name,
            name: group_name,
            is_advanced_managed_rule_set,
            ..
        } = group;
        let case = format!("{vendor_name}/{group_name}");
        let name = format!("integration-group-acl-{index}");
        let created = waf
            .ok(
                "CreateWebACL",
                create_request(
                    &name,
                    scope,
                    None,
                    json!([managed_rule(
                        "runs-the-group",
                        managed_statement(vendor_name, group_name)
                    )]),
                ),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create must answer an Id, got {created}"))
            .to_string();

        let read = get_web_acl(&waf, scope, &name, &id).await;
        assert_eq!(
            !read["ApplicationIntegrationURL"].is_null(),
            *is_advanced_managed_rule_set,
            "case {case:?}: a web ACL running this group must {} an integration URL, got \
             {read}",
            if *is_advanced_managed_rule_set {
                "carry"
            } else {
                "carry no"
            }
        );
    }
}

/// The integration URL a read publishes is composed from the address this emulator answers
/// on, so a client that follows it comes back here rather than leaving for a public host.
#[tokio::test]
async fn a_web_acl_integration_url_is_published_at_this_emulators_address() {
    let case = "an emulator served at its own base address";
    let waf = Waf::served_at(SERVED_BASE);
    let name = "served-integration-acl";
    let scope = "REGIONAL";
    let created = waf
        .ok(
            "CreateWebACL",
            create_request(
                name,
                scope,
                Some(&json!({
                    "ChallengeJavascriptSdkInjection": { "API_GATEWAY": {} },
                })),
                json!([]),
            ),
        )
        .await;
    let id = created["Summary"]["Id"].as_str().expect("Summary.Id").to_string();

    let read = get_web_acl(&waf, scope, name, &id).await;
    let url = read["ApplicationIntegrationURL"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the read must report an URL, got {read}"));
    let id_in_url = asserted_integration_id(url, SERVED_BASE, case);

    // The same account, so the same id: what the address changes is where the integration
    // is published, not which integration it is.
    let listed = waf
        .ok("ListAPIKeys", json!({ "Scope": scope }))
        .await;
    let listed_url = listed["ApplicationIntegrationURL"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the listing must report an URL, got {listed}"));
    assert_eq!(
        listed_url, url,
        "case {case:?}: both operations report the account's one integration URL, got \
         {listed_url:?} against {url:?}"
    );
    asserted_integration_id(listed_url, SERVED_BASE, case);
    assert_eq!(
        id_in_url.chars().count(),
        INTEGRATION_ID_LENGTH,
        "case {case:?}: the id must keep its published width, got {id_in_url:?}"
    );
}
