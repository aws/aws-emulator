// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateIdentityPool`, the mutation that reconfigures a stored
//! identity pool.
//!
//! The RULES every case asserts, over a table of inputs rather than one recorded call:
//!
//! * An update REPLACES the pool's configuration with the one the request carries — "if
//!   you don't provide a value for a parameter, Amazon Cognito sets it to its default
//!   value" (this service's model) — so a member an earlier call set and this one omits is
//!   gone afterwards, not carried over. What the update answers is what the pool's readers
//!   report, and the pool keeps the id it was minted with.
//! * A pool's TAGS are the exception, because they are not part of that configuration:
//!   they are the facet this service's tagging operations own, so an update that carries
//!   no `IdentityPoolTags` leaves the tag set the pool holds, and one that carries the
//!   member states the pool's whole tag set.
//! * The pool's developer-provider domain is the other exception, for the opposite reason:
//!   it is settable once and fixed from then on, which
//!   `developer_provider_name_immutability` covers on its own.
//! * An update naming a pool this service does not hold — one never created, or one
//!   already deleted — is the modelled `ResourceNotFoundException`, and writes nothing:
//!   every other pool keeps the configuration it had.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::identity_pool_arn;

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};
use crate::resource_tags::{listed_tags, tag, tag_map, tagged_pool};

/// The operation under test, spelled once.
const OPERATION: &str = "UpdateIdentityPool";

/// Every OPTIONAL CONFIGURATION member `UpdateIdentityPoolInput` declares, from
/// `smithy_query(query="operation", operation="UpdateIdentityPool")`. A row that omits one
/// must leave the stored pool without it, which is what makes this catalog the thing to
/// check after each update. Two members the operation declares are deliberately NOT here,
/// because neither is configuration this call replaces: `IdentityPoolTags`, which
/// `an_update_replaces_the_tag_set_only_when_it_carries_one` covers, and
/// `DeveloperProviderName`, which cannot be changed once set and which
/// `developer_provider_name_immutability` covers.
const OPTIONAL_MEMBERS: [&str; 5] = [
    "AllowClassicFlow",
    "SupportedLoginProviders",
    "OpenIdConnectProviderARNs",
    "CognitoIdentityProviders",
    "SamlProviderARNs",
];

/// An update request for `identity_pool_id` carrying `configuration`'s members.
fn update_request(identity_pool_id: &str, configuration: &Value) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolId".to_string(), json!(identity_pool_id));
    for (member, value) in configuration
        .as_object()
        .expect("a configuration row is an object")
    {
        request.insert(member.clone(), value.clone());
    }
    Value::Object(request)
}

/// One update per shape of configuration the model allows, in the order they are applied
/// to the one pool: each row both sets what it carries and drops what the row before it
/// set, so the replace rule is asserted over every member of the catalog.
fn configurations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "every member this call replaces",
            json!({
                "IdentityPoolName": "fully-configured",
                "AllowUnauthenticatedIdentities": true,
                "AllowClassicFlow": true,
                "SupportedLoginProviders": {
                    "graph.facebook.com": "1234567890",
                    "accounts.google.com": "client.apps.googleusercontent.com",
                },
                // The developer-provider domain is left out of every row of this table:
                // it is settable once, so a row could neither set it after another row
                // had nor have it dropped by the row after it
                // (`developer_provider_name_immutability` is where it is asserted).
                "OpenIdConnectProviderARNs": [
                    "arn:aws:iam::000000000000:oidc-provider/auth.example.com",
                ],
                "CognitoIdentityProviders": [{
                    "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_abcd1234",
                    "ClientId": "1h57kf5cpq17m0eml12eqpq8m2",
                    "ServerSideTokenCheck": true,
                }],
                "SamlProviderARNs": ["arn:aws:iam::000000000000:saml-provider/Example"],
                "IdentityPoolTags": {"Env": "test", "Owner": "identity-team"},
            }),
        ),
        (
            "only the required members, which drops every other member",
            json!({
                "IdentityPoolName": "stripped-down",
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
        (
            "classic flow turned off and one login provider",
            json!({
                "IdentityPoolName": "classic-off",
                "AllowUnauthenticatedIdentities": false,
                "AllowClassicFlow": false,
                "SupportedLoginProviders": {"graph.facebook.com": "9876543210"},
            }),
        ),
        (
            "an empty supported-login-providers map (the modelled minimum)",
            json!({
                "IdentityPoolName": "no-providers",
                "AllowUnauthenticatedIdentities": false,
                "SupportedLoginProviders": {},
            }),
        ),
        (
            "tags only",
            json!({
                "IdentityPoolName": "tagged",
                "AllowUnauthenticatedIdentities": true,
                "IdentityPoolTags": {"Env": "prod"},
            }),
        ),
        (
            "a name of the maximum modelled length",
            json!({
                "IdentityPoolName": "n".repeat(128),
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
        (
            "a name of the minimum modelled length",
            json!({"IdentityPoolName": "a", "AllowUnauthenticatedIdentities": true}),
        ),
    ]
}

/// RULE: an update replaces the pool's configuration with the request's, the readers
/// report exactly what it wrote, and the pool keeps its id.
#[tokio::test]
async fn an_update_replaces_the_stored_configuration() {
    let emulator = Emulator::new();
    let identity_pool_id = emulator.pool("before-any-update", false).await;
    // A second pool, never updated, so every row can also show the update touched only
    // the pool it named.
    let untouched_id = emulator.pool("untouched", true).await;
    let untouched_before = emulator
        .ok(
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &untouched_id}),
        )
        .await;

    for (case, configuration) in configurations() {
        let request = update_request(&identity_pool_id, &configuration);
        let updated = emulator.ok(OPERATION, &request).await;

        // Every member the request sent comes back with the value it was sent with, and
        // the id is the one the pool was minted with.
        for (member, sent) in request.as_object().expect("the request is an object") {
            // With ONE exception: an explicitly empty `SupportedLoginProviders` configures
            // no login provider, and AWS answers the pool the same way whether the member
            // was sent empty or not sent at all, so the member is ABSENT rather than echoed
            // as an empty map (`pool_configuration_rules` asserts that rule in full).
            if member == "SupportedLoginProviders" && sent.as_object().is_some_and(Map::is_empty) {
                assert!(
                    updated[member].is_null(),
                    "case {case:?}: an explicitly empty SupportedLoginProviders must leave the \
                     member out, got {updated}"
                );
                continue;
            }
            assert_eq!(
                &updated[member], sent,
                "case {case:?}: the update answered {member} = {}, expected {sent}",
                updated[member]
            );
        }

        // A member this row did not send is absent from the pool afterwards: an update
        // replaces the configuration rather than merging into it.
        for member in OPTIONAL_MEMBERS {
            if request.get(member).is_some() {
                continue;
            }
            assert!(
                updated[member].is_null(),
                "case {case:?}: expected {member} to be unset after an update that omits \
                 it, got {} in {updated}",
                updated[member]
            );
        }

        // What the update wrote is what the pool's readers report.
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            described, updated,
            "case {case:?}: DescribeIdentityPool reported {described}, expected the \
             updated pool {updated}"
        );

        // Including the account-wide listing, which reports the pool under its new name.
        let listed = emulator
            .ok("ListIdentityPools", &json!({"MaxResults": 60}))
            .await;
        let names: Vec<(&str, &str)> = listed["IdentityPools"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a page carries an IdentityPools list"))
            .iter()
            .map(|pool| {
                (
                    pool["IdentityPoolId"].as_str().unwrap_or_default(),
                    pool["IdentityPoolName"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        let expected_name = configuration["IdentityPoolName"].as_str().unwrap_or_default();
        assert!(
            names.contains(&(identity_pool_id.as_str(), expected_name)),
            "case {case:?}: expected the listing to report the pool as \
             {expected_name:?}, got {names:?}"
        );

        // And the pool the update did not name is exactly as it was.
        let untouched_now = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &untouched_id}),
            )
            .await;
        assert_eq!(
            untouched_now, untouched_before,
            "case {case:?}: the pool the update did not name expected {untouched_before}, \
             got {untouched_now}"
        );
    }
}

/// RULE: an update naming a pool this service does not hold is the modelled
/// `ResourceNotFoundException` and writes nothing.
#[tokio::test]
async fn an_update_of_a_pool_this_service_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let kept_id = emulator.pool("kept", true).await;
    let kept_before = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &kept_id}))
        .await;
    let deleted_id = emulator.pool("deleted", true).await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": &deleted_id}),
        )
        .await;

    let absent: Vec<(&str, String)> = vec![
        (
            "an IdentityPoolId no pool was ever created with",
            ABSENT_POOL_ID.to_string(),
        ),
        ("a pool that was deleted", deleted_id.clone()),
        (
            "a well-formed id naming another region",
            "eu-west-1:22222222-2222-2222-2222-222222222222".to_string(),
        ),
    ];
    for (case, identity_pool_id) in absent {
        let (status, body) = emulator
            .call(
                OPERATION,
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "IdentityPoolName": "renamed-by-a-doomed-update",
                    "AllowUnauthenticatedIdentities": true,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {identity_pool_id:?}, got {status} with \
             {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code} with {body}"
        );

        // The refused update created nothing: the pool it named is still not there.
        let (status, body) = emulator
            .call(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused update must not create {identity_pool_id:?}, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: a refused update must not create {identity_pool_id:?}, got {body}"
        );
    }

    // And the pool that does exist is exactly as it was.
    let kept_now = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &kept_id}))
        .await;
    assert_eq!(
        kept_now, kept_before,
        "the stored pool expected {kept_before} after the refused updates, got {kept_now}"
    );
}

/// What an update leaves on a pool's tag set, as `(case, the tags the pool was created
/// with, the `IdentityPoolTags` the update carries, the tag set the pool has afterwards)`.
/// A `None` means the member is not sent at all; the expectation is always a map, because the
/// response carries the member for every pool — the empty map for one with no tags.
///
/// The rows differ in the count of tags, in whether the update carries the member, in
/// whether the set it carries overlaps the stored one, and in key and value length:
/// `IdentityPoolTagsType` maps `TagKeysType` (len 1..128) to `TagValueType` (len 0..256),
/// from `smithy_query(query="shape",shape="IdentityPoolTagsType")`.
fn tag_cases() -> Vec<(&'static str, Option<Value>, Option<Value>, Value)> {
    vec![
        (
            "tags assigned at create, an update that omits the member",
            Some(json!({"Env": "test", "Owner": "platform"})),
            None,
            json!({"Env": "test", "Owner": "platform"}),
        ),
        (
            "no tags at all, an update that omits the member",
            None,
            None,
            json!({}),
        ),
        (
            "an update carrying a disjoint set replaces the stored one",
            Some(json!({"Env": "test", "Owner": "platform"})),
            Some(json!({"CostCenter": "4711"})),
            json!({"CostCenter": "4711"}),
        ),
        (
            "an update carrying an overlapping set replaces the stored one",
            Some(json!({"Env": "test", "Owner": "platform"})),
            Some(json!({"Env": "prod"})),
            json!({"Env": "prod"}),
        ),
        (
            "an update carrying an empty map leaves the pool with no tags",
            Some(json!({"Env": "test"})),
            Some(json!({})),
            json!({}),
        ),
        (
            "an update carrying tags for a pool that had none assigns them",
            None,
            Some(json!({"Env": "prod"})),
            json!({"Env": "prod"}),
        ),
        (
            "a key at the modelled maximum length of 128 survives an update",
            Some(tag_map([tag(&"k".repeat(128), "at-the-key-bound")])),
            None,
            tag_map([tag(&"k".repeat(128), "at-the-key-bound")]),
        ),
        (
            "values at the modelled bounds, 256 and 0, survive an update",
            Some(tag_map([
                tag("Description", &"v".repeat(256)),
                tag("Unset", ""),
            ])),
            None,
            tag_map([
                tag("Description", &"v".repeat(256)),
                tag("Unset", ""),
            ]),
        ),
        (
            "the 50 tags an identity pool can have survive an update",
            Some(tag_map(
                (0..50).map(|position| tag(&format!("key-{position}"), &format!("value-{position}"))),
            )),
            None,
            tag_map(
                (0..50).map(|position| tag(&format!("key-{position}"), &format!("value-{position}"))),
            ),
        ),
    ]
}

/// RULE: a pool's tags are not configuration this call replaces, so an update that carries
/// no `IdentityPoolTags` leaves the tag set the pool holds, while one that carries the
/// member states the pool's whole tag set — and both readers of that set, the pool's
/// describe and the tag listing, report what the update left.
#[tokio::test]
async fn an_update_replaces_the_tag_set_only_when_it_carries_one() {
    let emulator = Emulator::new();
    for (case, created_with, update_carries, expected) in tag_cases() {
        let identity_pool_id =
            tagged_pool(&emulator, "tagged-before-update", created_with.as_ref()).await;
        let mut request = json!({
            "IdentityPoolId": &identity_pool_id,
            "IdentityPoolName": "renamed-by-the-update",
            "AllowUnauthenticatedIdentities": true,
        });
        if let Some(tags) = &update_carries {
            request["IdentityPoolTags"] = tags.clone();
        }

        let updated = emulator.ok(OPERATION, &request).await;
        let answered = updated
            .get("IdentityPoolTags")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            answered, expected,
            "case {case:?}: the update answered IdentityPoolTags {answered}, expected \
             {expected}"
        );

        // The rest of the configuration IS replaced by the same call, so the tag set
        // surviving is not the whole record surviving.
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            described["IdentityPoolName"],
            json!("renamed-by-the-update"),
            "case {case:?}: expected the update to rename the pool, got {described}"
        );
        let described_tags = described
            .get("IdentityPoolTags")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            described_tags, expected,
            "case {case:?}: DescribeIdentityPool reported IdentityPoolTags \
             {described_tags}, expected {expected}"
        );

        // The tag listing reports the same set, as the empty map for a pool carrying none.
        let listed = listed_tags(&emulator, &identity_pool_id).await;
        let expected_listed = if expected.is_null() {
            tag_map([])
        } else {
            expected.clone()
        };
        assert_eq!(
            listed, expected_listed,
            "case {case:?}: ListTagsForResource reported {listed}, expected \
             {expected_listed}"
        );
    }
}

/// RULE: the same holds for tags a pool was given through the tagging operations rather
/// than at create time, however many updates later — and an update that carries the member
/// still states the whole set.
#[tokio::test]
async fn tags_assigned_through_the_tagging_operations_outlive_later_updates() {
    let emulator = Emulator::new();
    let assigned_sets: Vec<(&str, Value)> = vec![
        ("one tag", tag_map([tag("Env", "test")])),
        (
            "several tags",
            tag_map([
                tag("Env", "test"),
                tag("Owner", "platform"),
                tag("CostCenter", "4711"),
            ]),
        ),
        (
            "a value at the modelled maximum length of 256",
            tag_map([tag("Description", &"v".repeat(256))]),
        ),
    ];
    for (case, assigned) in assigned_sets {
        let identity_pool_id = tagged_pool(&emulator, "tagged-by-the-api", None).await;
        emulator
            .ok(
                "TagResource",
                &json!({
                    "ResourceArn": identity_pool_arn(&identity_pool_id),
                    "Tags": assigned.clone(),
                }),
            )
            .await;

        // Two updates in a row, neither carrying the member: the set is still there.
        for round in 1..=2 {
            let updated = emulator
                .ok(
                    OPERATION,
                    &json!({
                        "IdentityPoolId": &identity_pool_id,
                        "IdentityPoolName": format!("update-round-{round}"),
                        "AllowUnauthenticatedIdentities": false,
                    }),
                )
                .await;
            let answered = updated
                .get("IdentityPoolTags")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                answered, assigned,
                "case {case:?}, update {round}: the update answered IdentityPoolTags \
                 {answered}, expected {assigned}"
            );
            let listed = listed_tags(&emulator, &identity_pool_id).await;
            assert_eq!(
                listed, assigned,
                "case {case:?}, update {round}: ListTagsForResource reported {listed}, \
                 expected {assigned}"
            );
        }

        // And an update that does carry the member states the whole set, dropping every
        // key it leaves out.
        let replacement = tag_map([tag("Env", "prod")]);
        let updated = emulator
            .ok(
                OPERATION,
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "IdentityPoolName": "update-carrying-tags",
                    "AllowUnauthenticatedIdentities": false,
                    "IdentityPoolTags": replacement.clone(),
                }),
            )
            .await;
        assert_eq!(
            updated["IdentityPoolTags"], replacement,
            "case {case:?}: an update carrying tags answered {}, expected {replacement}",
            updated["IdentityPoolTags"]
        );
        let listed = listed_tags(&emulator, &identity_pool_id).await;
        assert_eq!(
            listed, replacement,
            "case {case:?}: ListTagsForResource reported {listed} after an update carrying \
             tags, expected {replacement}"
        );
    }
}
