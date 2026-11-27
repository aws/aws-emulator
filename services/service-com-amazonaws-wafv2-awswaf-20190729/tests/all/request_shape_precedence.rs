// SPDX-License-Identifier: CC0-1.0

//! WHEN a value the model CONSTRAINS is judged, relative to everything this service
//! resolves in its store — end to end over the wire.
//!
//! The rule asserted here: a value the model constrains is judged against those
//! constraints before the service resolves anything in its store, because the request
//! boundary runs that walk ahead of every handler. Where the boundary's walk cannot reach
//! the value — a map's KEYS carry the key shape's constraints and the walk visits a map's
//! VALUES — the handler stands in for it, so the refusal is still the validator's own
//! envelope (HTTP 400 `ValidationException`, one clause naming the member's path and the
//! constraint set) and is still answered whatever the request addresses.
//!
//! The first table drives that over `PutManagedRuleSetVersions`, whose `VersionsToPublish`
//! map is keyed by `VersionKeyString`: the rows cross WHICH constraint of that shape the
//! key breaks with every way the put's target can fail to resolve (the name, the id, both,
//! the namespace asked), with what the body references (a rule group that exists, a
//! well-formed ARN naming none, a value that is no ARN, no reference at all) and with the
//! token the request carries (the set's own, one no set carries, one outside `LockToken`'s
//! form) — so an answer keyed to a target, a reference or a token would show up as a
//! difference from the rest. The blocks after it are the contrast that pins the rest of
//! the order: with every KEY inside the modelled form, the same requests are told about
//! the store — the missing target first, then a reference WAF cannot use, then the stale
//! lock — and the request with nothing wrong is served and reads back.
//!
//! The second table shows the rule is the service's and not this operation's: the sibling
//! updates judge the `Description` their request carries the same way, ahead of resolving
//! the entity they address, so a localized description against an entity nothing holds is
//! the validator's refusal and the same request with an ASCII description is told about
//! the entity.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness in
//! [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK observes. No
//! operation of this model CREATES a managed rule set, so the held set is provisioned
//! through the state seam [`crate::managed_rule_sets::provision_managed_rule_set`].

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    resource_arn, NONEXISTENT_ITEM_MESSAGE, RULE_GROUP_ARN_KIND, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::managed_rule_sets::{
    get_managed_rule_set, provision_managed_rule_set, vendor_rule_groups, version_to_publish,
};
use crate::parameter_validation::{error_code, error_message};
use crate::web_acl_lifecycle::{visibility, Waf, TEST_REGION};

/// `VersionKeyString`'s constraints as one bracketed set, in the order WAF's request
/// validator lists them: the length ceiling, the length floor, then the pattern
/// (`smithy_query(query="shape",shape="VersionKeyString")` gives `@length(1..64)` and
/// `@pattern(^[\w#:\.\-/]+$)`). Written out here rather than imported, so the table pins
/// the clause and not merely the service's reuse of its own constant.
const VERSION_KEY_CONSTRAINTS: &str = "Member must have length less than or equal to 64, \
     Member must have length greater than or equal to 1, \
     Member must satisfy regular expression pattern: ^[\\w#:\\.\\-/]+$";

/// An `EntityId`/`LockToken`-shaped value no resource below carries.
const FOREIGN_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// A second one, for a row that has to get two identities wrong at once.
const OTHER_FOREIGN_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// A value outside `LockToken`'s `@pattern`, whose own modelled constraint this
/// operation judges LAST (`skip_validation` on its input).
const MALFORMED_LOCK_TOKEN: &str = "not-a-real-lock-token";

/// A value that is no ARN at all, past `ResourceArn`'s modelled length floor of 20
/// (`smithy_query(query="shape",shape="ResourceArn")`) so it reaches the service rather
/// than the request boundary.
const NOT_AN_ARN: &str = "not-an-arn-at-all-but-long-enough";

/// A version key inside `VersionKeyString`'s form, for the contrast blocks.
const LEGAL_VERSION: &str = "Version_1.0";

/// The version keys the table refuses, one per way `VersionKeyString`'s constraints can
/// be broken: the `@pattern` (two different characters outside its class), the `@length`
/// ceiling and the `@length` floor.
fn illegal_version_keys() -> Vec<(&'static str, String)> {
    vec![
        (
            "a key carrying spaces and an exclamation mark",
            "not a valid version!!".to_string(),
        ),
        ("a key carrying a dollar sign", "v1.0$".to_string()),
        (
            "a key one character past the length ceiling",
            "v".repeat(65),
        ),
        ("the empty key, below the length floor", String::new()),
    ]
}

/// The refusal a map-key constraint failure earns: the validator's envelope, naming the
/// map member's own path, the key shape's whole constraint set, and — as every clause
/// about a member does — echoing that MEMBER's value, which for a map is the whole map.
fn assert_version_key_refusal(status: u16, body: &Value, rendered_map: &str, case: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: a constrained value is judged before anything is resolved, got \
         status {status} and body {body}"
    );
    assert_eq!(
        error_message(body),
        format!(
            "1 validation error detected: Value '{rendered_map}' at 'versionsToPublish' \
             failed to satisfy constraint: Map keys must satisfy constraint: \
             [{VERSION_KEY_CONSTRAINTS}]"
        ),
        "case {case:?}: the clause must echo the map member's value and name the key \
         shape's constraint set, got {body}"
    );
    assert!(
        body.get("Field").is_none() && body.get("Reason").is_none(),
        "case {case:?}: a request-shape failure carries no parameter members, got {body}"
    );
}

/// One `VersionsToPublish` entry as the request sends it, and as a validation clause
/// renders it: `VersionToPublish(associatedRuleGroupArn=…, forecastedLifetime=…)` over the
/// members in the order the model declares them
/// (`smithy_query(query="shape",shape="VersionToPublish")`), each member the request left
/// out rendered as null.
fn entry(reference: Option<&str>, forecasted_lifetime: Option<i64>) -> (Value, String) {
    let mut sent = json!({});
    if let Some(arn) = reference {
        sent["AssociatedRuleGroupArn"] = json!(arn);
    }
    if let Some(days) = forecasted_lifetime {
        sent["ForecastedLifetime"] = json!(days);
    }
    let rendered = format!(
        "VersionToPublish(associatedRuleGroupArn={}, forecastedLifetime={})",
        reference.unwrap_or("null"),
        forecasted_lifetime.map_or("null".to_string(), |days| days.to_string()),
    );
    (sent, rendered)
}

/// A `VersionsToPublish` KEY outside `VersionKeyString`'s form is the validator's own
/// refusal whatever the put addresses, references or carries as its token; with the keys
/// inside that form, the same requests are told about the store, in the order WAF reads
/// it.
#[tokio::test]
async fn a_constrained_map_key_is_judged_before_the_put_resolves_anything() {
    let (waf, engine) = Waf::sharing_engine();
    let rule_groups = vendor_rule_groups(&waf, SCOPE_REGIONAL).await;
    let usable_group = rule_groups[0].clone();
    // A well-formed rule-group ARN of this namespace that names no rule group: WAF needs
    // the store to tell it apart from one that does.
    let absent_group = resource_arn(
        SCOPE_REGIONAL,
        TEST_REGION,
        RULE_GROUP_ARN_KIND,
        "never-created-group",
        OTHER_FOREIGN_UUID,
    );
    let set_name = "vendor-set";
    let (set_id, _set_arn, set_lock) =
        provision_managed_rule_set(&engine, SCOPE_REGIONAL, set_name, None);

    // Every way the target of the put can fail to resolve, and the one that resolves.
    let targets: [(&str, &str, &str, &str); 5] = [
        (
            "the set the account holds",
            SCOPE_REGIONAL,
            set_name,
            set_id.as_str(),
        ),
        (
            "a name no set carries",
            SCOPE_REGIONAL,
            "never-provisioned",
            set_id.as_str(),
        ),
        ("an id no set carries", SCOPE_REGIONAL, set_name, FOREIGN_UUID),
        (
            "a name and an id no set carries",
            SCOPE_REGIONAL,
            "never-provisioned",
            FOREIGN_UUID,
        ),
        // A scope is a namespace, so the set the account holds regionally is not held in
        // the global one.
        (
            "the other namespace",
            SCOPE_CLOUDFRONT,
            set_name,
            set_id.as_str(),
        ),
    ];

    // What the body publishes a version by, paired with the token the request carries, so
    // each row of the table differs in both as well as in its key and its target — and so
    // the clause's echoed value is rendered over a reference and a lifetime the request
    // sent as well as over members it left out.
    let bodies: [(&str, Option<&str>, Option<i64>, &str); 4] = [
        (
            "a reference to a rule group that exists",
            Some(usable_group.as_str()),
            Some(90),
            set_lock.as_str(),
        ),
        (
            "a well-formed reference naming no rule group",
            Some(absent_group.as_str()),
            Some(1),
            FOREIGN_UUID,
        ),
        (
            "a value that is no ARN",
            Some(NOT_AN_ARN),
            None,
            MALFORMED_LOCK_TOKEN,
        ),
        ("no reference at all", None, None, MALFORMED_LOCK_TOKEN),
    ];

    for (key_case, version) in illegal_version_keys() {
        for (target, scope, name, id) in targets {
            for (body_case, reference, forecasted_lifetime, token) in bodies {
                let case =
                    format!("{key_case}, against {target}, with {body_case} and token {token:?}");
                let (sent, rendered) = entry(reference, forecasted_lifetime);
                let (status, answer) = waf
                    .call(
                        "PutManagedRuleSetVersions",
                        json!({
                            "Name": name,
                            "Scope": scope,
                            "Id": id,
                            "LockToken": token,
                            "VersionsToPublish": { version.as_str(): sent },
                        }),
                    )
                    .await;
                assert_version_key_refusal(
                    status,
                    &answer,
                    &format!("{{{version}={rendered}}}"),
                    &case,
                );
            }
        }
    }

    // A map carrying SEVERAL entries renders every one of them, in key order, under the
    // one clause the single offending key earns.
    let (first_sent, first_rendered) = entry(None, Some(90));
    let (second_sent, second_rendered) = entry(None, Some(30));
    // Sorts after `LEGAL_VERSION`, and outside `VersionKeyString`'s pattern.
    let illegal_version = "z version!!";
    let (status, answer) = waf
        .call(
            "PutManagedRuleSetVersions",
            json!({
                "Name": set_name,
                "Scope": SCOPE_REGIONAL,
                "Id": &set_id,
                "LockToken": &set_lock,
                "VersionsToPublish": {
                    LEGAL_VERSION: first_sent,
                    illegal_version: second_sent,
                },
            }),
        )
        .await;
    assert_version_key_refusal(
        status,
        &answer,
        &format!("{{{LEGAL_VERSION}={first_rendered}, {illegal_version}={second_rendered}}}"),
        "one illegal key beside a legal one",
    );

    // Nothing the table sent published anything, and the set still carries the token it
    // was provisioned with: the ordering refuses bad input, it does not write.
    let held = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    assert_eq!(
        (
            &held["ManagedRuleSet"]["PublishedVersions"],
            held["LockToken"].as_str(),
        ),
        (&json!({}), Some(set_lock.as_str())),
        "a refused put must publish nothing and rotate no token, got {held}"
    );

    // The contrast: with every key inside the modelled form, the same requests are told
    // about the store. The target that does not resolve comes first...
    for (target, scope, name, id) in targets.into_iter().skip(1) {
        for (body_case, reference, forecasted_lifetime, token) in bodies {
            let case = format!("a legal key, against {target}, with {body_case}");
            let (sent, _rendered) = entry(reference, forecasted_lifetime);
            let (status, answer) = waf
                .call(
                    "PutManagedRuleSetVersions",
                    json!({
                        "Name": name,
                        "Scope": scope,
                        "Id": id,
                        "LockToken": token,
                        "VersionsToPublish": { LEGAL_VERSION: sent },
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "WAFNonexistentItemException"),
                "case {case:?}: a request that validates is told about the target it \
                 addresses, got status {status} and body {answer}"
            );
            assert_eq!(
                error_message(&answer),
                NONEXISTENT_ITEM_MESSAGE,
                "case {case:?}: expected WAF's own sentence, got {answer}"
            );
        }
    }

    // ...then, against the set the account holds, a reference WAF cannot use...
    for (body_case, reference) in [
        ("a value that is no ARN", NOT_AN_ARN),
        (
            "a well-formed reference naming no rule group",
            absent_group.as_str(),
        ),
    ] {
        let case = format!("a legal key, against the set the account holds, with {body_case}");
        let (status, answer) = waf
            .call(
                "PutManagedRuleSetVersions",
                json!({
                    "Name": set_name,
                    "Scope": SCOPE_REGIONAL,
                    "Id": &set_id,
                    "LockToken": &set_lock,
                    "VersionsToPublish": { LEGAL_VERSION: version_to_publish(reference, 90) },
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "WAFInvalidParameterException"),
            "case {case:?}: a reference WAF cannot use is reported once the target \
             resolves, got status {status} and body {answer}"
        );
    }

    // ...then the token, whose own modelled constraint this operation judges last...
    for token in [MALFORMED_LOCK_TOKEN, FOREIGN_UUID] {
        let case = format!("a legal key and a usable reference, with token {token:?}");
        let (status, answer) = waf
            .call(
                "PutManagedRuleSetVersions",
                json!({
                    "Name": set_name,
                    "Scope": SCOPE_REGIONAL,
                    "Id": &set_id,
                    "LockToken": token,
                    "VersionsToPublish": {
                        LEGAL_VERSION: version_to_publish(&usable_group, 90),
                    },
                }),
            )
            .await;
        let expected = if token == MALFORMED_LOCK_TOKEN {
            "ValidationException"
        } else {
            "WAFOptimisticLockException"
        };
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, expected),
            "case {case:?}: the token is judged last, got status {status} and body {answer}"
        );
    }

    // ...and the request with nothing wrong is served and reads back, which is what shows
    // the rows above refused bad input rather than input.
    let saved = waf
        .ok(
            "PutManagedRuleSetVersions",
            json!({
                "Name": set_name,
                "Scope": SCOPE_REGIONAL,
                "Id": &set_id,
                "LockToken": &set_lock,
                "RecommendedVersion": LEGAL_VERSION,
                "VersionsToPublish": {
                    LEGAL_VERSION: version_to_publish(&usable_group, 90),
                },
            }),
        )
        .await;
    assert!(
        saved["NextLockToken"]
            .as_str()
            .is_some_and(|token| token != set_lock),
        "a served put must rotate the set's token, got {saved}"
    );
    let read = get_managed_rule_set(&waf, SCOPE_REGIONAL, set_name, &set_id).await;
    let offered = &read["ManagedRuleSet"]["PublishedVersions"][LEGAL_VERSION];
    assert_eq!(
        (
            offered["AssociatedRuleGroupArn"].as_str(),
            offered["ForecastedLifetime"].as_i64(),
            read["ManagedRuleSet"]["RecommendedVersion"].as_str(),
        ),
        (Some(usable_group.as_str()), Some(90), Some(LEGAL_VERSION)),
        "the published version must read back as the put named it, got {read}"
    );
}

/// One update that describes the entity it addresses: the operation, and the members
/// that update requires besides the ones every row sends.
struct DescribedUpdate {
    operation: &'static str,
    members: fn(&str) -> Value,
}

/// Every update whose request carries an `EntityDescription`, with the rest of the body
/// each one requires (`smithy_query(query="operation",operation="UpdateIPSet,UpdateRuleGroup,UpdateRegexPatternSet,UpdateWebACL")`).
const DESCRIBED_UPDATES: &[DescribedUpdate] = &[
    DescribedUpdate {
        operation: "UpdateIPSet",
        members: |_name| json!({ "Addresses": ["10.0.0.0/8"] }),
    },
    DescribedUpdate {
        operation: "UpdateRegexPatternSet",
        members: |_name| json!({ "RegularExpressionList": [{ "RegexString": "^/api" }] }),
    },
    DescribedUpdate {
        operation: "UpdateRuleGroup",
        members: |name| json!({ "Rules": [], "VisibilityConfig": visibility(name) }),
    },
    DescribedUpdate {
        operation: "UpdateWebACL",
        members: |name| {
            json!({
                "DefaultAction": { "Allow": {} },
                "Rules": [],
                "VisibilityConfig": visibility(name),
            })
        },
    },
];

/// The same rule, one level up: every update that carries a `Description` judges that
/// constrained value before it resolves the entity it addresses, so an entity nothing
/// holds is told about only once the request itself validates.
#[tokio::test]
async fn a_constrained_description_is_judged_before_the_update_resolves_its_entity() {
    let waf = Waf::new();
    // Outside `EntityDescription`'s `@pattern`, whose character classes are the ASCII
    // ones a Smithy pattern is read with
    // (`smithy_query(query="shape",shape="EntityDescription")`); see
    // `crate::entity_description_form` for the whole table of forms.
    let past_the_length_bound = "d".repeat(257);
    let illegal_descriptions = [
        "описание-1",
        "blocked 🎉 requests",
        past_the_length_bound.as_str(),
    ];

    for update in DESCRIBED_UPDATES {
        let name = "never-created-entity";
        for description in illegal_descriptions {
            let case = format!("{}: {description:?}", update.operation);
            let mut request = json!({
                "Name": name,
                "Scope": SCOPE_REGIONAL,
                "Id": FOREIGN_UUID,
                "LockToken": FOREIGN_UUID,
                "Description": description,
            });
            let Value::Object(members) = (update.members)(name) else {
                unreachable!("every row's members are an object");
            };
            for (member, value) in members {
                request[member] = value;
            }
            let (status, answer) = waf.call(update.operation, request).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a constrained value is judged before the entity is \
                 resolved, got status {status} and body {answer}"
            );
            assert!(
                error_message(&answer).contains("at 'description'"),
                "case {case:?}: the clause must name the member that failed, got {answer}"
            );
        }

        // With a description inside the modelled form, the same request is told about the
        // entity it addresses.
        let case = format!("{}: an ASCII description", update.operation);
        let mut request = json!({
            "Name": name,
            "Scope": SCOPE_REGIONAL,
            "Id": FOREIGN_UUID,
            "LockToken": FOREIGN_UUID,
            "Description": "desc-01 v1.0 test_env",
        });
        let Value::Object(members) = (update.members)(name) else {
            unreachable!("every row's members are an object");
        };
        for (member, value) in members {
            request[member] = value;
        }
        let (status, answer) = waf.call(update.operation, request).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "WAFNonexistentItemException"),
            "case {case:?}: a request that validates is told about the entity it \
             addresses, got status {status} and body {answer}"
        );
    }
}
