// SPDX-License-Identifier: CC0-1.0

//! The `Scope` is PART of the identity a WAFv2 vendor write resolves before anything its
//! body carries — end to end over the wire.
//!
//! The rule the sibling suite `crate::refusal_precedence` asserts for the rest of a
//! managed rule set's identity is that the TARGET a write addresses is resolved before
//! anything its body refers to, so an identity no set of the account carries is HTTP 400
//! `WAFNonexistentItemException` with WAF's own sentence whatever the body references. That
//! suite's tables vary the `Name`, the `Id`, the body's reference and the lock token, and
//! `crate::managed_rule_sets` varies the same `(Name, Id)` and the publish round-trip; both
//! hold the `Scope` fixed at the namespace that holds the set.
//!
//! This suite covers the one cell they leave out: a `(Name, Id)` the account really carries,
//! asked for in the OTHER namespace. A scope is a namespace, so such a triple names a target
//! the namespace asked does not hold, and it earns the same refusal — which also shows the
//! refusal is not keyed to a `(Name, Id)` nothing has ever minted. The rows cross the two
//! namespaces (each one asked while the other holds the set) with what the body carries
//! (nothing, a version defined by a rule group of the namespace asked, a value that is no
//! ARN) and with both writes
//! that address a set by that triple: `PutManagedRuleSetVersions`, and
//! `UpdateManagedRuleSetVersionExpiryDate`, whose `VersionToExpire` is resolved inside the
//! set the triple names. Every row sends the lock token the held set really carries, so no
//! row can be answered by the token's form either, and the contrast at the end of each
//! namespace is the same triple sent to the namespace that holds it, where the write is
//! served.
//!
//! The one body that is answered on something else is kept beside those rows: a version
//! defined by a rule group of the namespace that HOLDS the set is a reference the namespace
//! asked cannot use at all, judged from the request ahead of the target — see
//! [`crate::managed_rule_set_reference_identity`] for that rule's own table.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK observes. No
//! operation of this model CREATES a managed rule set, so the held set is provisioned through
//! the state seam [`crate::managed_rule_sets::provision_managed_rule_set`].

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    NONEXISTENT_ITEM_MESSAGE, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::managed_rule_sets::provision_managed_rule_set;
use crate::parameter_validation::{error_code, error_message};use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::Waf;

/// A version key inside `VersionKeyString`'s `@length(1..64)` and
/// `@pattern(^[\w#:\.\-/]+$)` (`smithy_query(query="shape",shape="VersionKeyString")`), so
/// the key is never what a row is answered on.
const VERSION: &str = "Version_1.0";

/// A value that is no ARN at all, past `ResourceArn`'s own modelled length floor of 20
/// (`smithy_query(query="shape",shape="ResourceArn")`) so it reaches the service rather than
/// the request boundary.
const NOT_AN_ARN: &str = "not-an-arn-at-all-but-long-enough";

/// The instant an expiry row names, in epoch seconds as the protocol carries a timestamp:
/// any instant will do, the rows turn on the target rather than on it.
const EXPIRY_TIMESTAMP: i64 = 1_800_000_000;

/// The refusal WAF answers for an identity the namespace asked does not hold: 400, the
/// nonexistent-item class, and that class's own fixed sentence.
fn assert_missing_target(status: u16, body: &Value, case: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "WAFNonexistentItemException"),
        "case {case:?}: the namespace is part of the identity the write resolves first, got \
         status {status} and body {body}"
    );
    assert_eq!(
        error_message(body),
        NONEXISTENT_ITEM_MESSAGE,
        "case {case:?}: expected WAF's own sentence, got {body}"
    );
}

/// The other namespace: the two `Scope` values are each other's contrast.
fn other_scope(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        SCOPE_REGIONAL
    } else {
        SCOPE_CLOUDFRONT
    }
}

/// A `(Name, Id)` the account carries only in the OTHER namespace is a target the namespace
/// asked does not hold, for either vendor write and whatever the body carries; the same
/// triple sent to the namespace that holds it is served.
#[tokio::test]
async fn a_set_held_only_in_the_other_namespace_is_a_target_this_one_does_not_hold() {
    for asked in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let holder = other_scope(asked);
        let (waf, engine) = Waf::sharing_engine();
        let (_asked_group_id, group_in_asked, _asked_group_lock) =
            create_rule_group(&waf, asked, "group-of-the-namespace-asked", 100, json!([])).await;
        let (_holder_group_id, group_in_holder, _holder_group_lock) =
            create_rule_group(&waf, holder, "group-of-the-holding-namespace", 200, json!([])).await;
        // The set EXISTS under this exact `(Name, Id)` — in the other namespace.
        let set_name = "cross-namespace-set";
        let (set_id, _set_arn, set_lock) =
            provision_managed_rule_set(&engine, holder, set_name, Some("the vendor's set"));

        let bodies = [
            ("nothing to publish", None),
            (
                "a version defined by a rule group of the namespace asked",
                Some(group_in_asked.clone()),
            ),
            ("a value that is no ARN", Some(NOT_AN_ARN.to_string())),
        ];

        for (publication, reference) in &bodies {
            let case = format!("{asked} asked while {holder} holds the set, {publication}");
            let mut request = json!({
                "Name": set_name,
                "Scope": asked,
                "Id": &set_id,
                "LockToken": &set_lock,
            });
            if let Some(arn) = reference {
                request["RecommendedVersion"] = json!(VERSION);
                request["VersionsToPublish"] = json!({
                    VERSION: { "AssociatedRuleGroupArn": arn, "ForecastedLifetime": 90 },
                });
            }
            let (status, body) = waf.call("PutManagedRuleSetVersions", request).await;
            assert_missing_target(status, &body, &case);
            if let Some(arn) = reference {
                assert!(
                    !body.to_string().contains(arn),
                    "case {case:?}: the answer must name no reference of the body, got {body}"
                );
            }
        }

        // The one body that is NOT answered on the target: a reference of the namespace
        // that HOLDS the set is one the namespace asked cannot use at all — an ARN names
        // the namespace its resource lives in, and both halves of that comparison are in
        // the request — so it is judged ahead of the target lookup and is what the
        // publish is told about. That rule has its own table in
        // `crate::managed_rule_set_reference_identity`; this is the cell this suite
        // reaches.
        let case = format!(
            "{asked} asked while {holder} holds the set, a version defined by a rule \
             group of the holding namespace"
        );
        let (status, body) = waf
            .call(
                "PutManagedRuleSetVersions",
                json!({
                    "Name": set_name,
                    "Scope": asked,
                    "Id": &set_id,
                    "LockToken": &set_lock,
                    "RecommendedVersion": VERSION,
                    "VersionsToPublish": {
                        VERSION: {
                            "AssociatedRuleGroupArn": &group_in_holder,
                            "ForecastedLifetime": 90,
                        },
                    },
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "WAFInvalidParameterException"),
            "case {case:?}: a reference of another namespace is what the publish is told \
             about, got status {status} and body {body}"
        );
        assert_eq!(
            (body["Field"].as_str(), body["Parameter"].as_str()),
            (Some("RESOURCE_ARN"), Some(group_in_holder.as_str())),
            "case {case:?}: expected the refusal to name that reference, got {body}"
        );

        // The other write that addresses a set by the same triple: the version it names to
        // expire is resolved INSIDE that set, so the namespace that does not hold the set is
        // what the caller is told about.
        let case = format!("{asked} asked while {holder} holds the set, expiring a version");
        let (status, body) = waf
            .call(
                "UpdateManagedRuleSetVersionExpiryDate",
                json!({
                    "Name": set_name,
                    "Scope": asked,
                    "Id": &set_id,
                    "LockToken": &set_lock,
                    "VersionToExpire": VERSION,
                    "ExpiryTimestamp": EXPIRY_TIMESTAMP,
                }),
            )
            .await;
        assert_missing_target(status, &body, &case);

        // The namespace asked holds no set under that triple to read, and none of the rows
        // above created one in it.
        let case = format!("{asked} asked while {holder} holds the set, reading it back");
        let (status, body) = waf
            .call(
                "GetManagedRuleSet",
                json!({ "Name": set_name, "Scope": asked, "Id": &set_id }),
            )
            .await;
        assert_missing_target(status, &body, &case);
        let listed = waf
            .ok("ListManagedRuleSets", json!({ "Scope": asked }))
            .await;
        assert_eq!(
            listed["ManagedRuleSets"],
            json!([]),
            "case {asked:?}: a refused write must create nothing in the namespace asked, \
             got {listed}"
        );

        // The contrast: the SAME triple under the SAME token, sent to the namespace that
        // holds the set, is served — which is what shows the rows above turned on the
        // namespace and not on the body or the token.
        let saved = waf
            .ok(
                "PutManagedRuleSetVersions",
                json!({
                    "Name": set_name,
                    "Scope": holder,
                    "Id": &set_id,
                    "LockToken": &set_lock,
                    "RecommendedVersion": VERSION,
                    "VersionsToPublish": {
                        VERSION: {
                            "AssociatedRuleGroupArn": &group_in_holder,
                            "ForecastedLifetime": 90,
                        },
                    },
                }),
            )
            .await;
        let next_lock_token = saved["NextLockToken"]
            .as_str()
            .expect("NextLockToken")
            .to_string();
        assert_ne!(
            next_lock_token, set_lock,
            "case {holder:?}: a publish must rotate the set's token, got {saved}"
        );
        let read = waf
            .ok(
                "GetManagedRuleSet",
                json!({ "Name": set_name, "Scope": holder, "Id": &set_id }),
            )
            .await;
        let offered = &read["ManagedRuleSet"]["PublishedVersions"][VERSION];
        assert_eq!(
            (
                offered["AssociatedRuleGroupArn"].as_str(),
                offered["ForecastedLifetime"].as_i64(),
                read["ManagedRuleSet"]["RecommendedVersion"].as_str(),
            ),
            (Some(group_in_holder.as_str()), Some(90), Some(VERSION)),
            "case {holder:?}: the published version must read back as the put named it, \
             got {read}"
        );
        let expired = waf
            .ok(
                "UpdateManagedRuleSetVersionExpiryDate",
                json!({
                    "Name": set_name,
                    "Scope": holder,
                    "Id": &set_id,
                    "LockToken": &next_lock_token,
                    "VersionToExpire": VERSION,
                    "ExpiryTimestamp": EXPIRY_TIMESTAMP,
                }),
            )
            .await;
        assert_eq!(
            (
                expired["ExpiringVersion"].as_str(),
                expired["ExpiryTimestamp"].as_f64()
            ),
            (Some(VERSION), Some(EXPIRY_TIMESTAMP as f64)),
            "case {holder:?}: an expiry must answer the version and instant it named, \
             got {expired}"
        );
    }
}
