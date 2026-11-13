// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UntagResource`: removing tags from an identity pool.
//!
//! The RULES every case asserts, over a table of inputs rather than one recorded call:
//!
//! * `UntagResource` removes from the tag set of the pool its `ResourceArn` names exactly
//!   the keys its `TagKeys` names, and leaves every other key the pool holds — and its
//!   value — exactly as it was; what is left is what the pool's readers
//!   (`ListTagsForResource`, `DescribeIdentityPool`) report.
//! * A key the pool does not hold removes nothing and still succeeds, so the same call
//!   made twice leaves the same set.
//! * A `TagKeys` entry outside the bounds the model states for
//!   `IdentityPoolTagsListType`'s member is HTTP 400 `ValidationException`, and a key in
//!   the reserved `aws:` namespace is the modelled 400 `InvalidParameterException`; a
//!   refused call removes nothing.
//! * The pool a `ResourceArn` names is the `IdentityPoolId` inside it, so any well-formed
//!   identity-pool ARN for one pool untags THAT pool; an ARN that is not an identity-pool
//!   ARN of this service is the modelled 400 `InvalidParameterException` and one naming a
//!   pool this service does not hold is the modelled `ResourceNotFoundException`.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::{
    identity_pool_arn, RESERVED_TAG_KEY_PREFIX,
};

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The region the harness signs for, and so the region every pool it creates is minted in
/// and named by.
const REGION: &str = "us-east-1";

/// An identity pool's ARN, in the form AWS documents for a Cognito Identity pool:
/// `arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`.
fn pool_arn(partition: &str, region: &str, account: &str, identity_pool_id: &str) -> String {
    format!("arn:{partition}:cognito-identity:{region}:{account}:identitypool/{identity_pool_id}")
}

/// The ARN a client holds for a pool in this emulator: the canonical partition, the pool's
/// own region and the emulator's account id (`engine::emulator_account_id`).
fn canonical_arn(identity_pool_id: &str) -> String {
    pool_arn("aws", REGION, engine::emulator_account_id(), identity_pool_id)
}

/// A JSON tag map from `(key, value)` pairs, so a case can carry keys and values that are
/// computed rather than written out.
fn tag_map<I: IntoIterator<Item = (String, String)>>(pairs: I) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

/// `(key, value)` as owned strings, for [`tag_map`].
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The tag map `tags` without the keys in `removed`: the set an `UntagResource` call leaves
/// behind, computed here rather than read back from the emulator.
fn without<'a, I: IntoIterator<Item = &'a str>>(tags: &Value, removed: I) -> Value {
    let mut result = tags
        .as_object()
        .unwrap_or_else(|| panic!("a tag map, got {tags:?}"))
        .clone();
    for key in removed {
        result.remove(key);
    }
    Value::Object(result)
}

/// Create a pool carrying `tags` (the member is sent only when the case has one) and return
/// its minted `IdentityPoolId`.
async fn tagged_pool(emulator: &Emulator, name: &str, tags: Option<&Value>) -> String {
    let mut request = json!({
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": false,
    });
    if let Some(tags) = tags {
        request["IdentityPoolTags"] = tags.clone();
    }
    let created = emulator.ok("CreateIdentityPool", &request).await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// List the tags of the pool `identity_pool_id` through its canonical ARN.
async fn listed_tags(emulator: &Emulator, identity_pool_id: &str) -> Value {
    let listed = emulator
        .ok(
            "ListTagsForResource",
            &json!({"ResourceArn": canonical_arn(identity_pool_id)}),
        )
        .await;
    listed["Tags"].clone()
}

/// The `IdentityPoolTags` the pool's other reader reports, as an empty map when it reports
/// no member at all.
async fn described_tags(emulator: &Emulator, identity_pool_id: &str) -> Value {
    let described = emulator
        .ok(
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": identity_pool_id}),
        )
        .await;
    described
        .get("IdentityPoolTags")
        .cloned()
        .unwrap_or_else(|| tag_map([]))
}

/// Call `UntagResource` for `arn` with `tag_keys`, returning the HTTP status and the parsed
/// body.
async fn untag_call(emulator: &Emulator, arn: &str, tag_keys: &[&str]) -> (u16, Value) {
    emulator
        .call(
            "UntagResource",
            &json!({"ResourceArn": arn, "TagKeys": tag_keys}),
        )
        .await
}

/// The tag set every removal case below starts from: keys differing in length, in the
/// characters they carry and in whether a later case shares a prefix with them.
fn baseline() -> Value {
    tag_map([
        tag("Environment", "Production"),
        tag("Owner", "platform"),
        tag("CostCenter", "4711"),
        tag("Unset", ""),
        tag("Description", "pool identité étape 1"),
        tag("k", "at-the-key-bound"),
        tag(&"k".repeat(128), "at-the-other-key-bound"),
    ])
}

/// RULE: `UntagResource` removes exactly the keys its `TagKeys` names from the pool its
/// `ResourceArn` names, and leaves every other key — and its value — exactly as it was.
///
/// The rows differ in HOW MANY keys the call names (none, one, several, all), in WHICH keys
/// those are (a long one, a short one, one with an empty value, one with a non-ASCII
/// value), in the ORDER they are named, and in whether they are keys the pool holds at all.
#[tokio::test]
async fn untag_resource_removes_only_the_keys_the_request_names() {
    let emulator = Emulator::new();
    let long_key = "k".repeat(128);

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("no keys at all", vec![]),
        ("one key", vec!["Environment"]),
        ("several keys", vec!["Environment", "Owner"]),
        (
            "the same keys named in the other order",
            vec!["Owner", "Environment"],
        ),
        ("a key whose value is empty", vec!["Unset"]),
        ("a key whose value is not ASCII", vec!["Description"]),
        (
            "a key at the modelled minimum length of 1",
            vec!["k"],
        ),
        (
            "a key at the modelled maximum length of 128",
            vec![long_key.as_str()],
        ),
        (
            "a key the pool does not hold",
            vec!["NeverAssigned"],
        ),
        (
            "one key the pool holds and one it does not",
            vec!["CostCenter", "NeverAssigned"],
        ),
        (
            "the same key named twice",
            vec!["CostCenter", "CostCenter"],
        ),
        (
            "the same key named many times over",
            vec!["CostCenter"; 100],
        ),
        (
            "every key the pool holds",
            vec![
                "Environment",
                "Owner",
                "CostCenter",
                "Unset",
                "Description",
                "k",
                long_key.as_str(),
            ],
        ),
    ];

    for (case, tag_keys) in cases {
        let assigned = baseline();
        let pool_id = tagged_pool(&emulator, "untag-pool", Some(&assigned)).await;
        let expected = without(&assigned, tag_keys.iter().copied());

        let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &tag_keys).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );

        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected ListTagsForResource to report Tags {expected}, got \
             {listed:?}"
        );
        let described = described_tags(&emulator, &pool_id).await;
        assert_eq!(
            described, expected,
            "case {case:?}: expected DescribeIdentityPool to report IdentityPoolTags \
             {expected}, got {described:?}"
        );

        // Removing the same keys again removes nothing more: the call is idempotent.
        let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &tag_keys).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated UntagResource to be HTTP 200, got {status} \
             with {body}"
        );
        let again = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            again, expected,
            "case {case:?}: expected a repeated UntagResource to leave Tags {expected}, got \
             {again:?}"
        );
    }
}

/// RULE: a removal acts on ONE resource, so the tag sets of every other pool are untouched,
/// and a key removed from one pool is still there on another that carries it.
#[tokio::test]
async fn untag_resource_touches_only_the_pool_its_arn_names() {
    let emulator = Emulator::new();
    let assigned = tag_map([tag("Environment", "Production"), tag("Owner", "platform")]);
    let untagged = tagged_pool(&emulator, "untagged-pool", Some(&assigned)).await;
    let neighbour = tagged_pool(&emulator, "neighbour-pool", Some(&assigned)).await;

    let (status, body) = untag_call(&emulator, &canonical_arn(&untagged), &["Environment"]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with {body}");

    let listed = listed_tags(&emulator, &untagged).await;
    let expected = without(&assigned, ["Environment"]);
    assert_eq!(
        listed, expected,
        "the untagged pool expected Tags {expected}, got {listed:?}"
    );
    let neighbour_tags = listed_tags(&emulator, &neighbour).await;
    assert_eq!(
        neighbour_tags, assigned,
        "the neighbouring pool expected its own Tags {assigned}, got {neighbour_tags:?}"
    );
}

/// RULE: a pool whose tag set is empty after a removal still reads back as a pool with an
/// empty tag set, and `TagResource` can assign to it again — removal takes the keys away,
/// not the resource's ability to carry tags.
#[tokio::test]
async fn a_pool_emptied_by_a_removal_can_be_tagged_again() {
    let emulator = Emulator::new();
    let assigned = tag_map([tag("Environment", "Production")]);
    let pool_id = tagged_pool(&emulator, "emptied-pool", Some(&assigned)).await;

    let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &["Environment"]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with {body}");
    let listed = listed_tags(&emulator, &pool_id).await;
    assert_eq!(
        listed,
        tag_map([]),
        "an emptied pool expected an empty Tags map, got {listed:?}"
    );

    let reassigned = tag_map([tag("Environment", "Staging"), tag("Owner", "identity")]);
    emulator
        .ok(
            "TagResource",
            &json!({"ResourceArn": canonical_arn(&pool_id), "Tags": reassigned.clone()}),
        )
        .await;
    let listed = listed_tags(&emulator, &pool_id).await;
    assert_eq!(
        listed, reassigned,
        "after re-tagging expected Tags {reassigned}, got {listed:?}"
    );

    // And the keys just assigned can be removed again.
    let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &["Owner"]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with {body}");
    let listed = listed_tags(&emulator, &pool_id).await;
    let expected = without(&reassigned, ["Owner"]);
    assert_eq!(
        listed, expected,
        "after the second removal expected Tags {expected}, got {listed:?}"
    );
}

/// RULE: a pool that carries no tag set at all accepts a removal — there is simply nothing
/// to remove — and keeps reporting exactly the stored set it had: the `IdentityPoolTags`
/// member is on the response either way, as the empty map, and a removal that removed
/// nothing leaves it exactly as it was.
#[tokio::test]
async fn untagging_a_pool_that_carries_no_tags_removes_nothing() {
    let emulator = Emulator::new();

    // (case, the tags the pool is created with)
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("no IdentityPoolTags member at all", None),
        ("an empty tag map", Some(tag_map([]))),
    ];

    for (case, created) in cases {
        let pool_id = tagged_pool(&emulator, "no-tags-pool", created.as_ref()).await;
        // What the pool's describe reader carries BEFORE the removal, member-present and
        // all, so the assertion after it compares the raw member rather than a map an
        // absent member was folded into.
        let before = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": pool_id.clone()}),
            )
            .await;
        let member_before = before.get("IdentityPoolTags").cloned();
        assert_eq!(
            member_before,
            Some(tag_map([])),
            "case {case:?}: expected DescribeIdentityPool to carry IdentityPoolTags as the \
             empty map before the removal, got {member_before:?}"
        );

        let (status, body) = untag_call(
            &emulator,
            &canonical_arn(&pool_id),
            &["Environment", "Owner"],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed,
            tag_map([]),
            "case {case:?}: expected an empty Tags map, got {listed:?}"
        );

        // The RAW member afterwards: the empty map the pool carries, unchanged.
        let after = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": pool_id.clone()}),
            )
            .await;
        let member_after = after.get("IdentityPoolTags").cloned();
        assert_eq!(
            member_after, member_before,
            "case {case:?}: expected the removal to leave IdentityPoolTags {member_before:?}, \
             got {member_after:?}"
        );
    }
}

/// RULE: the pool a `ResourceArn` names is the `IdentityPoolId` inside it, so an
/// `UntagResource` through any well-formed identity-pool ARN for one pool untags THAT pool
/// — whatever partition, region or account the ARN names it in.
#[tokio::test]
async fn untag_resource_untags_the_pool_any_well_formed_arn_for_it_names() {
    let emulator = Emulator::new();
    let keys = ["minted", "canonical", "china", "govcloud", "other-account"];
    let assigned = tag_map(keys.iter().map(|key| tag(key, "assigned")));
    let pool_id = tagged_pool(&emulator, "arn-forms-untagging", Some(&assigned)).await;

    let arns: Vec<(&str, String)> = vec![
        (
            "the ARN this service itself mints for the pool",
            identity_pool_arn(&pool_id),
        ),
        (
            "the canonical partition, region and emulator account",
            canonical_arn(&pool_id),
        ),
        (
            "the China partition",
            pool_arn(
                "aws-cn",
                "cn-north-1",
                engine::emulator_account_id(),
                &pool_id,
            ),
        ),
        (
            "the GovCloud partition",
            pool_arn(
                "aws-us-gov",
                "us-gov-west-1",
                engine::emulator_account_id(),
                &pool_id,
            ),
        ),
        (
            "another twelve-digit account id",
            pool_arn("aws", REGION, "123456789012", &pool_id),
        ),
    ];

    let mut removed: Vec<&str> = Vec::new();
    for ((case, arn), key) in arns.into_iter().zip(keys) {
        let (status, body) = untag_call(&emulator, &arn, &[key]).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({arn}): expected HTTP 200, got {status} with {body}"
        );
        removed.push(key);
        let expected = without(&assigned, removed.iter().copied());
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?} ({arn}): expected Tags {expected}, got {listed:?}"
        );
    }
}

/// RULE: a `ResourceArn` that is not an identity-pool ARN of this service names no resource
/// at all, so it is a bad input parameter (400 `InvalidParameterException`) and not a
/// missing resource — while a value outside the bounds the MODEL states for `ARNString`, or
/// a required member that is absent, never reaches the operation: the constraint-validation
/// layer ahead of it refuses the request with 400 `ValidationException`. Nothing is removed
/// either way.
#[tokio::test]
async fn untag_resource_refuses_an_arn_that_names_no_identity_pool() {
    let emulator = Emulator::new();
    let assigned = baseline();
    let pool_id = tagged_pool(&emulator, "malformed-arns-untagging", Some(&assigned)).await;

    // The third column is the code the row is refused with: this handler's own
    // `InvalidParameterException` for an ARN it can read that names nothing of this service,
    // and `ValidationException` where the value is outside a modelled bound and so is
    // refused before the handler runs.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "not an ARN at all",
            "this-is-not-an-arn-at-all".to_string(),
            "InvalidParameterException",
        ),
        (
            "another service's ARN namespace",
            format!("arn:aws:cognito-idp:{REGION}:000000000000:userpool/{REGION}_abcdefghi"),
            "InvalidParameterException",
        ),
        (
            "another resource type in this namespace",
            format!("arn:aws:cognito-identity:{REGION}:000000000000:identity/{pool_id}"),
            "InvalidParameterException",
        ),
        (
            "no pool id after the resource type",
            format!("arn:aws:cognito-identity:{REGION}:000000000000:identitypool/"),
            "InvalidParameterException",
        ),
        (
            "an account id that is not twelve digits",
            pool_arn("aws", REGION, "12345", &pool_id),
            "InvalidParameterException",
        ),
        (
            "an empty region",
            pool_arn("aws", "", engine::emulator_account_id(), &pool_id),
            "InvalidParameterException",
        ),
        (
            "an empty partition",
            pool_arn("", REGION, engine::emulator_account_id(), &pool_id),
            "InvalidParameterException",
        ),
        (
            "too few ARN fields",
            format!("arn:aws:cognito-identity:identitypool/{pool_id}"),
            "InvalidParameterException",
        ),
        (
            "shorter than ARNString's modelled minimum of 20 characters",
            "arn:aws:cognito".to_string(),
            "ValidationException",
        ),
    ];

    for (case, arn, expected_code) in cases {
        let (status, body) = untag_call(&emulator, &arn, &["Environment"]).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({arn}): expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?} ({arn}): expected {expected_code}, got {body}"
        );
    }

    // A required member that is absent is the same class of refusal as a value outside a
    // modelled bound: the request never reaches the operation.
    let absent: Vec<(&str, Value)> = vec![
        ("no member at all", json!({})),
        (
            "no ResourceArn",
            json!({"TagKeys": ["Environment"]}),
        ),
        (
            "no TagKeys",
            json!({"ResourceArn": canonical_arn(&pool_id)}),
        ),
    ];
    for (case, request) in absent {
        let (status, body) = emulator.call("UntagResource", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // Every refusal above removed nothing.
    let listed = listed_tags(&emulator, &pool_id).await;
    assert_eq!(
        listed, assigned,
        "a refused UntagResource removes nothing, expected {assigned}, got {listed:?}"
    );
}

/// RULE: a `ResourceArn` naming a pool this service does not hold — one never created, or
/// one already deleted — is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn untag_resource_refuses_an_arn_naming_a_pool_that_is_not_stored() {
    let emulator = Emulator::new();
    let assigned = tag_map([tag("Environment", "Production")]);
    let kept = tagged_pool(&emulator, "kept-while-untagging", Some(&assigned)).await;
    let deleted = tagged_pool(&emulator, "deleted-while-untagging", Some(&assigned)).await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": deleted.clone()}),
        )
        .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "an IdentityPoolId no pool was ever created with",
            canonical_arn(ABSENT_POOL_ID),
        ),
        (
            "a pool named in another region, never created there",
            pool_arn(
                "aws",
                "eu-west-1",
                engine::emulator_account_id(),
                "eu-west-1:22222222-2222-2222-2222-222222222222",
            ),
        ),
        ("a pool that was deleted", canonical_arn(&deleted)),
    ];

    for (case, arn) in cases {
        let (status, body) = untag_call(&emulator, &arn, &["Environment"]).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({arn}): expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?} ({arn}): expected ResourceNotFoundException, got {body}"
        );
    }

    // The pool that was kept is untouched by every miss above.
    let listed = listed_tags(&emulator, &kept).await;
    assert_eq!(
        listed, assigned,
        "the kept pool expected its own Tags {assigned} after the misses, got {listed:?}"
    );
}

/// RULE: a `TagKeys` entry outside the bounds the model states for
/// `IdentityPoolTagsListType`'s member shape is refused with HTTP 400
/// `ValidationException`, nothing is removed, and an entry sitting ON a published bound is
/// accepted.
#[tokio::test]
async fn untag_resource_refuses_tag_keys_outside_the_modelled_bounds() {
    let emulator = Emulator::new();
    // `IdentityPoolTagsListType` is a list of `TagKeysType`, len 1..128
    // (`smithy_query(query="shape",shape="IdentityPoolTagsListType")`), so each row is one
    // step outside one published bound.
    let over = "k".repeat(129);
    let over_in_characters = "é".repeat(129);
    let assigned = baseline();

    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "a key one character past the maximum of 128",
            vec![over.as_str()],
        ),
        ("a key below the minimum of 1", vec![""]),
        (
            "a key past the maximum counted in characters, not bytes",
            vec![over_in_characters.as_str()],
        ),
        (
            "one entry inside the bounds and one outside",
            vec!["Environment", over.as_str()],
        ),
        (
            "many entries outside the bounds in one list",
            (0..40)
                .map(|_| over.as_str())
                .chain((0..40).map(|_| ""))
                .collect(),
        ),
    ];

    for (case, tag_keys) in cases {
        let pool_id = tagged_pool(&emulator, "bounds-untagging", Some(&assigned)).await;
        let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &tag_keys).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, assigned,
            "case {case:?}: a refused UntagResource removes nothing, expected {assigned}, got \
             {listed:?}"
        );
    }

    // At each published bound the same call is accepted: an entry is refused for being
    // OUTSIDE the bound, never for sitting on it.
    let at_the_bound = "k".repeat(128);
    let at_the_bounds: Vec<(&str, Vec<&str>)> = vec![
        ("a key at the minimum of 1", vec!["k"]),
        (
            "a key at the maximum of 128",
            vec![at_the_bound.as_str()],
        ),
    ];
    for (case, tag_keys) in at_the_bounds {
        let pool_id = tagged_pool(&emulator, "bounds-untagging", Some(&assigned)).await;
        let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &tag_keys).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let expected = without(&assigned, tag_keys.iter().copied());
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected Tags {expected}, got {listed:?}"
        );
    }
}

/// RULE: the `aws:` tag-key prefix is reserved for AWS's own tags, which a caller can
/// neither assign nor delete, so naming such a key is refused with the modelled 400
/// `InvalidParameterException` and nothing is removed — while a key that merely carries
/// those letters elsewhere is an ordinary key and is removed.
#[tokio::test]
async fn untag_resource_refuses_a_tag_key_in_the_reserved_aws_namespace() {
    let emulator = Emulator::new();

    // (case, the tag key, accepted?) — the reservation is on the PREFIX, so where the
    // letters sit is what decides, and the prefix is reserved in any casing.
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "a key AWS itself generates",
            "aws:cloudformation:stack-name".to_string(),
            false,
        ),
        (
            "the prefix with nothing after it",
            RESERVED_TAG_KEY_PREFIX.to_string(),
            false,
        ),
        ("the prefix in upper case", "AWS:region".to_string(), false),
        ("the prefix in mixed case", "Aws:createdBy".to_string(), false),
        (
            "a key that only starts with the same letters",
            "awsome-service".to_string(),
            true,
        ),
        (
            "the letters followed by something other than the colon",
            "aws-region".to_string(),
            true,
        ),
        (
            "the prefix further inside the key",
            "team:aws:owner".to_string(),
            true,
        ),
        (
            "a key that ends with the prefix",
            "my-aws:".to_string(),
            true,
        ),
    ];

    for (case, key, accepted) in cases {
        // A pool carrying the key under test beside one that is never named, so an
        // accepted removal is visible and a refused one is too.
        let mut pairs = Map::new();
        pairs.insert(key.clone(), Value::String("value".to_string()));
        pairs.insert("Owner".to_string(), Value::String("platform".to_string()));
        let assigned = Value::Object(pairs);
        let pool_id = tagged_pool(&emulator, "reserved-key-untagging", Some(&assigned)).await;

        let (status, body) = untag_call(&emulator, &canonical_arn(&pool_id), &[key.as_str()]).await;
        let listed = listed_tags(&emulator, &pool_id).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} (key {key:?}): expected HTTP 200, got {status} with {body}"
            );
            let expected = without(&assigned, [key.as_str()]);
            assert_eq!(
                listed, expected,
                "case {case:?} (key {key:?}): expected Tags {expected}, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (key {key:?}): expected HTTP 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterException"),
                "case {case:?} (key {key:?}): expected InvalidParameterException, got {body}"
            );
            assert_eq!(
                listed, assigned,
                "case {case:?} (key {key:?}): a refused UntagResource removes nothing, expected \
                 {assigned}, got {listed:?}"
            );
        }
    }
}
