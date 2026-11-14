// SPDX-License-Identifier: CC0-1.0

//! The vendor-side managed-rule-set surface, end to end over the wire.
//!
//! `ManagedRuleSet` is "a set of rules that is managed by Amazon Web Services and
//! Amazon Web Services Marketplace sellers to provide versioned managed rule groups
//! for customers of WAF", and the model's note on the shape names the four
//! operations that work on one: `ListManagedRuleSets`, `GetManagedRuleSet`,
//! `PutManagedRuleSetVersions` and `UpdateManagedRuleSetVersionExpiryDate`. The rule
//! these suites assert is: each of them addresses a set the ACCOUNT holds, so an
//! identity no set of the account carries is refused with 400
//! `WAFNonexistentItemException` and nothing is published under it — target
//! resolution comes before every other judgement the request invites — while against
//! a set that IS held, a put stores the versions it names (each priced by the rule
//! group that defines it) and an expiry update stores the instant it names on one
//! published version, both rotating the set's optimistic-lock token, and both
//! readable back through `GetManagedRuleSet`.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in scope, in how the addressed identity fails to resolve (name,
//! id, both), in how many versions a put publishes and in their keys, and in which
//! published version an expiry names.
//!
//! No operation of this model CREATES a managed rule set — a vendor's set exists
//! because the vendor was onboarded as a managed-rule-set vendor, which is not an API
//! call — so these suites provision one through the state API
//! ([`provision_managed_rule_set`], the same seam `crate::top_path_statistics` uses
//! for WAF's data-plane request log) and then drive every assertion through the
//! router's front door, the same door an SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ComAmazonawsWafv2Awswaf20190729State, DEFAULT_REGION,
    MAX_PUBLISHED_VERSIONS_PER_MANAGED_RULE_SET, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{is_model_uuid, rule, sqli_statement, Waf, TEST_REGION};

/// A `LockToken`-shaped token no set carries.
const FOREIGN_LOCK_TOKEN: &str = "11111111-2222-3333-4444-555555555555";

/// An `EntityId`-shaped id no set carries.
const FOREIGN_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The vendor a provisioned set belongs to, as its label namespace names it.
const VENDOR: &str = "ExampleVendor";

/// A declared capacity for the rule groups a published version is defined by; each
/// published version reports the capacity of its own rule group, so the rows use
/// different ones.
const RULE_GROUP_CAPACITIES: [i64; 2] = [50, 120];

/// Provision a managed rule set for [`VENDOR`] in `scope`, and answer
/// `(id, arn, lock token)`.
///
/// The one thing these suites cannot do over the wire: no operation in this model
/// creates a managed rule set.
///
/// Shared with the sibling suites that need a set that EXISTS (see
/// `crate::refusal_precedence`) rather than copied into each one.
pub(crate) fn provision_managed_rule_set(
    engine: &engine::InMemoryEngine,
    scope: &str,
    name: &str,
    description: Option<&str>,
) -> (String, String, String) {
    let region = if scope == SCOPE_CLOUDFRONT {
        DEFAULT_REGION
    } else {
        TEST_REGION
    };
    let entry = engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .insert_managed_rule_set(
            scope,
            VENDOR,
            name,
            region,
            description.map(str::to_string),
        );
    (
        entry.managed_rule_set.id,
        entry.managed_rule_set.arn,
        entry.lock_token,
    )
}

/// `GetManagedRuleSet` for `(scope, name, id)`, as a 200.
async fn get_managed_rule_set(waf: &Waf, scope: &str, name: &str, id: &str) -> Value {
    waf.ok(
        "GetManagedRuleSet",
        json!({ "Name": name, "Scope": scope, "Id": id }),
    )
    .await
}

/// A `VersionsToPublish` entry.
fn version_to_publish(arn: &str, forecasted_lifetime: i64) -> Value {
    json!({ "AssociatedRuleGroupArn": arn, "ForecastedLifetime": forecasted_lifetime })
}

/// Create the rule groups a put publishes its versions by, and answer their ARNs.
async fn vendor_rule_groups(waf: &Waf, scope: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, capacity) in RULE_GROUP_CAPACITIES.into_iter().enumerate() {
        let name = format!("vendor-group-{index}");
        let (_id, arn, _lock) = create_rule_group(
            waf,
            scope,
            &name,
            capacity,
            json!([rule("inspect", sqli_statement())]),
        )
        .await;
        arns.push(arn);
    }
    arns
}

/// Every way an addressed managed rule set can fail to resolve: the case name and
/// the `(Name, Id)` the request carries, given the provisioned set's own.
fn unresolvable_identities(name: &str, id: &str) -> Vec<(String, String, String)> {
    vec![
        (
            "a name no set carries".to_string(),
            format!("{name}-absent"),
            id.to_string(),
        ),
        (
            "an id no set carries".to_string(),
            name.to_string(),
            FOREIGN_ID.to_string(),
        ),
        (
            "neither the name nor the id".to_string(),
            format!("{name}-absent"),
            FOREIGN_ID.to_string(),
        ),
    ]
}

/// Against an identity no managed rule set of the account carries, every operation
/// of this surface answers the nonexistent-item failure and publishes nothing.
///
/// The rows cross both scopes with each way the identity can fail to resolve, and
/// the empty-account row is the one the fabricated-set case in the recorded check
/// exercises: an account that holds no managed rule set at all.
#[tokio::test]
async fn an_identity_no_managed_rule_set_carries_is_refused() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let (waf, engine) = Waf::sharing_engine();
        let rule_groups = vendor_rule_groups(&waf, scope).await;
        let set_name = "vendor-set";
        let (set_id, _set_arn, set_lock) =
            provision_managed_rule_set(&engine, scope, set_name, Some("the vendor's set"));

        let mut identities = unresolvable_identities(set_name, &set_id);
        identities.push((
            "an account that holds no set under either name".to_string(),
            "never-provisioned".to_string(),
            FOREIGN_ID.to_string(),
        ));

        for (case, name, id) in identities {
            let case = format!("{scope}, {case}");
            waf.refused(
                "GetManagedRuleSet",
                json!({ "Name": &name, "Scope": scope, "Id": &id }),
                "WAFNonexistentItemException",
                &case,
            )
            .await;
            waf.refused(
                "PutManagedRuleSetVersions",
                json!({
                    "Name": &name,
                    "Scope": scope,
                    "Id": &id,
                    "LockToken": FOREIGN_LOCK_TOKEN,
                    "RecommendedVersion": "Version_1.0",
                    "VersionsToPublish": {
                        "Version_1.0": version_to_publish(&rule_groups[0], 90),
                    },
                }),
                "WAFNonexistentItemException",
                &case,
            )
            .await;
            waf.refused(
                "UpdateManagedRuleSetVersionExpiryDate",
                json!({
                    "Name": &name,
                    "Scope": scope,
                    "Id": &id,
                    "LockToken": FOREIGN_LOCK_TOKEN,
                    "VersionToExpire": "Version_1.0",
                    "ExpiryTimestamp": 1_800_000_000,
                }),
                "WAFNonexistentItemException",
                &case,
            )
            .await;
        }

        // Nothing was published under any of the refused identities: the account
        // still holds exactly the one provisioned set, and it still offers nothing.
        let listed = waf
            .ok("ListManagedRuleSets", json!({ "Scope": scope }))
            .await;
        let names: Vec<&str> = listed["ManagedRuleSets"]
            .as_array()
            .expect("ManagedRuleSets")
            .iter()
            .map(|summary| summary["Name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec![set_name],
            "case {scope}: a refused call must publish nothing, got {listed}"
        );
        let got = get_managed_rule_set(&waf, scope, set_name, &set_id).await;
        assert_eq!(
            got["ManagedRuleSet"]["PublishedVersions"],
            json!({}),
            "case {scope}: a refused put must leave the set offering nothing, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(set_lock),
            "case {scope}: a refused call must not rotate the lock token, got {got}"
        );
    }
}

/// One put to publish: the version keys it offers, and which rule group defines
/// each.
struct PutCase {
    case: &'static str,
    /// `(version key, index into the vendor's rule groups, forecasted lifetime)`.
    versions: Vec<(&'static str, usize, i64)>,
    /// The version the put recommends, if any.
    recommended: Option<&'static str>,
}

/// The rows: one version and several, keys in the vendor's own notations, and a put
/// that recommends none.
fn put_cases() -> Vec<PutCase> {
    vec![
        PutCase {
            case: "one version",
            versions: vec![("Version_1.0", 0, 90)],
            recommended: Some("Version_1.0"),
        },
        PutCase {
            case: "two versions defined by different rule groups",
            versions: vec![("Version_1.0", 0, 90), ("Version_2.0", 1, 365)],
            recommended: Some("Version_2.0"),
        },
        PutCase {
            case: "version keys in a dotted and a hashed notation",
            versions: vec![("1.2.3", 1, 30), ("release#2026-01", 0, 1)],
            recommended: None,
        },
    ]
}

/// A put publishes the versions it names against a set the account holds: each reads
/// back with the rule group that defines it, that group's capacity, its forecasted
/// lifetime and its publish instant, the set's recommended version is what the put
/// named, and the set's lock token rotates.
#[tokio::test]
async fn a_put_publishes_the_versions_it_names() {
    for row in put_cases() {
        let (waf, engine) = Waf::sharing_engine();
        let rule_groups = vendor_rule_groups(&waf, SCOPE_REGIONAL).await;
        let set_name = "vendor-set";
        let (set_id, set_arn, set_lock) =
            provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

        let mut versions_to_publish = json!({});
        for (version, group, lifetime) in &row.versions {
            versions_to_publish[*version] = version_to_publish(&rule_groups[*group], *lifetime);
        }
        let mut request = json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": &set_lock,
            "VersionsToPublish": versions_to_publish,
        });
        if let Some(recommended) = row.recommended {
            request["RecommendedVersion"] = json!(recommended);
        }

        // A stale token is refused and publishes nothing, so the versions below are
        // there because the accepted put put them there.
        let mut stale = request.clone();
        stale["LockToken"] = json!(FOREIGN_LOCK_TOKEN);
        waf.refused(
            "PutManagedRuleSetVersions",
            stale,
            "WAFOptimisticLockException",
            row.case,
        )
        .await;
        let unchanged = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
        assert_eq!(
            unchanged["ManagedRuleSet"]["PublishedVersions"],
            json!({}),
            "case {:?}: a refused put must publish nothing, got {unchanged}",
            row.case
        );

        let published = waf.ok("PutManagedRuleSetVersions", request).await;
        let next_lock = published["NextLockToken"]
            .as_str()
            .expect("NextLockToken")
            .to_string();
        assert!(
            is_model_uuid(&next_lock),
            "case {:?}: the put must answer a UUID NextLockToken, got {next_lock:?}",
            row.case
        );
        assert_ne!(
            next_lock, set_lock,
            "case {:?}: the put must rotate the lock token",
            row.case
        );

        let got = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
        let set = &got["ManagedRuleSet"];
        assert_eq!(
            set["ARN"],
            json!(set_arn),
            "case {:?}: the set must keep its ARN, got {got}",
            row.case
        );
        assert_eq!(
            set["LabelNamespace"],
            json!(format!("awswaf:managed:{VENDOR}:{set_name}:")),
            "case {:?}: the set must carry the documented label namespace prefix, got {got}",
            row.case
        );
        assert_eq!(
            got["LockToken"],
            json!(next_lock),
            "case {:?}: the read must answer the rotated lock token, got {got}",
            row.case
        );
        assert_eq!(
            set["RecommendedVersion"],
            row.recommended.map_or(Value::Null, |version| json!(version)),
            "case {:?}: the set must recommend what the put named, got {got}",
            row.case
        );
        let offered = set["PublishedVersions"]
            .as_object()
            .expect("PublishedVersions")
            .clone();
        assert_eq!(
            offered.len(),
            row.versions.len(),
            "case {:?}: the set must offer exactly the versions the put named, got {got}",
            row.case
        );
        for (version, group, lifetime) in &row.versions {
            let entry = &offered[*version];
            assert_eq!(
                entry["AssociatedRuleGroupArn"],
                json!(rule_groups[*group]),
                "case {:?}: version {version} must name the rule group that defines it, \
                 got {entry}",
                row.case
            );
            assert_eq!(
                entry["Capacity"],
                json!(RULE_GROUP_CAPACITIES[*group]),
                "case {:?}: version {version} must report the capacity of its rule group, \
                 got {entry}",
                row.case
            );
            assert_eq!(
                entry["ForecastedLifetime"],
                json!(lifetime),
                "case {:?}: version {version} must keep its forecasted lifetime, got {entry}",
                row.case
            );
            assert!(
                entry["PublishTimestamp"].as_f64().is_some_and(|at| at > 0.0),
                "case {:?}: version {version} must carry the instant it was published, \
                 got {entry}",
                row.case
            );
            assert!(
                entry["ExpiryTimestamp"].is_null(),
                "case {:?}: version {version} must not expire until a vendor says so, \
                 got {entry}",
                row.case
            );
        }
    }
}

/// An expiry update stores the instant it names on the published version it names,
/// rotates the set's lock token, and refuses a version the set has not published.
///
/// The rows differ in WHICH published version expires and in the instant named, and
/// the re-publish row pins that a later put keeps both the expiry a vendor set and
/// the version's first publish instant — `PublishTimestamp` is "the time that you
/// FIRST published this version", `LastUpdateTimestamp` "the last time that you
/// updated" it.
#[tokio::test]
async fn an_expiry_update_stores_the_instant_it_names() {
    let (waf, engine) = Waf::sharing_engine();
    let rule_groups = vendor_rule_groups(&waf, SCOPE_REGIONAL).await;
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);
    let lock_token = waf
        .ok(
            "PutManagedRuleSetVersions",
            json!({
                "Name": set_name,
                "Scope": SCOPE_REGIONAL,
                "Id": &set_id,
                "LockToken": &set_lock,
                "RecommendedVersion": "Version_2.0",
                "VersionsToPublish": {
                    "Version_1.0": version_to_publish(&rule_groups[0], 90),
                    "Version_2.0": version_to_publish(&rule_groups[1], 365),
                },
            }),
        )
        .await["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();

    // A version the set has not published is not on the vendor's list of offerings,
    // so there is nothing to expire.
    waf.refused(
        "UpdateManagedRuleSetVersionExpiryDate",
        json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": &lock_token,
            "VersionToExpire": "Version_9.9",
            "ExpiryTimestamp": 1_800_000_000,
        }),
        "WAFNonexistentItemException",
        "a version the set has not published",
    )
    .await;
    // A stale token is refused for the lock, not for the version.
    waf.refused(
        "UpdateManagedRuleSetVersionExpiryDate",
        json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": FOREIGN_LOCK_TOKEN,
            "VersionToExpire": "Version_1.0",
            "ExpiryTimestamp": 1_800_000_000,
        }),
        "WAFOptimisticLockException",
        "an expiry update under a stale lock token",
    )
    .await;
    let unchanged = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    assert!(
        unchanged["ManagedRuleSet"]["PublishedVersions"]["Version_1.0"]["ExpiryTimestamp"]
            .is_null(),
        "a refused expiry update must set no expiry, got {unchanged}"
    );
    assert_eq!(
        unchanged["LockToken"],
        json!(lock_token),
        "a refused expiry update must not rotate the lock token, got {unchanged}"
    );

    // Each published version takes its own expiry, at its own instant.
    let mut lock_token = lock_token;
    let expiries: [(&str, i64); 2] = [("Version_1.0", 1_800_000_000), ("Version_2.0", 1_900_000_000)];
    for (version, at) in expiries {
        let answered = waf
            .ok(
                "UpdateManagedRuleSetVersionExpiryDate",
                json!({
                    "Name": set_name,
                    "Scope": SCOPE_REGIONAL,
                    "Id": &set_id,
                    "LockToken": &lock_token,
                    "VersionToExpire": version,
                    "ExpiryTimestamp": at,
                }),
            )
            .await;
        assert_eq!(
            answered["ExpiringVersion"],
            json!(version),
            "case {version}: the answer must name the version that is expiring, got {answered}"
        );
        assert_eq!(
            answered["ExpiryTimestamp"].as_f64(),
            Some(at as f64),
            "case {version}: the answer must carry the instant the request named, got {answered}"
        );
        let next = answered["NextLockToken"]
            .as_str()
            .expect("NextLockToken")
            .to_string();
        assert_ne!(
            next, lock_token,
            "case {version}: the update must rotate the lock token"
        );
        lock_token = next;

        let got = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
        let stored = &got["ManagedRuleSet"]["PublishedVersions"][version];
        assert_eq!(
            stored["ExpiryTimestamp"].as_f64(),
            Some(at as f64),
            "case {version}: the stored version must carry the expiry, got {got}"
        );
    }

    // Re-publishing a version keeps the expiry its vendor set and the instant it was
    // first published, and moves only its last-update instant.
    let before = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    let first_published = before["ManagedRuleSet"]["PublishedVersions"]["Version_1.0"]
        ["PublishTimestamp"]
        .clone();
    waf.ok(
        "PutManagedRuleSetVersions",
        json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": &lock_token,
            "RecommendedVersion": "Version_1.0",
            "VersionsToPublish": {
                "Version_1.0": version_to_publish(&rule_groups[1], 120),
            },
        }),
    )
    .await;
    let after = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    let republished = &after["ManagedRuleSet"]["PublishedVersions"]["Version_1.0"];
    assert_eq!(
        republished["PublishTimestamp"], first_published,
        "re-publishing a version must keep the instant it was first published, got {after}"
    );
    assert_eq!(
        republished["ExpiryTimestamp"].as_f64(),
        Some(1_800_000_000_f64),
        "re-publishing a version must keep the expiry its vendor set, got {after}"
    );
    assert_eq!(
        republished["Capacity"],
        json!(RULE_GROUP_CAPACITIES[1]),
        "re-publishing a version must re-price it against its new rule group, got {after}"
    );
    assert_eq!(
        after["ManagedRuleSet"]["PublishedVersions"]["Version_2.0"]["ExpiryTimestamp"].as_f64(),
        Some(1_900_000_000_f64),
        "a put must leave the versions it does not name alone, got {after}"
    );
}

/// A put whose `AssociatedRuleGroupArn` names no rule group of the account is
/// refused as the invalid PARAMETER the ARN is — the one refusal WAF answers for
/// every ARN it cannot use for that member, malformed or merely unresolvable — and
/// it is answered before the set the put targets is resolved at all.
///
/// The rows differ in HOW the ARN fails to name one — a well-formed rule-group ARN
/// nothing minted, and a rule group of the OTHER scope, since a scope is a
/// namespace.
#[tokio::test]
async fn a_put_whose_rule_group_does_not_exist_is_refused() {
    let (waf, engine) = Waf::sharing_engine();
    let cloudfront_groups = vendor_rule_groups(&waf, SCOPE_CLOUDFRONT).await;
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

    let absent = "arn:aws:wafv2:us-west-2:000000000000:regional/rulegroup/\
                  never-created/00000000-0000-0000-0000-000000000000"
        .to_string();
    for (case, arn) in [
        ("a rule-group ARN nothing minted", absent),
        (
            "a rule group of the other scope",
            cloudfront_groups[0].clone(),
        ),
    ] {
        let refusal = waf
            .refused(
                "PutManagedRuleSetVersions",
                json!({
                    "Name": set_name,
                    "Scope": SCOPE_REGIONAL,
                    "Id": &set_id,
                    "LockToken": &set_lock,
                    "VersionsToPublish": {
                        "Version_1.0": version_to_publish(&arn, 90),
                    },
                }),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            (
                refusal["Field"].as_str(),
                refusal["Parameter"].as_str(),
                refusal["Reason"].as_str()
            ),
            (
                Some("RESOURCE_ARN"),
                Some(arn.as_str()),
                Some(
                    "The ARN isn't valid. A valid ARN begins with arn: and includes \
                     other information separated by colons or slashes."
                )
            ),
            "case {case:?}: the refusal must name the ARN member, the rejected ARN and \
             the reason, got {refusal}"
        );
        let got = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
        assert_eq!(
            got["ManagedRuleSet"]["PublishedVersions"],
            json!({}),
            "case {case:?}: a refused put must publish nothing, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(set_lock),
            "case {case:?}: a refused put must not rotate the lock token, got {got}"
        );
    }
}

/// A `VersionsToPublish` KEY that breaks `VersionKeyString`'s own constraints is
/// refused as a bad request — before the set itself is resolved, because the service
/// cannot read a version key out of it at all.
///
/// The rows differ in WHICH constraint the key breaks (the pattern, the length
/// ceiling) and are each tried against a set that exists and one that does not, so
/// the ordering is pinned as well as the refusal. A key that satisfies every
/// constraint reaches the operation, which is the row that keeps this from refusing
/// legal keys.
#[tokio::test]
async fn a_version_key_outside_the_modelled_form_is_refused() {
    let (waf, engine) = Waf::sharing_engine();
    let rule_groups = vendor_rule_groups(&waf, SCOPE_REGIONAL).await;
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

    // `VersionKeyString` is `@length(1..64)` with `@pattern(^[\w#:\.\-/]+$)`
    // (`smithy_query(query="shape",shape="VersionKeyString")`).
    let too_long = "v".repeat(65);
    let illegal: &[(&str, &str)] = &[
        ("a key containing spaces and punctuation", "not a valid version!!"),
        ("a key one character past the length ceiling", too_long.as_str()),
    ];

    for (case, version) in illegal {
        for (target, name, id) in [
            ("a set that exists", set_name, set_id.as_str()),
            ("a set that does not", "never-provisioned", FOREIGN_ID),
        ] {
            let case = format!("{case}, against {target}");
            let body = waf
                .refused(
                    "PutManagedRuleSetVersions",
                    json!({
                        "Name": name,
                        "Scope": SCOPE_REGIONAL,
                        "Id": id,
                        "LockToken": &set_lock,
                        "VersionsToPublish": {
                            *version: version_to_publish(&rule_groups[0], 90),
                        },
                    }),
                    "ValidationException",
                    &case,
                )
                .await;
            let message = body["message"]
                .as_str()
                .or_else(|| body["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains("at 'versionsToPublish'")
                    && message.contains("Map keys must satisfy constraint:"),
                "case {case:?}: the clause must report the map's keys, got {message:?}"
            );
        }
    }

    // Nothing was published by any of those refusals.
    let got = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    assert_eq!(
        got["ManagedRuleSet"]["PublishedVersions"],
        json!({}),
        "a refused put must publish nothing, got {got}"
    );

    // Keys at the edges of the modelled form are published, not refused: the
    // ceiling exactly, and each character class the pattern admits.
    let legal = ["v".repeat(64), "Version_1.0-rc1#2026/01:final".to_string()];
    let mut versions_to_publish = json!({});
    for version in &legal {
        versions_to_publish[version.as_str()] = version_to_publish(&rule_groups[0], 90);
    }
    waf.ok(
        "PutManagedRuleSetVersions",
        json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": &set_lock,
            "VersionsToPublish": versions_to_publish,
        }),
    )
    .await;
    let published = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    for version in &legal {
        assert!(
            !published["ManagedRuleSet"]["PublishedVersions"][version.as_str()].is_null(),
            "case {version:?}: a legal version key must be published, got {published}"
        );
    }
}

/// One managed rule set offers at most
/// [`MAX_PUBLISHED_VERSIONS_PER_MANAGED_RULE_SET`] versions: the quota'th publish
/// still succeeds, the one past it is refused and changes nothing, so no caller can
/// grow one stored set without limit.
#[tokio::test]
async fn the_published_version_count_is_bounded() {
    let (waf, engine) = Waf::sharing_engine();
    let rule_groups = vendor_rule_groups(&waf, SCOPE_REGIONAL).await;
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

    // The quota, published in one call.
    let mut versions_to_publish = json!({});
    for index in 0..MAX_PUBLISHED_VERSIONS_PER_MANAGED_RULE_SET {
        versions_to_publish[format!("Version_{index}")] =
            version_to_publish(&rule_groups[0], 90);
    }
    let lock_token = waf
        .ok(
            "PutManagedRuleSetVersions",
            json!({
                "Name": set_name,
                "Scope": SCOPE_REGIONAL,
                "Id": &set_id,
                "LockToken": &set_lock,
                "VersionsToPublish": versions_to_publish,
            }),
        )
        .await["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();
    let full = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    assert_eq!(
        full["ManagedRuleSet"]["PublishedVersions"]
            .as_object()
            .map(serde_json::Map::len),
        Some(MAX_PUBLISHED_VERSIONS_PER_MANAGED_RULE_SET),
        "the quota'th publish must succeed, got {full}"
    );

    // One more version is refused, and the set keeps exactly what it offered.
    waf.refused(
        "PutManagedRuleSetVersions",
        json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": &lock_token,
            "VersionsToPublish": {
                "Version_one_too_many": version_to_publish(&rule_groups[0], 90),
            },
        }),
        "WAFInvalidParameterException",
        "one version past the bound",
    )
    .await;
    let after = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    assert_eq!(
        after["ManagedRuleSet"]["PublishedVersions"], full["ManagedRuleSet"]["PublishedVersions"],
        "a refused put must leave the set offering what it offered, got {after}"
    );
    assert_eq!(
        after["LockToken"],
        json!(lock_token),
        "a refused put must not rotate the lock token, got {after}"
    );

    // Re-publishing a version the set already offers is not a new version, so it is
    // accepted at the bound.
    waf.ok(
        "PutManagedRuleSetVersions",
        json!({
            "Name": set_name,
            "Scope": SCOPE_REGIONAL,
            "Id": &set_id,
            "LockToken": &lock_token,
            "VersionsToPublish": {
                "Version_0": version_to_publish(&rule_groups[1], 30),
            },
        }),
    )
    .await;
}

/// `ListManagedRuleSets` enumerates only the account's sets in the scope asked for,
/// in a stable order, a page at a time.
#[tokio::test]
async fn list_managed_rule_sets_enumerates_one_scope_a_page_at_a_time() {
    let (waf, engine) = Waf::sharing_engine();

    // An account that holds none answers an empty list and no marker.
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let empty = waf
            .ok("ListManagedRuleSets", json!({ "Scope": scope }))
            .await;
        assert_eq!(
            empty["ManagedRuleSets"],
            json!([]),
            "case {scope}: an account with no managed rule set must answer an empty \
             list, got {empty}"
        );
        assert!(
            empty["NextMarker"].is_null(),
            "case {scope}: an empty page must carry no marker, got {empty}"
        );
    }

    // Provisioned out of alphabetical order, so the listing's order is the service's
    // and not the order they were provisioned in.
    for name in ["set-c", "set-a", "set-b"] {
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, name, Some("a vendor's set"));
    }
    provision_managed_rule_set(&engine, SCOPE_CLOUDFRONT, "set-global", None);

    let listed = waf
        .ok("ListManagedRuleSets", json!({ "Scope": SCOPE_REGIONAL }))
        .await;
    let names: Vec<&str> = listed["ManagedRuleSets"]
        .as_array()
        .expect("ManagedRuleSets")
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["set-a", "set-b", "set-c"],
        "the listing must enumerate only this scope's sets, in a stable order, got {listed}"
    );
    let summary = &listed["ManagedRuleSets"][0];
    assert_eq!(
        summary["Description"],
        json!("a vendor's set"),
        "a summary must carry the set's description, got {summary}"
    );
    assert!(
        summary["LockToken"]
            .as_str()
            .is_some_and(is_model_uuid),
        "a summary must carry the set's current lock token, got {summary}"
    );
    assert!(
        summary["ARN"]
            .as_str()
            .is_some_and(|arn| arn.contains("/managedruleset/")),
        "a summary must carry the set's ARN, got {summary}"
    );

    // Pages hold at most `Limit` summaries and resume from the marker.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "Scope": SCOPE_REGIONAL, "Limit": 2 });
        if let Some(marker) = marker.as_deref() {
            request["NextMarker"] = json!(marker);
        }
        let page = waf.ok("ListManagedRuleSets", request).await;
        let summaries = page["ManagedRuleSets"]
            .as_array()
            .expect("ManagedRuleSets")
            .clone();
        assert!(
            summaries.len() <= 2,
            "a page must hold at most Limit summaries, got {page}"
        );
        if summaries.is_empty() {
            assert!(
                page["NextMarker"].is_null(),
                "the terminal page must carry no marker, got {page}"
            );
            break;
        }
        seen.extend(
            summaries
                .iter()
                .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string()),
        );
        marker = page["NextMarker"].as_str().map(str::to_string);
        assert!(
            marker.is_some(),
            "a non-empty page must carry a marker, got {page}"
        );
    }
    assert_eq!(
        seen,
        vec!["set-a", "set-b", "set-c"],
        "paging must enumerate every set exactly once, got {seen:?}"
    );

    // A `Limit` outside the modelled `PaginationLimit` range (`range 1..100`) is
    // refused rather than clamped, so no caller gets a silently empty terminal
    // page. The range is on the member, so the request boundary is what refuses it,
    // in its own envelope.
    for limit in [0, 101] {
        waf.refused(
            "ListManagedRuleSets",
            json!({ "Scope": SCOPE_REGIONAL, "Limit": limit }),
            "ValidationException",
            &format!("Limit {limit}"),
        )
        .await;
    }
}
