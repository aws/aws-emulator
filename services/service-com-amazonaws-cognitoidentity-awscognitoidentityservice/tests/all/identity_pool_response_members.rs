// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ENVELOPE every identity-pool answer carries: the pool's ARN, the
//! instant it was created, the instant it last changed and its tag set.
//!
//! RULE: every operation that answers with an identity pool reports that pool as this service
//! holds it — the pool's ARN
//! (`arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`), the
//! instant it was created, the instant it last changed and its tag set, the tag set reported
//! as the EMPTY map for a pool that carries no tags — so a create echo, a describe, an update
//! echo and a listing entry all report the same values for the same pool. The instants are the
//! pool's own history: creation never moves, last-modified moves when (and only when) the
//! stored pool changes, and never because the pool was read.
//!
//! `IdentityPoolTags` is modelled on the `IdentityPool` shape
//! (`smithy_query(query="shape", shape="IdentityPool")`); `IdentityPoolArn`, `CreationDate`
//! and `LastModifiedDate` are not modelled, and are members the recorded behaviour of this
//! service carries on every identity-pool response.
//!
//! The rows differ along what the rule generalises over: the shape of the pool (which optional
//! configuration members it carries), the COUNT of tags it holds (none at all, the empty map,
//! one, several, the 50 an identity pool can have), the region it is minted in — the ARN's
//! region is the pool's own, not a constant — and, for the instants, the KIND of change made
//! to the pool afterwards.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::Emulator;
use crate::resource_tags::{tag, tag_map};

/// The ARN every identity-pool answer carries for `identity_pool_id`, built here from its
/// parts rather than through the service's own helper: the canonical partition, this
/// service's ARN namespace, the region the pool was minted in, the emulator's account id and
/// the pool as the resource.
fn expected_arn(region: &str, identity_pool_id: &str) -> String {
    format!(
        "arn:aws:cognito-identity:{region}:{}:identitypool/{identity_pool_id}",
        engine::emulator_account_id()
    )
}

/// Seconds since the epoch, as an `awsJson` body writes an instant and as this suite reads
/// one, so a reported instant can be compared against the clock the call ran under.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is at or after the epoch")
        .as_secs_f64()
}

/// The instant `member` carries on `body`, asserting the member is there and is an instant.
fn instant(case: &str, operation: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {operation} carries {member}: {body}"))
}

/// The ARN `body` reports for the pool, asserting the member is there and is a string.
fn reported_arn(case: &str, operation: &str, body: &Value) -> String {
    body.get("IdentityPoolArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: {operation} carries IdentityPoolArn: {body}"))
        .to_string()
}

/// The tag set `body` reports for the pool, asserting the member is THERE: the service carries
/// it on every pool it answers with, as the empty map for a pool that holds no tags.
fn reported_tags(case: &str, operation: &str, body: &Value) -> Value {
    let tags = body
        .get("IdentityPoolTags")
        .unwrap_or_else(|| panic!("case {case:?}: {operation} carries IdentityPoolTags: {body}"));
    assert!(
        tags.is_object(),
        "case {case:?}: {operation} reports IdentityPoolTags as a map, got {tags}"
    );
    tags.clone()
}

/// Create a pool in `region`, named `name`, carrying `extras` (the optional members of this
/// row) and `tags` (the member is sent only when the row has one). Answers the whole create
/// response.
async fn create_pool(
    emulator: &Emulator,
    region: &str,
    name: &str,
    extras: &Value,
    tags: Option<&Value>,
) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolName".to_string(), json!(name));
    request.insert("AllowUnauthenticatedIdentities".to_string(), json!(true));
    for (member, value) in extras.as_object().expect("the extras are an object") {
        request.insert(member.clone(), value.clone());
    }
    if let Some(tags) = tags {
        request.insert("IdentityPoolTags".to_string(), tags.clone());
    }
    let (status, created) = emulator
        .call_in(region, "CreateIdentityPool", &Value::Object(request))
        .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateIdentityPool, got {status} with {created}"
    );
    created
}

/// `DescribeIdentityPool` for `identity_pool_id`, signed for `region`.
async fn describe_pool(emulator: &Emulator, region: &str, identity_pool_id: &str) -> Value {
    let (status, described) = emulator
        .call_in(
            region,
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": identity_pool_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DescribeIdentityPool, got {status} with {described}"
    );
    described
}

/// The one entry a listing reports for `identity_pool_id`, read in `region`. `MaxResults` is
/// the modelled maximum, so the fixture's pools fit in one page.
async fn listed_pool(emulator: &Emulator, region: &str, identity_pool_id: &str) -> Value {
    let (status, page) = emulator
        .call_in(region, "ListIdentityPools", &json!({"MaxResults": 60}))
        .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListIdentityPools, got {status} with {page}"
    );
    page["IdentityPools"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries an IdentityPools list: {page}"))
        .iter()
        .find(|pool| pool["IdentityPoolId"] == json!(identity_pool_id))
        .unwrap_or_else(|| panic!("the listing reports the pool {identity_pool_id}: {page}"))
        .clone()
}

/// Every shape of pool the rows below report, as `(case, the optional configuration members,
/// the `IdentityPoolTags` the pool is created with)`. `None` sends the tag member not at all.
///
/// The configuration members come from
/// `smithy_query(query="operation", operation="CreateIdentityPool")`; the 50-tag row is the
/// ceiling the model's own `TagResource` doc publishes ("An identity pool can have as many as
/// 50 tags").
fn pool_shapes() -> Vec<(&'static str, Value, Option<Value>)> {
    vec![
        ("only the required members, no tag member", json!({}), None),
        ("an empty tag map", json!({}), Some(tag_map([]))),
        (
            "one tag",
            json!({}),
            Some(tag_map([tag("Environment", "Test")])),
        ),
        (
            "several tags and the classic flow",
            json!({"AllowClassicFlow": true}),
            Some(tag_map([
                tag("Environment", "Production"),
                tag("Owner", "platform"),
                tag("CostCenter", "4711"),
            ])),
        ),
        (
            "every optional configuration member, and tags",
            json!({
                "AllowClassicFlow": false,
                "DeveloperProviderName": "login.example.com",
                "SupportedLoginProviders": {"graph.facebook.com": "0123456789"},
                "CognitoIdentityProviders": [{
                    "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_abcdEFGHI",
                    "ClientId": "1h57kf5cpq17m0eml12eu9ebf",
                    "ServerSideTokenCheck": true,
                }],
                "OpenIdConnectProviderARNs": [
                    "arn:aws:iam::123456789012:oidc-provider/accounts.example.com",
                ],
                "SamlProviderARNs": ["arn:aws:iam::123456789012:saml-provider/Login"],
            }),
            Some(tag_map([tag("Description", "pool identité étape 1")])),
        ),
        (
            "the 50 tags an identity pool can have",
            json!({}),
            Some(tag_map((0..50).map(|position| {
                tag(&format!("key-{position}"), &format!("value-{position}"))
            }))),
        ),
    ]
}

/// The regions the rows are minted in: the ARN and the id name the region the call was signed
/// for, so a row in another region proves neither is a constant.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// RULE: every operation that answers with a pool carries that pool's ARN, its creation and
/// last-modified instants and its tag set — the empty map for a pool with no tags — and a
/// listing entry reports the same values the pool's own readers do.
#[tokio::test]
async fn every_pool_answer_carries_the_arn_the_instants_and_the_tag_set() {
    let emulator = Emulator::new();
    for region in REGIONS {
        for (case, extras, tags) in pool_shapes() {
            let case = &format!("{case} in {region}");
            let expected_tags = tags.clone().unwrap_or_else(|| tag_map([]));

            let before = epoch_seconds(SystemTime::now());
            let created = create_pool(&emulator, region, "pool-envelope", &extras, tags.as_ref())
                .await;
            let after = epoch_seconds(SystemTime::now());

            let identity_pool_id = created["IdentityPoolId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a create answers an id: {created}"))
                .to_string();
            assert!(
                identity_pool_id.starts_with(&format!("{region}:")),
                "case {case:?}: expected an id minted in {region}, got {identity_pool_id:?}"
            );

            // The create echo: the ARN of the pool just minted, the two instants of a pool
            // nothing has changed yet, and the tag set it was created with.
            let arn = reported_arn(case, "CreateIdentityPool", &created);
            assert_eq!(
                arn,
                expected_arn(region, &identity_pool_id),
                "case {case:?}: expected the pool's own ARN, got {arn:?}"
            );
            let creation = instant(case, "CreateIdentityPool", &created, "CreationDate");
            let modified = instant(case, "CreateIdentityPool", &created, "LastModifiedDate");
            assert!(
                (before..=after).contains(&creation),
                "case {case:?}: expected a creation instant between {before} and {after}, got \
                 {creation}"
            );
            assert_eq!(
                creation, modified,
                "case {case:?}: a pool nothing has changed was last modified when it was \
                 created, got {creation} and {modified}"
            );
            let answered_tags = reported_tags(case, "CreateIdentityPool", &created);
            assert_eq!(
                answered_tags, expected_tags,
                "case {case:?}: CreateIdentityPool expected IdentityPoolTags {expected_tags}, \
                 got {answered_tags}"
            );

            // The pool's reader reports the very same envelope, member for member.
            let described = describe_pool(&emulator, region, &identity_pool_id).await;
            assert_eq!(
                described, created,
                "case {case:?}: DescribeIdentityPool reported {described}, expected the created \
                 pool {created}"
            );

            // And a read moves nothing: asked again, it answers the same instants.
            let again = describe_pool(&emulator, region, &identity_pool_id).await;
            assert_eq!(
                again, described,
                "case {case:?}: a second DescribeIdentityPool reported {again}, expected \
                 {described}"
            );

            // The listing entry is the pool's id and name plus the same ARN and instants its
            // own readers report, and nothing else.
            let listed = listed_pool(&emulator, region, &identity_pool_id).await;
            let expected_entry = json!({
                "IdentityPoolId": identity_pool_id,
                "IdentityPoolName": created["IdentityPoolName"].clone(),
                "IdentityPoolArn": arn,
                "CreationDate": creation,
                "LastModifiedDate": modified,
            });
            assert_eq!(
                listed, expected_entry,
                "case {case:?}: the listing entry {listed} expected {expected_entry}"
            );
        }
    }
}

/// What a row does to the pool after it was created, and whether that is a CHANGE to the
/// stored pool (so the last-modified instant moves) or not (so it stays put).
#[derive(Clone, Copy)]
enum Change {
    /// An `UpdateIdentityPool` renaming the pool.
    Rename,
    /// An `UpdateIdentityPool` carrying a whole new tag set.
    UpdateTags,
    /// A `TagResource` assigning a key.
    Tag,
    /// A `TagResource` carrying no key at all.
    TagNothing,
    /// A `TagResource` re-assigning the value a key the pool holds already has.
    TagSame,
    /// An `UntagResource` removing a key the pool holds.
    Untag,
    /// An `UntagResource` naming a key the pool does not hold.
    UntagAbsent,
    /// A read: the pool's own reader.
    Describe,
    /// A read: the account's listing.
    List,
}

/// RULE: the instants are the pool's own history — the creation instant never moves, and the
/// last-modified instant moves when the stored pool changes and only then, never because the
/// pool was read.
///
/// A row that changes the pool brackets the changing call between two readings of the clock
/// and requires the new instant to land inside them, so the assertion fails against a handler
/// that leaves the instant where the create put it.
#[tokio::test]
async fn the_last_modified_instant_moves_with_the_pool_and_never_with_a_read() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    // The rows span the KINDS of call a pool can be made through: the two that rewrite its
    // configuration or tag set, the two tagging calls, each in a form that does change the
    // stored set and one that cannot, and the two reads. (case, what the row does, whether the
    // stored pool changes)
    let rows: [(&str, Change, bool); 9] = [
        ("an update renaming the pool", Change::Rename, true),
        ("an update carrying a new tag set", Change::UpdateTags, true),
        ("a TagResource assigning a key", Change::Tag, true),
        ("a TagResource carrying no key", Change::TagNothing, false),
        (
            "a TagResource re-assigning the value a key already has",
            Change::TagSame,
            false,
        ),
        ("an UntagResource removing a held key", Change::Untag, true),
        (
            "an UntagResource naming a key the pool does not hold",
            Change::UntagAbsent,
            false,
        ),
        ("a DescribeIdentityPool", Change::Describe, false),
        ("a ListIdentityPools", Change::List, false),
    ];

    for (case, change, changes_the_pool) in rows {
        let held = tag_map([tag("Environment", "Test")]);
        let created = create_pool(
            &emulator,
            region,
            "pool-instants",
            &json!({}),
            Some(&held),
        )
        .await;
        let identity_pool_id = created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a create answers an id: {created}"))
            .to_string();
        let creation = instant(case, "CreateIdentityPool", &created, "CreationDate");
        let modified = instant(case, "CreateIdentityPool", &created, "LastModifiedDate");
        let arn = expected_arn(region, &identity_pool_id);

        // The clock is read around the call the row makes, so a row that DOES change the pool
        // can assert the last-modified instant became THIS call's — an assertion a handler
        // that left the instant where the create put it cannot satisfy.
        let before = epoch_seconds(SystemTime::now());
        match change {
            Change::Rename => {
                let updated = emulator
                    .call_in(
                        region,
                        "UpdateIdentityPool",
                        &json!({
                            "IdentityPoolId": &identity_pool_id,
                            "IdentityPoolName": "renamed-pool",
                            "AllowUnauthenticatedIdentities": false,
                        }),
                    )
                    .await
                    .1;
                // The update's own echo already reports the moved instant and the untouched
                // creation one.
                assert_eq!(
                    instant(case, "UpdateIdentityPool", &updated, "CreationDate"),
                    creation,
                    "case {case:?}: the creation instant does not move, got {updated}"
                );
                let echoed =
                    instant(case, "UpdateIdentityPool", &updated, "LastModifiedDate");
                assert!(
                    echoed > modified,
                    "case {case:?}: expected the last-modified instant to have moved past \
                     {modified}, got {echoed}"
                );
            }
            Change::UpdateTags => {
                emulator
                    .call_in(
                        region,
                        "UpdateIdentityPool",
                        &json!({
                            "IdentityPoolId": &identity_pool_id,
                            "IdentityPoolName": "retagged-pool",
                            "AllowUnauthenticatedIdentities": true,
                            "IdentityPoolTags": tag_map([tag("Owner", "platform")]),
                        }),
                    )
                    .await;
            }
            Change::Tag => {
                emulator
                    .call_in(
                        region,
                        "TagResource",
                        &json!({
                            "ResourceArn": &arn,
                            "Tags": tag_map([tag("Owner", "platform")]),
                        }),
                    )
                    .await;
            }
            Change::TagNothing => {
                emulator
                    .call_in(
                        region,
                        "TagResource",
                        &json!({"ResourceArn": &arn, "Tags": tag_map([])}),
                    )
                    .await;
            }
            Change::TagSame => {
                emulator
                    .call_in(
                        region,
                        "TagResource",
                        &json!({"ResourceArn": &arn, "Tags": held.clone()}),
                    )
                    .await;
            }
            Change::Untag => {
                emulator
                    .call_in(
                        region,
                        "UntagResource",
                        &json!({"ResourceArn": &arn, "TagKeys": ["Environment"]}),
                    )
                    .await;
            }
            Change::UntagAbsent => {
                emulator
                    .call_in(
                        region,
                        "UntagResource",
                        &json!({"ResourceArn": &arn, "TagKeys": ["NeverAssigned"]}),
                    )
                    .await;
            }
            Change::Describe => {
                describe_pool(&emulator, region, &identity_pool_id).await;
            }
            Change::List => {
                listed_pool(&emulator, region, &identity_pool_id).await;
            }
        }

        let after = epoch_seconds(SystemTime::now());

        let described = describe_pool(&emulator, region, &identity_pool_id).await;
        assert_eq!(
            instant(case, "DescribeIdentityPool", &described, "CreationDate"),
            creation,
            "case {case:?}: expected the creation instant {creation} to stand, got {described}"
        );
        let now_modified = instant(case, "DescribeIdentityPool", &described, "LastModifiedDate");
        if changes_the_pool {
            assert!(
                (before..=after).contains(&now_modified),
                "case {case:?}: expected the last-modified instant to be the changing call's, \
                 between {before} and {after}, got {now_modified}"
            );
        } else {
            assert_eq!(
                now_modified, modified,
                "case {case:?}: expected the last-modified instant {modified} to stand, got \
                 {now_modified}"
            );
        }
        // The listing reports the same instants as the pool's own reader, whatever happened.
        let listed = listed_pool(&emulator, region, &identity_pool_id).await;
        assert_eq!(
            (
                listed["IdentityPoolArn"].clone(),
                listed["CreationDate"].clone(),
                listed["LastModifiedDate"].clone(),
            ),
            (
                described["IdentityPoolArn"].clone(),
                described["CreationDate"].clone(),
                described["LastModifiedDate"].clone(),
            ),
            "case {case:?}: the listing entry {listed} expected the pool's own envelope \
             {described}"
        );
    }
}
