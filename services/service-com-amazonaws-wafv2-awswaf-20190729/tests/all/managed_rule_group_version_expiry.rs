// SPDX-License-Identifier: CC0-1.0

//! The LIFECYCLE of a version of a VENDOR-published managed rule group — offered,
//! then expired — end to end over the wire.
//!
//! The rule asserted here: a version of a managed rule group is offered by its
//! vendor until the vendor retires it, and from then on every call that resolves
//! that version refuses it with the modelled
//! `WAFExpiredManagedRuleGroupVersionException` (HTTP 400) rather than serving it or
//! treating it as a version nobody published — `DescribeManagedRuleGroup` for the
//! version a request names, and the save and pricing paths for a
//! `ManagedRuleGroupStatement$Version` that pins one (`CreateWebACL`,
//! `UpdateWebACL`, `CheckCapacity`) — while `ListAvailableManagedRuleGroupVersions`
//! prunes a retired version from what the vendor OFFERS. A refused save stores
//! nothing, so the roll-forward path a pinning caller takes is: the pin is refused,
//! the listing names a version still on offer, and that one saves.
//!
//! Unlike the refusals in [`crate::exception_messages`], this one's sentence is a
//! TEMPLATE: WAF words it with the version, the group and the vendor the call named,
//! so [`expired_message`] spells the fixed text out here as a literal and fills the
//! variable parts from the row.
//!
//! Which versions are retired is published data, so every expectation below is
//! composed FROM the service's managed-rule-group catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`), and the tables span every
//! group in it: those whose vendor has retired nothing, one version and several.
//! Every call goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ManagedRuleGroupDescriptor, ManagedRuleGroupRelease, ManagedVersionState,
    MANAGED_RULE_GROUP_CATALOGUE,
};

use crate::web_acl_lifecycle::{create_web_acl, visibility, Waf};

/// A `ManagedRuleGroupStatement` that PINS `version` of `(vendor, name)`.
fn pinned_statement(vendor: &str, name: &str, version: &str) -> Value {
    json!({
        "ManagedRuleGroupStatement": {
            "VendorName": vendor,
            "Name": name,
            "Version": version,
        }
    })
}

/// A one-rule list whose rule RUNS the group `statement` names: such a rule carries
/// `OverrideAction` rather than `Action` — "you can only use this for rule
/// statements that reference a rule group" (`OverrideAction`'s own doc).
fn rules_running(statement: Value) -> Value {
    json!([{
        "Name": "managed",
        "Priority": 0,
        "Statement": statement,
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility("managed"),
    }])
}

/// The sentence WAF words an expired-version refusal with, as recorded from it: its
/// fixed text, with the version, the group and its vendor taken from the call.
fn expired_message(vendor: &str, name: &str, version: &str) -> String {
    format!("You're trying to use an expired version ({version}) of {name} by {vendor}.")
}

/// The awsJson error body's message member, whichever casing the shape models.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The version names a `ListAvailableManagedRuleGroupVersions` page lists.
fn listed_names(response: &Value) -> Vec<String> {
    response["Versions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|version| version["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every `(group, retired version)` pair the catalogue publishes — the rows the
/// tables below are driven over.
fn retired_rows() -> Vec<(&'static ManagedRuleGroupDescriptor, &'static str)> {
    MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .flat_map(|group| {
            group
                .expired_versions
                .iter()
                .map(move |release| (group, release.name))
        })
        .collect()
}

/// Describing a version its vendor has retired is refused with the modelled
/// `WAFExpiredManagedRuleGroupVersionException`, whose sentence names the version,
/// the group and the vendor — and the refusal describes nothing.
///
/// The table is every retired version the catalogue publishes, crossed with both
/// declared scopes: a managed rule group is a vendor's publication and a scope is
/// only the namespace a request is made in, so the version's state cannot differ
/// between them.
#[tokio::test]
async fn every_retired_version_is_refused_as_expired_by_the_describe() {
    let rows = retired_rows();
    assert!(
        rows.len() > 1,
        "the catalogue must publish retired versions of more than one group for \
         this table to span the class, got {} rows",
        rows.len()
    );

    let waf = Waf::new();
    for (group, version) in rows {
        for scope in ["REGIONAL", "CLOUDFRONT"] {
            let case = format!("{scope} {}/{}@{version}", group.vendor_name, group.name);
            let response = waf
                .refused(
                    "DescribeManagedRuleGroup",
                    json!({
                        "Scope": scope,
                        "VendorName": group.vendor_name,
                        "Name": group.name,
                        "VersionName": version,
                    }),
                    "WAFExpiredManagedRuleGroupVersionException",
                    &case,
                )
                .await;
            let expected = expired_message(group.vendor_name, group.name, version);
            assert_eq!(
                error_message(&response),
                expected,
                "case {case:?}: expected the refusal to say {expected:?}, got {response}"
            );
            assert!(
                response["Capacity"].is_null()
                    && response["Rules"].is_null()
                    && response["VersionName"].is_null(),
                "case {case:?}: a refusal must describe nothing, got {response}"
            );
        }
    }
}

/// A retired version is pruned from what the vendor OFFERS while staying KNOWN: it
/// is absent from `ListAvailableManagedRuleGroupVersions` and is never the
/// `CurrentDefaultVersion`, yet the describe still tells it apart from a version
/// nobody published — expired rather than a rejected parameter.
///
/// The table is the WHOLE catalogue, one row per published group, so the groups
/// whose vendor has retired nothing are covered beside those that have.
#[tokio::test]
async fn a_retired_version_is_pruned_from_the_offered_list_but_still_known() {
    let waf = Waf::new();
    let mut retired_rows_seen = 0_usize;
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        let listed = waf
            .ok(
                "ListAvailableManagedRuleGroupVersions",
                json!({
                    "Scope": "REGIONAL",
                    "VendorName": group.vendor_name,
                    "Name": group.name,
                }),
            )
            .await;
        let offered = listed_names(&listed);
        // Whatever its vendor has retired, a published group keeps a version on
        // offer, which is what a pinning caller rolls forward to.
        assert!(
            !offered.is_empty(),
            "case {case:?}: a published group must offer a version, got {listed}"
        );
        let default = listed["CurrentDefaultVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            group.version_state(&default),
            ManagedVersionState::Offered,
            "case {case:?}: the current default must be a version on offer, got {listed}"
        );

        for release in group.expired_versions {
            retired_rows_seen += 1;
            assert!(
                !offered.contains(&release.name.to_string()),
                "case {case:?}: the retired version {:?} must not be offered, \
                 got {listed}",
                release.name
            );
            // The asymmetry: pruned from the listing, still known to the describe.
            let described = waf
                .refused(
                    "DescribeManagedRuleGroup",
                    json!({
                        "Scope": "REGIONAL",
                        "VendorName": group.vendor_name,
                        "Name": group.name,
                        "VersionName": release.name,
                    }),
                    "WAFExpiredManagedRuleGroupVersionException",
                    &case,
                )
                .await;
            assert_eq!(
                error_message(&described),
                expired_message(group.vendor_name, group.name, release.name),
                "case {case:?}: a version pruned from the listing must still be \
                 reported as expired, got {described}"
            );
        }

        // Reading the listing changes nothing: the same page after the describes.
        let again = waf
            .ok(
                "ListAvailableManagedRuleGroupVersions",
                json!({
                    "Scope": "REGIONAL",
                    "VendorName": group.vendor_name,
                    "Name": group.name,
                }),
            )
            .await;
        assert_eq!(
            again, listed,
            "case {case:?}: the offered versions must not change with a read, got \
             {again} after {listed}"
        );
    }
    assert!(
        retired_rows_seen > 1,
        "the catalogue must publish more than one retired version for this table to \
         span the class, got {retired_rows_seen}"
    );
}

/// Pinning a retired version refuses the save and stores NOTHING, refuses the
/// pricing call the same way, and leaves a stored web ACL exactly as it was — while
/// the version the vendor still offers saves and reads back verbatim.
///
/// The table is every retired version the catalogue publishes, each row driving the
/// whole roll-forward path: the refused `CheckCapacity`, the refused `CreateWebACL`
/// whose name no listing shows, the save that pins the offered version instead, the
/// refused `UpdateWebACL` back onto the retired one, and the untouched stored web
/// ACL.
#[tokio::test]
async fn pinning_a_retired_version_refuses_the_save_and_stores_nothing() {
    let waf = Waf::new();
    for (index, (group, version)) in retired_rows().into_iter().enumerate() {
        let case = format!("{}/{}@{version}", group.vendor_name, group.name);
        let acl_name = format!("expired-pin-{index}");
        let expected = expired_message(group.vendor_name, group.name, version);
        let retired_rules =
            rules_running(pinned_statement(group.vendor_name, group.name, version));

        // A version WAF would not run cannot be priced either.
        let priced = waf
            .refused(
                "CheckCapacity",
                json!({ "Scope": "REGIONAL", "Rules": retired_rules.clone() }),
                "WAFExpiredManagedRuleGroupVersionException",
                &case,
            )
            .await;
        assert_eq!(
            error_message(&priced),
            expected,
            "case {case:?}: expected the refusal to say {expected:?}, got {priced}"
        );
        assert!(
            priced["Capacity"].is_null(),
            "case {case:?}: a refusal must price nothing, got {priced}"
        );

        let refused = waf
            .refused(
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": "REGIONAL",
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "Rules": retired_rules.clone(),
                }),
                "WAFExpiredManagedRuleGroupVersionException",
                &case,
            )
            .await;
        assert_eq!(
            error_message(&refused),
            expected,
            "case {case:?}: expected the refusal to say {expected:?}, got {refused}"
        );
        let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
        assert!(
            !listed.to_string().contains(acl_name.as_str()),
            "case {case:?}: a refused CreateWebACL must store nothing, got {listed}"
        );

        // Rolling forward: the version the vendor does offer saves and reads back.
        let offered = group
            .current_default_version()
            .expect("a published group offers a version");
        let offered_statement = pinned_statement(group.vendor_name, group.name, offered);
        let (acl_id, _arn, lock) = create_web_acl(
            &waf,
            "REGIONAL",
            &acl_name,
            rules_running(offered_statement.clone()),
        )
        .await;
        let saved = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            saved["WebACL"]["Rules"][0]["Statement"], offered_statement,
            "case {case:?}: the offered version must read back verbatim, got {saved}"
        );

        // Updating a live web ACL back onto the retired version is refused, and
        // leaves the stored web ACL and its lock token alone.
        let refused_update = waf
            .refused(
                "UpdateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": "REGIONAL",
                    "Id": &acl_id,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&acl_name),
                    "LockToken": &lock,
                    "Rules": retired_rules,
                }),
                "WAFExpiredManagedRuleGroupVersionException",
                &case,
            )
            .await;
        assert_eq!(
            error_message(&refused_update),
            expected,
            "case {case:?}: expected the refusal to say {expected:?}, \
             got {refused_update}"
        );
        let after = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            after, saved,
            "case {case:?}: a refused update must leave the stored web ACL and its \
             lock token exactly as they were, got {after} after {saved}"
        );
    }
}

/// The published version history the expectations above compose themselves from is
/// well formed, and the catalogue reading every consumer of it goes through answers
/// the three states a version name can be in.
///
/// One row per published group, each crossed with every version it offers, every
/// version its vendor has retired, and three names in neither list (a version no
/// vendor publishes, an offered name in the wrong case and a prefix of one).
#[test]
fn the_published_version_history_is_well_formed() {
    let mut retired_counts: BTreeSet<usize> = BTreeSet::new();
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        retired_counts.insert(group.expired_versions.len());
        assert!(
            !group.versions.is_empty(),
            "case {case:?}: a published group keeps a version on offer"
        );

        let newest_offered = group
            .versions
            .last()
            .and_then(ManagedRuleGroupRelease::last_update_timestamp)
            .unwrap_or_else(|| panic!("case {case:?}: an offered version must be dated"))
            .as_secs_f64();
        let mut previous: Option<&ManagedRuleGroupRelease> = None;
        for release in group.expired_versions {
            // `VersionKeyString`: `@length(1..64)`, `@pattern(^[\w#:\.\-/]+$)`.
            assert!(
                (1..=64).contains(&release.name.len())
                    && release
                        .name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_#:.-/".contains(&byte)),
                "case {case:?}: the retired version {:?} must satisfy VersionKeyString",
                release.name
            );
            let at = release
                .last_update_timestamp()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the retired version {:?} must carry a readable \
                         instant, got {:?}",
                        release.name, release.last_update
                    )
                })
                .as_secs_f64();
            if let Some(previous) = previous {
                let before = previous
                    .last_update_timestamp()
                    .expect("a readable instant")
                    .as_secs_f64();
                assert!(
                    before < at,
                    "case {case:?}: retired versions are published oldest first, so \
                     {:?} must predate {:?}, got {before} and {at}",
                    previous.name,
                    release.name
                );
            }
            previous = Some(release);
            // A retired version is one a LATER version superseded: it sits somewhere
            // in the published ladder, so it predates the newest version on offer
            // without having to predate every one of them — a vendor retires a
            // version in the middle of its ladder, which is what leaves the gaps a
            // published history has.
            assert!(
                at < newest_offered,
                "case {case:?}: the retired {:?} must predate the newest version \
                 still offered, got {at} and {newest_offered}",
                release.name
            );
            assert_eq!(
                group.version_state(release.name),
                ManagedVersionState::Expired,
                "case {case:?}: {:?} is retired, so it must read as expired",
                release.name
            );
            assert_eq!(
                group.offered_version(Some(release.name)),
                None,
                "case {case:?}: a retired version is not one the vendor offers, \
                 got {:?}",
                release.name
            );
        }

        for release in group.versions {
            assert_eq!(
                group.version_state(release.name),
                ManagedVersionState::Offered,
                "case {case:?}: {:?} is on offer, so it must read as offered",
                release.name
            );
        }
        let default = group
            .current_default_version()
            .expect("a published group offers a version");
        assert_eq!(
            group.version_state(default),
            ManagedVersionState::Offered,
            "case {case:?}: the current default {default:?} must be on offer"
        );

        for unknown in [
            "Version_999.0".to_string(),
            default.to_lowercase(),
            default[..default.len() - 1].to_string(),
        ] {
            assert_eq!(
                group.version_state(&unknown),
                ManagedVersionState::Unknown,
                "case {case:?}: {unknown:?} is no version this group published, so it \
                 must read as unknown rather than expired"
            );
        }
    }

    // The rows above are only worth crossing if the catalogue spans vendors that
    // have retired nothing, one version and several.
    assert!(
        retired_counts.contains(&0)
            && retired_counts.contains(&1)
            && retired_counts.iter().any(|count| *count > 1),
        "the catalogue must publish groups with no retired version, with one and \
         with several, got the counts {retired_counts:?}"
    );
}
