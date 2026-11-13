// SPDX-License-Identifier: CC0-1.0

//! What a `ManagedRuleGroupStatement` that PINS a version resolves to, end to end
//! over the wire.
//!
//! The rule asserted here: a managed rule group's versions are its VENDOR's, and
//! `ManagedRuleGroupStatement$Version` names one of them — "the version of the
//! managed rule group to use. If you specify this, the version setting is fixed until
//! you change it… You can retrieve the available versions for the managed rule group
//! by calling `ListAvailableManagedRuleGroupVersions`". So a statement pinning a
//! version that listing does not carry names nothing that exists, and every operation
//! that resolves the statement refuses it with the modelled
//! `WAFNonexistentItemException` (HTTP 400) and its constant not-found sentence —
//! `CheckCapacity`, which would otherwise price a rule set WAF could not run, and the
//! web ACL save path, which would otherwise store a pin that resolves to nothing. A
//! statement naming no version at all runs "the vendor's default version", and one
//! naming a published version is resolved and priced.
//!
//! The rows span the class: every version the vendor offers for a group is accepted
//! (read from `ListAvailableManagedRuleGroupVersions`, not written out), and several
//! shapes of unpublished-but-well-formed version name are refused, over more than one
//! catalogued group — so neither half is asserted on one hand-picked name.
//!
//! Every call goes through the router's front door, the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{visibility, Waf};

/// The scope the rows are made in. The versions on offer are the vendor's, so a scope
/// is only the namespace the call is made in.
const SCOPE: &str = "REGIONAL";

/// The vendor whose managed rule groups this service publishes.
const VENDOR: &str = "AWS";

/// The sentence `WAFNonexistentItemException` carries, whatever the request named —
/// written out here rather than imported, so the table does not restate the code
/// under test (`crate::exception_messages` pins the same text for every operation
/// that answers this exception).
const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// Version names no vendor ever published, each well formed under
/// `VersionKeyString` (`@length 1..64`, `@pattern ^[\w#:\.\-/]+$`), so the refusal is
/// the RESOLUTION's and not the member's own constraint.
const UNPUBLISHED_VERSIONS: &[&str] = &[
    "Version_9.99",
    "Version_0.0",
    "99.99.99",
    "not-a-published-version",
];

/// The awsJson error body's message member, whichever casing the shape models.
fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A rule that runs the managed group `(VENDOR, group)`, pinned to `version` when the
/// row names one. Such a rule carries `OverrideAction` rather than `Action` ("you can
/// only use this for rule statements that reference a rule group",
/// `Rule$OverrideAction`).
fn pinned_rule(group: &str, version: Option<&str>) -> Value {
    let mut statement = json!({
        "ManagedRuleGroupStatement": { "VendorName": VENDOR, "Name": group }
    });
    if let Some(version) = version {
        statement["ManagedRuleGroupStatement"]["Version"] = json!(version);
    }
    json!({
        "Name": "runs-managed",
        "Priority": 0,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility("runs-managed"),
    })
}

/// The versions `ListAvailableManagedRuleGroupVersions` offers for `group`, and the
/// one it reports as the current default.
async fn offered(waf: &Waf, group: &str) -> (Vec<String>, String) {
    let listed = waf
        .ok(
            "ListAvailableManagedRuleGroupVersions",
            json!({ "Scope": SCOPE, "VendorName": VENDOR, "Name": group }),
        )
        .await;
    let versions = listed["Versions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|version| version["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    let default = listed["CurrentDefaultVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("case {group:?}: the group must report a current default version, got {listed}"))
        .to_string();
    (versions, default)
}

/// The two groups the rows are driven over: the versioned group a pinned statement is
/// usually written against, and a second one, so neither half of the rule is asserted
/// on one group.
const GROUPS: &[&str] = &[
    "AWSManagedRulesCommonRuleSet",
    "AWSManagedRulesAdminProtectionRuleSet",
];

/// Every version the vendor offers can be pinned: `CheckCapacity` prices the rule set
/// and `CreateWebACL` stores it, reading back the pin verbatim. So does a statement
/// naming no version, and one naming the current default.
#[tokio::test]
async fn a_version_the_vendor_offers_can_be_pinned_and_priced() {
    let waf = Waf::new();
    for group in GROUPS {
        let (versions, default) = offered(&waf, group).await;
        assert!(
            !versions.is_empty(),
            "case {group:?}: the group must offer at least one version to pin"
        );
        let mut rows: Vec<Option<String>> = versions.iter().cloned().map(Some).collect();
        rows.push(Some(default));
        rows.push(None);

        for (index, version) in rows.iter().enumerate() {
            let case = format!("{group} pinned to {version:?}");
            let rules = json!([pinned_rule(group, version.as_deref())]);
            let priced = waf
                .ok("CheckCapacity", json!({ "Scope": SCOPE, "Rules": &rules }))
                .await;
            assert!(
                priced["Capacity"].as_i64().is_some_and(|capacity| capacity > 0),
                "case {case:?}: an offered version must be priced, got {priced}"
            );

            let name = format!("pinned-{}-{index}", group.to_lowercase());
            let created = waf
                .ok(
                    "CreateWebACL",
                    json!({
                        "Name": &name,
                        "Scope": SCOPE,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility(&name),
                        "Rules": &rules,
                    }),
                )
                .await;
            let id = created["Summary"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Summary.Id, got {created}"))
                .to_string();
            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": &name, "Scope": SCOPE, "Id": &id }),
                )
                .await;
            assert_eq!(
                got["WebACL"]["Rules"], rules,
                "case {case:?}: the pinned statement must read back verbatim, got {got}"
            );
        }
    }
}

/// A version no release of the group ever carried resolves to nothing, so
/// `CheckCapacity`, `CreateWebACL` and `UpdateWebACL` all refuse it with the
/// not-found sentence — and no web ACL is created.
#[tokio::test]
async fn a_version_the_vendor_never_published_is_refused_by_every_operation_that_resolves_it() {
    let waf = Waf::new();
    // A stored web ACL the update rows are driven against, so a refused update can be
    // checked for having changed nothing.
    let name = "pinned-update-acl";
    let stored_rules = json!([pinned_rule(GROUPS[0], None)]);
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(name),
                "Rules": &stored_rules,
            }),
        )
        .await;
    let id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let lock_token = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    for (group_index, group) in GROUPS.iter().enumerate() {
        let (versions, _default) = offered(&waf, group).await;
        for (version_index, version) in UNPUBLISHED_VERSIONS.iter().enumerate() {
            let case = format!("{group} pinned to the unpublished {version:?}");
            assert!(
                !versions.iter().any(|offered| offered == version),
                "case {case:?}: the row needs a version the group does NOT offer, and \
                 it offers {versions:?}"
            );
            let rules = json!([pinned_rule(group, Some(version))]);
            // `EntityName` admits word characters and hyphens only, so the row is
            // named by its position rather than by the version it pins.
            let refused_name = format!("unpinned-{group_index}-{version_index}");

            for (operation, body) in [
                (
                    "CheckCapacity",
                    json!({ "Scope": SCOPE, "Rules": &rules }),
                ),
                (
                    "CreateWebACL",
                    json!({
                        "Name": &refused_name,
                        "Scope": SCOPE,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility(&refused_name),
                        "Rules": &rules,
                    }),
                ),
                (
                    "UpdateWebACL",
                    json!({
                        "Name": name,
                        "Scope": SCOPE,
                        "Id": &id,
                        "LockToken": &lock_token,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility(name),
                        "Rules": &rules,
                    }),
                ),
            ] {
                let case = format!("{operation}: {case}");
                let refused = waf
                    .refused(operation, body, "WAFNonexistentItemException", &case)
                    .await;
                assert_eq!(
                    error_message(&refused),
                    NONEXISTENT_ITEM,
                    "case {case:?}: expected the constant not-found sentence, got {refused}"
                );
                assert!(
                    refused["Capacity"].is_null()
                        && refused["Summary"].is_null()
                        && refused["NextLockToken"].is_null(),
                    "case {case:?}: a refusal must carry no answer of its own, got {refused}"
                );
            }

            let listed = waf.ok("ListWebACLs", json!({ "Scope": SCOPE })).await;
            let names: Vec<&str> = listed["WebACLs"]
                .as_array()
                .map(|summaries| {
                    summaries
                        .iter()
                        .filter_map(|summary| summary["Name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                names,
                vec![name],
                "case {case:?}: a refused create must leave no web ACL behind, got {listed}"
            );

            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": name, "Scope": SCOPE, "Id": &id }),
                )
                .await;
            assert_eq!(
                (got["WebACL"]["Rules"].clone(), got["LockToken"].as_str()),
                (stored_rules.clone(), Some(lock_token.as_str())),
                "case {case:?}: a refused update must change nothing at all, got {got}"
            );
        }
    }
}
