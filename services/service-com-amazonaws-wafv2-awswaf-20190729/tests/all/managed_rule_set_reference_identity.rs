// SPDX-License-Identifier: CC0-1.0

//! The NAMESPACE a reference a vendor's publish carries belongs to — part of the
//! identity WAF reads off that reference before it resolves anything — end to end over
//! the wire.
//!
//! The rule asserted here: a reference is an ARN, and an ARN names the region whose
//! endpoint serves the resource and the namespace its `Scope` names. A WAFv2 resource
//! lives in exactly that namespace and an endpoint cannot act on another one, so an
//! `AssociatedRuleGroupArn` naming a namespace other than the one the request addresses
//! is a reference this call cannot use: HTTP 400 `WAFInvalidParameterException` with
//! `Field` RESOURCE_ARN, that reference as `Parameter`, the ARN sentence as `Reason` and
//! the message composed from those three. Both halves of the comparison come from the
//! REQUEST — the ARN it carried and the `Scope` it named at the endpoint it reached — so
//! the judgement needs no store and precedes every store read, which is why the same
//! refusal comes back whether or not the managed rule set the publish targets is there.
//! It is the judgement `GetWebACL` already makes about the ARN it addresses a web ACL by
//! (asserted for that operation in [`crate::refusal_precedence`]).
//!
//! A reference of the namespace the request DOES address is resolved in the order WAF
//! resolves things, which the second suite here and the sibling
//! [`crate::refusal_precedence`] assert: the target the call addresses first (an identity
//! no set of the account carries is `WAFNonexistentItemException` whatever the body
//! references), then whether the reference resolves to a rule group of that namespace.
//!
//! The table spans what the rule generalises over: the namespace asked, whether the
//! targeted set is held, HOW the reference leaves that namespace (another region with the
//! same namespace segment, the other namespace at the region that serves it, the other
//! namespace at this request's own region), and whether such a reference is the only
//! version published or one of two beside a reference that resolves.
//!
//! No operation of this model CREATES a managed rule set, so the held set is provisioned
//! through the state seam [`crate::managed_rule_sets::provision_managed_rule_set`]; every
//! assertion is driven through the router's front door (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), the same door an SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    resource_arn, DEFAULT_REGION, RULE_GROUP_ARN_KIND, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::managed_rule_sets::provision_managed_rule_set;
use crate::parameter_validation::{error_code, error_message};
use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{Waf, TEST_REGION};

/// The `ParameterExceptionField` token naming a resource ARN
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists RESOURCE_ARN
/// among its values). Spelled out here, so this suite pins the token rather than the
/// service's reuse of its own constant.
const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `Reason` WAF carries for a reference it will not accept, as recorded. Spelled out
/// here for the same reason as [`FIELD_RESOURCE_ARN`].
const ARN_NOT_VALID: &str = "The ARN isn't valid. A valid ARN begins with arn: and \
     includes other information separated by colons or slashes.";

/// The fixed sentence `WAFNonexistentItemException` carries, as recorded (note the
/// typographic apostrophes).
const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// A region no row's request is signed for and no namespace here lives in.
const ANOTHER_REGION: &str = "eu-central-1";

/// A version key inside `VersionKeyString`'s `@length(1..64)` and
/// `@pattern(^[\w#:\.\-/]+$)`, so no row is ever answered on its key.
const VERSION: &str = "Version_1.0";

/// A second version key, for the rows that publish two versions at once.
const OTHER_VERSION: &str = "Version_2.0";

/// A `LockToken`-shaped token, for the rows whose target is an identity nothing holds.
const WELL_FORMED_LOCK: &str = "11111111-2222-3333-4444-555555555555";

/// An `EntityId`-shaped identifier nothing minted.
const UNMINTED_IDENTIFIER: &str = "11111111-2222-3333-4444-555555555555";

/// The other namespace: the two `Scope` values are each other's contrast.
fn other_scope(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        SCOPE_REGIONAL
    } else {
        SCOPE_CLOUDFRONT
    }
}

/// The region the namespace `scope` lives in, as this harness signs for it: the one
/// endpoint that serves the global namespace, and this harness's own region for the
/// regional one.
fn namespace_region(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        DEFAULT_REGION
    } else {
        TEST_REGION
    }
}

/// A rule-group reference in `region` carrying `namespace`'s own namespace segment.
///
/// The layout is the one `crate::state::resource_arn` mints
/// (`arn:aws:wafv2:<region>:<account>:<global|regional>/rulegroup/<name>/<id>`), with the
/// region and the namespace segment a row needs rather than the pair the request's own
/// `Scope` and endpoint imply — that pairing is exactly what these rows vary, so the
/// region is substituted into the minted form.
fn reference(namespace: &str, region: &str, name: &str, id: &str) -> String {
    let minted = resource_arn(
        namespace,
        namespace_region(namespace),
        RULE_GROUP_ARN_KIND,
        name,
        id,
    );
    minted.replace(
        &format!(":{}:", namespace_region(namespace)),
        &format!(":{region}:"),
    )
}

/// A `PutManagedRuleSetVersions` body publishing `versions`, against
/// `(scope, name, id)` under `lock_token`.
fn publish(scope: &str, name: &str, id: &str, lock_token: &str, versions: Value) -> Value {
    json!({
        "Name": name,
        "Scope": scope,
        "Id": id,
        "LockToken": lock_token,
        "RecommendedVersion": VERSION,
        "VersionsToPublish": versions,
    })
}

/// One published version defined by `arn`.
fn one_version(arn: &str) -> Value {
    json!({ VERSION: { "AssociatedRuleGroupArn": arn, "ForecastedLifetime": 90 } })
}

/// The refusal WAF answers a reference it will not accept: 400, the invalid-parameter
/// class, `Field` RESOURCE_ARN, the reference itself as `Parameter`, the ARN sentence as
/// `Reason`, and the message composed from exactly those three.
fn assert_unusable_reference(status: u16, body: &Value, expected: &str, case: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "WAFInvalidParameterException"),
        "case {case:?}: a reference of another namespace is the invalid-parameter \
         refusal, got status {status} and body {body}"
    );
    assert_eq!(
        (
            body["Field"].as_str(),
            body["Parameter"].as_str(),
            body["Reason"].as_str(),
        ),
        (Some(FIELD_RESOURCE_ARN), Some(expected), Some(ARN_NOT_VALID)),
        "case {case:?}: expected the structured refusal naming {expected:?}, got {body}"
    );
    assert_eq!(
        error_message(body),
        format!(
            "Error reason: {ARN_NOT_VALID}, field: {FIELD_RESOURCE_ARN}, parameter: {expected}"
        ),
        "case {case:?}: the message is composed from this answer's own Field, \
         Parameter and Reason, got {body}"
    );
}

/// A publish whose `AssociatedRuleGroupArn` names another namespace is refused as the
/// unusable reference it is, before anything is resolved — so the same refusal comes back
/// whether the managed rule set it targets is held or not, and nothing is published
/// either way.
///
/// The rows cross the namespace asked, whether the target is held, the three ways a
/// reference can name another namespace, and whether that reference is the only version
/// published or one of two.
#[tokio::test]
async fn a_reference_of_another_namespace_is_refused_before_anything_is_resolved() {
    for asked in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let (waf, engine) = Waf::sharing_engine();
        let (_group_id, group_arn, _group_lock) =
            create_rule_group(&waf, asked, "vendor-group", 100, json!([])).await;
        let set_name = "vendor-set";
        let (set_id, _set_arn, set_lock) =
            provision_managed_rule_set(&engine, asked, set_name, Some("the vendor's set"));

        // The two targets: the set the account holds, and an identity it does not.
        let targets = [
            (
                "the set the account holds",
                set_name.to_string(),
                set_id.clone(),
                set_lock.clone(),
            ),
            (
                "an identity no set carries",
                format!("{set_name}-absent"),
                UNMINTED_IDENTIFIER.to_string(),
                WELL_FORMED_LOCK.to_string(),
            ),
        ];
        // The three ways a reference leaves the namespace the request addresses.
        let elsewhere = [
            (
                "this namespace at another region",
                reference(asked, ANOTHER_REGION, "vendor-group", UNMINTED_IDENTIFIER),
            ),
            (
                "the other namespace at the region that serves it",
                reference(
                    other_scope(asked),
                    namespace_region(other_scope(asked)),
                    "vendor-group",
                    UNMINTED_IDENTIFIER,
                ),
            ),
            (
                "the other namespace at this request's own region",
                reference(
                    other_scope(asked),
                    namespace_region(asked),
                    "vendor-group",
                    UNMINTED_IDENTIFIER,
                ),
            ),
        ];

        for (target, name, id, lock) in &targets {
            for (how, unusable) in &elsewhere {
                let case = format!("{asked} asked, {target}, a reference naming {how}");
                let (status, body) = waf
                    .call(
                        "PutManagedRuleSetVersions",
                        publish(asked, name, id, lock, one_version(unusable)),
                    )
                    .await;
                assert_unusable_reference(status, &body, unusable, &case);

                // The same reference beside one of this namespace that DOES resolve:
                // the publish is still refused on the one it cannot use.
                let case = format!("{case}, beside a reference that resolves");
                let (status, body) = waf
                    .call(
                        "PutManagedRuleSetVersions",
                        publish(
                            asked,
                            name,
                            id,
                            lock,
                            json!({
                                VERSION: { "AssociatedRuleGroupArn": &group_arn },
                                OTHER_VERSION: { "AssociatedRuleGroupArn": unusable },
                            }),
                        ),
                    )
                    .await;
                assert_unusable_reference(status, &body, unusable, &case);
            }
        }

        // Nothing above published anything: the held set still offers no version and
        // still carries the token it was provisioned with.
        let read = waf
            .ok(
                "GetManagedRuleSet",
                json!({ "Name": set_name, "Scope": asked, "Id": &set_id }),
            )
            .await;
        assert_eq!(
            (
                &read["ManagedRuleSet"]["PublishedVersions"],
                read["LockToken"].as_str(),
            ),
            (&json!({}), Some(set_lock.as_str())),
            "case {asked:?}: a refused publish must publish nothing and rotate no \
             token, got {read}"
        );
        let listed = waf
            .ok("ListManagedRuleSets", json!({ "Scope": asked }))
            .await;
        let names: Vec<&str> = listed["ManagedRuleSets"]
            .as_array()
            .map(|sets| {
                sets.iter()
                    .filter_map(|summary| summary["Name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec![set_name],
            "case {asked:?}: a refused publish must leave the account's sets alone, \
             got {listed}"
        );
    }
}

/// A reference of the namespace the request DOES address is resolved in the order WAF
/// resolves things, which is what shows the rows above turn on the namespace the
/// reference names and not on the store: against an identity no set carries the missing
/// TARGET is reported, against the held set a reference of this namespace that resolves
/// to nothing is reported as the bad parameter it is, and a publish whose reference
/// resolves is served and reads back.
///
/// The rows cross the namespace asked with each kind of reference of that namespace — a
/// rule group that exists, and a well-formed one nothing minted.
#[tokio::test]
async fn a_reference_of_this_namespace_is_resolved_after_the_target() {
    for asked in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let (waf, engine) = Waf::sharing_engine();
        let (_group_id, group_arn, _group_lock) =
            create_rule_group(&waf, asked, "vendor-group", 100, json!([])).await;
        let set_name = "vendor-set";
        let (set_id, _set_arn, set_lock) =
            provision_managed_rule_set(&engine, asked, set_name, Some("the vendor's set"));
        let unminted = reference(
            asked,
            namespace_region(asked),
            "never-created-group",
            UNMINTED_IDENTIFIER,
        );

        // Against an identity no set carries, a reference of this namespace leaves the
        // missing target as what the publish is told about.
        for (case, arn) in [
            ("a rule group that exists", group_arn.clone()),
            ("a reference nothing minted", unminted.clone()),
        ] {
            let case = format!("{asked} asked, an identity no set carries, {case}");
            let (status, body) = waf
                .call(
                    "PutManagedRuleSetVersions",
                    publish(
                        asked,
                        &format!("{set_name}-absent"),
                        UNMINTED_IDENTIFIER,
                        WELL_FORMED_LOCK,
                        one_version(&arn),
                    ),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "WAFNonexistentItemException"),
                "case {case:?}: the target is resolved before the body's references, \
                 got status {status} and body {body}"
            );
            assert_eq!(
                error_message(&body),
                NONEXISTENT_ITEM,
                "case {case:?}: expected WAF's own sentence, got {body}"
            );
        }

        // Against the held set, a reference of this namespace that resolves to nothing
        // is reported as the bad parameter it is.
        let case = format!("{asked} asked, the held set, a reference nothing minted");
        let (status, body) = waf
            .call(
                "PutManagedRuleSetVersions",
                publish(asked, set_name, &set_id, &set_lock, one_version(&unminted)),
            )
            .await;
        assert_unusable_reference(status, &body, &unminted, &case);

        // And a publish whose reference resolves is served, and reads back.
        let published = waf
            .ok(
                "PutManagedRuleSetVersions",
                publish(asked, set_name, &set_id, &set_lock, one_version(&group_arn)),
            )
            .await;
        let next_lock_token = published["NextLockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            next_lock_token, set_lock,
            "case {asked:?}: a publish must rotate the set's token, got {published}"
        );
        let read = waf
            .ok(
                "GetManagedRuleSet",
                json!({ "Name": set_name, "Scope": asked, "Id": &set_id }),
            )
            .await;
        assert_eq!(
            read["ManagedRuleSet"]["PublishedVersions"][VERSION]["AssociatedRuleGroupArn"]
                .as_str(),
            Some(group_arn.as_str()),
            "case {asked:?}: the published version must read back as the publish \
             named it, got {read}"
        );
    }
}
