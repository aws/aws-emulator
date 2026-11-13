// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tagging of an identity pool: `TagResource`, which assigns the
//! tags, and `ListTagsForResource`, which reports them.
//!
//! The RULES every case asserts, over a table of inputs rather than one recorded call:
//!
//! * `TagResource` writes the keys the request carries onto the tag set of the pool its
//!   `ResourceArn` names — overwriting a key already there and leaving every other key
//!   alone — and the set it leaves behind is what the pool's readers report. It refuses,
//!   WITHOUT changing the stored set, a key in the reserved `aws:` namespace and a call
//!   that would leave the pool with more tags than an identity pool can have (both the
//!   modelled 400 `InvalidParameterException`).
//! * `ListTagsForResource` reports exactly the tag set stored on the resource its
//!   `ResourceArn` names — an EMPTY map when the resource carries none — and leaves that
//!   set unchanged.
//! * Either operation refuses an ARN that is not an identity-pool ARN of this service with
//!   the modelled 400 `InvalidParameterException`, and answers the modelled
//!   `ResourceNotFoundException` for one naming a pool this service does not hold.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::{
    identity_pool_arn, MAX_TAGS_PER_IDENTITY_POOL, RESERVED_TAG_KEY_PREFIX,
};

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The region the harness signs for, and so the region every pool it creates is minted
/// in and named by.
const REGION: &str = "us-east-1";

/// An identity pool's ARN, in the form AWS documents for a Cognito Identity pool:
/// `arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`.
/// Built here from its parts rather than through the service's own helper, so a case can
/// vary each field independently of how the emulator renders an ARN.
fn pool_arn(partition: &str, region: &str, account: &str, identity_pool_id: &str) -> String {
    format!("arn:{partition}:cognito-identity:{region}:{account}:identitypool/{identity_pool_id}")
}

/// The ARN a client holds for a pool in this emulator: the canonical partition, the
/// pool's own region and the emulator's account id (`engine::emulator_account_id`).
fn canonical_arn(identity_pool_id: &str) -> String {
    pool_arn("aws", REGION, engine::emulator_account_id(), identity_pool_id)
}

/// A JSON tag map from `(key, value)` pairs, so a case can carry keys and values that are
/// computed rather than written out.
pub(crate) fn tag_map<I: IntoIterator<Item = (String, String)>>(pairs: I) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

/// `(key, value)` as owned strings, for [`tag_map`].
pub(crate) fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Every shape of tag set a pool can carry, as `(case, IdentityPoolTags)` — `None` sends
/// the member not at all. The rows differ in the COUNT of tags, in key and value length
/// (`IdentityPoolTagsType` maps `TagKeysType`, len 1..128, to `TagValueType`, len 0..256,
/// from `smithy_query(query="shape",shape="IdentityPoolTagsType")`, so each bound is a
/// row) and in the characters a value carries. The 50-tag row is the ceiling the model's
/// own `TagResource` doc publishes: "An identity pool can have as many as 50 tags"
/// (`smithy_query(query="doc",shape="TagResource")`).
fn tag_sets() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no IdentityPoolTags member at all", None),
        ("an empty tag map", Some(tag_map([]))),
        ("one tag", Some(tag_map([tag("Environment", "Test")]))),
        (
            "several tags",
            Some(tag_map([
                tag("Environment", "Production"),
                tag("Owner", "platform"),
                tag("CostCenter", "4711"),
            ])),
        ),
        (
            "a key at the modelled maximum length of 128",
            Some(tag_map([tag(&"k".repeat(128), "at-the-key-bound")])),
        ),
        (
            "a key at the modelled minimum length of 1",
            Some(tag_map([tag("k", "at-the-key-bound")])),
        ),
        (
            "a value at the modelled maximum length of 256",
            Some(tag_map([tag("at-the-value-bound", &"v".repeat(256))])),
        ),
        (
            "an empty value, the modelled minimum length of 0",
            Some(tag_map([tag("Unset", "")])),
        ),
        (
            // The characters a tag is spelled with: AWS's tagging rules publish them as
            // letters, numbers, spaces representable in UTF-8 and `_ . : / = + - @` — which
            // is the set the refusal of a tag outside it names (`TagResource`) — so a value
            // of letters of any script and spaces is one AWS stores, byte for byte.
            "a value carrying spaces and non-ASCII letters",
            Some(tag_map([tag("Description", "pool identité étape 1")])),
        ),
        (
            "the 50 tags an identity pool can have",
            Some(tag_map(
                (0..50).map(|position| tag(&format!("key-{position}"), &format!("value-{position}"))),
            )),
        ),
    ]
}

/// Create a pool carrying `tags` (the member is sent only when the case has one) and
/// return its minted `IdentityPoolId`.
pub(crate) async fn tagged_pool(
    emulator: &Emulator,
    name: &str,
    tags: Option<&Value>,
) -> String {
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
pub(crate) async fn listed_tags(emulator: &Emulator, identity_pool_id: &str) -> Value {
    let listed = emulator
        .ok(
            "ListTagsForResource",
            &json!({"ResourceArn": canonical_arn(identity_pool_id)}),
        )
        .await;
    listed["Tags"].clone()
}

/// RULE: `ListTagsForResource` reports exactly the tag set stored on the pool its
/// `ResourceArn` names, as an empty map when the pool carries none, and reading it leaves
/// that set exactly as it was — as the pool's other reader, `DescribeIdentityPool`,
/// confirms.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_stored_on_the_pool() {
    let emulator = Emulator::new();
    for (case, tags) in tag_sets() {
        let pool_id = tagged_pool(&emulator, "tagged-pool", tags.as_ref()).await;
        // A pool created without tags carries NONE, which is an empty set.
        let expected = tags.unwrap_or_else(|| tag_map([]));

        let listed = listed_tags(&emulator, &pool_id).await;
        assert!(
            listed.is_object(),
            "case {case:?}: expected Tags to be a map even when empty, got {listed:?}"
        );
        assert_eq!(
            listed, expected,
            "case {case:?}: expected Tags {expected}, got {listed:?}"
        );

        // A read changes nothing a later caller sees: the same call, and the pool's other
        // reader, report the same set.
        let again = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second ListTagsForResource expected Tags {expected}, got {again:?}"
        );
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": pool_id.clone()}),
            )
            .await;
        let described_tags = described
            .get("IdentityPoolTags")
            .cloned()
            .unwrap_or_else(|| tag_map([]));
        assert_eq!(
            described_tags, expected,
            "case {case:?}: DescribeIdentityPool expected IdentityPoolTags {expected}, got {described_tags:?}"
        );
    }
}

/// RULE: the pool a `ResourceArn` names is the `IdentityPoolId` inside it, so every
/// well-formed identity-pool ARN for one pool reads that pool's tags — whatever partition,
/// region or account the ARN names them in.
#[tokio::test]
async fn every_well_formed_arn_for_a_pool_reads_that_pools_tags() {
    let emulator = Emulator::new();
    let tags = tag_map([tag("Environment", "Test"), tag("Owner", "platform")]);
    let pool_id = tagged_pool(&emulator, "arn-forms", Some(&tags)).await;

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

    for (case, arn) in arns {
        let listed = emulator
            .ok("ListTagsForResource", &json!({"ResourceArn": arn.clone()}))
            .await;
        assert_eq!(
            listed["Tags"], tags,
            "case {case:?} ({arn}): expected Tags {tags}, got {listed}"
        );
    }
}

/// RULE: a `ResourceArn` naming a pool this service does not hold — one never created, or
/// one already deleted — is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn an_arn_naming_a_pool_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    // A stored pool, so the store is not empty and a miss is a miss on this pool alone.
    let kept = tagged_pool(
        &emulator,
        "kept-pool",
        Some(&tag_map([tag("Environment", "Test")])),
    )
    .await;
    let deleted = tagged_pool(
        &emulator,
        "deleted-pool",
        Some(&tag_map([tag("Environment", "Test")])),
    )
    .await;
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
        let (status, body) = emulator
            .call("ListTagsForResource", &json!({"ResourceArn": arn.clone()}))
            .await;
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
        listed,
        tag_map([tag("Environment", "Test")]),
        "the kept pool expected its own tags after the misses, got {listed:?}"
    );
}

/// RULE: a `ResourceArn` that is not an identity-pool ARN of this service names no
/// resource at all, so it is a bad input parameter (400 `InvalidParameterException`) and
/// not a missing resource — while an ARN outside the bounds the MODEL states for
/// `ARNString` never reaches the operation: the constraint-validation layer ahead of it
/// refuses the request with 400 `ValidationException`.
#[tokio::test]
async fn an_arn_that_names_no_identity_pool_is_an_invalid_parameter() {
    let emulator = Emulator::new();
    let pool_id = tagged_pool(&emulator, "malformed-arns", None).await;

    // The third column is the code the row is refused with: the handler's own
    // `InvalidParameterException` for an ARN it can read and that names nothing of this
    // service, and `ValidationException` where the value is outside a modelled bound and
    // so is refused before the handler runs.
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
        let (status, body) = emulator
            .call("ListTagsForResource", &json!({"ResourceArn": arn.clone()}))
            .await;
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
    let (status, body) = emulator.call("ListTagsForResource", &json!({})).await;
    assert_eq!(
        status, 400,
        "an absent ResourceArn expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "an absent ResourceArn expected ValidationException, got {body}"
    );
}

// ---------------------------------------------------------------------------------------
// `TagResource`: assigning the tags the readers above report.
// ---------------------------------------------------------------------------------------

/// The tag sets a `TagResource` call can assign: the rows of [`tag_sets`] that carry the
/// member at all, since `Tags` is modelled **required** on `TagResource`
/// (`smithy_query(query="operation",operation="TagResource")`).
fn assignable_tag_sets() -> Vec<(&'static str, Value)> {
    tag_sets()
        .into_iter()
        .filter_map(|(case, tags)| tags.map(|tags| (case, tags)))
        .collect()
}

/// `count` tags keyed `<prefix>-<position>` with values `<value_prefix>-<position>`, so a
/// case can build a tag set of any size and re-assign the same keys with other values.
fn numbered_tags_valued(prefix: &str, count: usize, value_prefix: &str) -> Value {
    tag_map((0..count).map(|position| {
        tag(
            &format!("{prefix}-{position}"),
            &format!("{value_prefix}-{position}"),
        )
    }))
}

/// `count` tags keyed `<prefix>-<position>`.
fn numbered_tags(prefix: &str, count: usize) -> Value {
    numbered_tags_valued(prefix, count, "value")
}

/// `base` with every entry of `update` written over it: the tag set a `TagResource` call
/// leaves behind, computed here rather than read back from the emulator.
fn merged(base: &Value, update: &Value) -> Value {
    let mut result = base
        .as_object()
        .unwrap_or_else(|| panic!("a tag map, got {base:?}"))
        .clone();
    for (key, value) in update
        .as_object()
        .unwrap_or_else(|| panic!("a tag map, got {update:?}"))
    {
        result.insert(key.clone(), value.clone());
    }
    Value::Object(result)
}

/// How many tags a tag map carries.
fn tag_count(tags: &Value) -> usize {
    tags.as_object()
        .unwrap_or_else(|| panic!("a tag map, got {tags:?}"))
        .len()
}

/// Call `TagResource` for `arn`, returning the HTTP status and the parsed body.
async fn tag_call(emulator: &Emulator, arn: &str, tags: &Value) -> (u16, Value) {
    emulator
        .call(
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": tags.clone()}),
        )
        .await
}

/// Assign `tags` to the pool through its canonical ARN, expecting the call to succeed.
async fn tag_pool(emulator: &Emulator, identity_pool_id: &str, tags: &Value) {
    emulator
        .ok(
            "TagResource",
            &json!({"ResourceArn": canonical_arn(identity_pool_id), "Tags": tags.clone()}),
        )
        .await;
}

/// RULE: `TagResource` PERSISTS the tag set it is handed on the pool its `ResourceArn`
/// names, so the pool's readers report exactly that set afterwards — and assigning the
/// same set again leaves it as it is.
#[tokio::test]
async fn tag_resource_persists_the_tag_set_the_request_carries() {
    let emulator = Emulator::new();
    for (case, tags) in assignable_tag_sets() {
        let pool_id = tagged_pool(&emulator, "tag-resource-pool", None).await;
        tag_pool(&emulator, &pool_id, &tags).await;

        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, tags,
            "case {case:?}: expected ListTagsForResource to report the assigned Tags {tags}, got {listed:?}"
        );
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": pool_id.clone()}),
            )
            .await;
        let described_tags = described
            .get("IdentityPoolTags")
            .cloned()
            .unwrap_or_else(|| tag_map([]));
        assert_eq!(
            described_tags, tags,
            "case {case:?}: expected DescribeIdentityPool to report IdentityPoolTags {tags}, got {described_tags:?}"
        );

        // Assigning the same set again is the same set: one more call changes nothing.
        tag_pool(&emulator, &pool_id, &tags).await;
        let again = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            again, tags,
            "case {case:?}: expected a repeated TagResource to leave Tags {tags}, got {again:?}"
        );
    }
}

/// RULE: a `TagResource` call writes the keys it carries — overwriting a key the pool
/// already holds — and leaves every other key the pool holds exactly as it was.
#[tokio::test]
async fn tag_resource_writes_its_own_keys_over_the_set_the_pool_already_carries() {
    let emulator = Emulator::new();
    // Two tags to start from, one of which several rows below overwrite.
    let baseline = tag_map([tag("Environment", "Initial"), tag("Owner", "platform")]);
    for (case, tags) in assignable_tag_sets() {
        let expected = merged(&baseline, &tags);
        // The pool's tag ceiling is a rule of its own, with its own case below: a row that
        // would take this pool past it is not a merge row.
        if tag_count(&expected) > MAX_TAGS_PER_IDENTITY_POOL {
            continue;
        }
        let pool_id = tagged_pool(&emulator, "merge-pool", Some(&baseline)).await;
        tag_pool(&emulator, &pool_id, &tags).await;

        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the merge of the baseline {baseline} and {tags} to be \
             {expected}, got {listed:?}"
        );
    }
}

/// RULE: the value stored for a key is the one the LAST call that assigned it carried, and
/// a call carrying no keys at all changes nothing.
#[tokio::test]
async fn the_last_call_that_assigns_a_key_is_the_value_the_pool_carries() {
    let emulator = Emulator::new();
    let pool_id = tagged_pool(
        &emulator,
        "sequence-pool",
        Some(&tag_map([tag("Environment", "Initial")])),
    )
    .await;

    // (case, the tags the call assigns, the set the pool carries afterwards)
    let sequence: Vec<(&str, Value, Value)> = vec![
        (
            "a key the pool does not carry is added beside the one it does",
            tag_map([tag("Owner", "platform")]),
            tag_map([tag("Environment", "Initial"), tag("Owner", "platform")]),
        ),
        (
            "a key the pool carries takes the new call's value",
            tag_map([tag("Environment", "Production")]),
            tag_map([tag("Environment", "Production"), tag("Owner", "platform")]),
        ),
        (
            "a call naming both keys writes both",
            tag_map([tag("Environment", "Staging"), tag("Owner", "identity")]),
            tag_map([tag("Environment", "Staging"), tag("Owner", "identity")]),
        ),
        (
            "an empty value is a value, so it overwrites the one before it",
            tag_map([tag("Environment", "")]),
            tag_map([tag("Environment", ""), tag("Owner", "identity")]),
        ),
        (
            "re-assigning the value a key already has leaves it there",
            tag_map([tag("Owner", "identity")]),
            tag_map([tag("Environment", ""), tag("Owner", "identity")]),
        ),
        (
            "a call carrying no tags assigns nothing",
            tag_map([]),
            tag_map([tag("Environment", ""), tag("Owner", "identity")]),
        ),
    ];

    for (case, assigned, expected) in sequence {
        tag_pool(&emulator, &pool_id, &assigned).await;
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: after assigning {assigned} expected Tags {expected}, got {listed:?}"
        );
    }
}

/// RULE: a pool can carry at most the published number of tags, so a call that would leave
/// it with more is refused with the modelled 400 `InvalidParameterException` and the stored
/// set is left exactly as it was; a call that stays at or under the ceiling — including one
/// that only re-assigns keys a full pool already carries — is accepted.
#[tokio::test]
async fn tag_resource_refuses_a_call_that_would_take_the_pool_past_its_tag_ceiling() {
    // The ceiling comes from the model's own `TagResource` doc: "An identity pool can have
    // as many as 50 tags" (`smithy_query(query="doc",shape="TagResource")`).
    assert_eq!(
        MAX_TAGS_PER_IDENTITY_POOL, 50,
        "the published identity-pool tag ceiling is 50, got {MAX_TAGS_PER_IDENTITY_POOL}"
    );
    let ceiling = MAX_TAGS_PER_IDENTITY_POOL;
    let emulator = Emulator::new();

    // (case, the tags the pool is created with, the tags the call assigns, accepted?)
    let cases: Vec<(&str, Value, Value, bool)> = vec![
        (
            "the whole ceiling in one call",
            tag_map([]),
            numbered_tags("key", ceiling),
            true,
        ),
        (
            "one tag past the ceiling in one call",
            tag_map([]),
            numbered_tags("key", ceiling + 1),
            false,
        ),
        (
            "the last tag that fits",
            numbered_tags("key", ceiling - 1),
            tag_map([tag("Extra", "fits")]),
            true,
        ),
        (
            "two tags where only one fits",
            numbered_tags("key", ceiling - 1),
            tag_map([tag("Extra", "fits"), tag("Another", "does-not")]),
            false,
        ),
        (
            "re-assigning keys a full pool already carries",
            numbered_tags("key", ceiling),
            numbered_tags_valued("key", 5, "rewritten"),
            true,
        ),
        (
            "one new key on a full pool",
            numbered_tags("key", ceiling),
            tag_map([tag("Extra", "over")]),
            false,
        ),
        (
            "two halves that land exactly on the ceiling",
            numbered_tags("key", ceiling / 2),
            numbered_tags("other", ceiling - ceiling / 2),
            true,
        ),
        (
            "two halves that land one past the ceiling",
            numbered_tags("key", ceiling / 2),
            numbered_tags("other", ceiling - ceiling / 2 + 1),
            false,
        ),
    ];

    for (case, created, assigned, accepted) in cases {
        let pool_id = tagged_pool(&emulator, "ceiling-pool", Some(&created)).await;
        let (status, body) = tag_call(&emulator, &canonical_arn(&pool_id), &assigned).await;
        let listed = listed_tags(&emulator, &pool_id).await;
        if accepted {
            let expected = merged(&created, &assigned);
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for a set of {} tags, got {status} with {body}",
                tag_count(&expected)
            );
            assert_eq!(
                listed, expected,
                "case {case:?}: expected Tags {expected}, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {body}"
            );
            assert_eq!(
                listed, created,
                "case {case:?}: a refused TagResource leaves the stored set as it was, expected \
                 {created}, got {listed:?}"
            );
        }
    }
}

/// RULE: the `aws:` tag-key prefix is reserved for AWS's own tags, so a key carrying it is
/// refused with the modelled 400 `InvalidParameterException` and nothing is written, while
/// a key that merely contains those letters elsewhere is an ordinary key.
#[tokio::test]
async fn tag_resource_refuses_a_tag_key_in_the_reserved_aws_namespace() {
    let emulator = Emulator::new();
    let baseline = tag_map([tag("Environment", "Initial")]);

    // (case, the tag key, accepted?) — the reservation is on the PREFIX, so where the
    // letters sit is what decides, and the prefix is reserved in any casing.
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "a key AWS itself generates",
            "aws:cloudformation:stack-name".to_string(),
            false,
        ),
        ("the prefix with nothing after it", RESERVED_TAG_KEY_PREFIX.to_string(), false),
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
        ("a key that ends with the prefix", "my-aws:".to_string(), true),
    ];

    for (case, key, accepted) in cases {
        let pool_id = tagged_pool(&emulator, "reserved-key-pool", Some(&baseline)).await;
        let assigned = tag_map([(key.clone(), "value".to_string())]);
        let (status, body) = tag_call(&emulator, &canonical_arn(&pool_id), &assigned).await;
        let listed = listed_tags(&emulator, &pool_id).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} (key {key:?}): expected HTTP 200, got {status} with {body}"
            );
            let expected = merged(&baseline, &assigned);
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
                listed, baseline,
                "case {case:?} (key {key:?}): a refused TagResource writes nothing, expected \
                 {baseline}, got {listed:?}"
            );
        }
    }
}

/// RULE: a `TagResource` whose `Tags` fall outside the bounds the model states for the tag
/// map's key and value shapes is refused with HTTP 400 and nothing is written. A KEY
/// outside its bound is this handler's own refusal — the protocol's boundary walk checks
/// the map's values but not its keys — and carries `ValidationException`, the code AWS
/// answers for a modelled constraint (recorded in
/// `20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md`:
/// "expected ValidationException for a 129-char tag key").
#[tokio::test]
async fn tag_resource_refuses_tags_outside_the_modelled_bounds() {
    let emulator = Emulator::new();
    let baseline = tag_map([tag("Environment", "Initial")]);

    // `IdentityPoolTagsType` maps `TagKeysType` (len 1..128) to `TagValueType` (len 0..256)
    // (`smithy_query(query="shape",shape="IdentityPoolTagsType")`), so each row is one step
    // outside one published bound. The third column is the error code the row is refused
    // with where this operation owns the refusal; `None` where the protocol's own boundary
    // walk refuses the request before the handler runs, since which code THAT path answers
    // for a modelled constraint is the service-wide classification question of
    // `20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md`.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a key one character past the maximum of 128",
            tag_map([tag(&"k".repeat(129), "over-the-key-bound")]),
            Some("ValidationException"),
        ),
        (
            "a key below the minimum of 1",
            tag_map([tag("", "under-the-key-bound")]),
            Some("ValidationException"),
        ),
        (
            "a key past the maximum counted in characters, not bytes",
            tag_map([tag(&"é".repeat(129), "over-the-key-bound-in-characters")]),
            Some("ValidationException"),
        ),
        (
            "a value one character past the maximum of 256",
            tag_map([tag("over-the-value-bound", &"v".repeat(257))]),
            None,
        ),
        (
            "a multi-byte value one character past the maximum, counted in characters",
            tag_map([tag("over-the-value-bound-in-characters", &"é".repeat(257))]),
            None,
        ),
        (
            "one entry inside the bounds and one outside",
            tag_map([
                tag("Owner", "platform"),
                tag("over-the-value-bound", &"v".repeat(257)),
            ]),
            None,
        ),
    ];

    for (case, assigned, expected_code) in cases {
        let pool_id = tagged_pool(&emulator, "bounds-pool", Some(&baseline)).await;
        let (status, body) = tag_call(&emulator, &canonical_arn(&pool_id), &assigned).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        if let Some(expected_code) = expected_code {
            assert!(
                error_code(&body).contains(expected_code),
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, baseline,
            "case {case:?}: a refused TagResource writes nothing, expected {baseline}, got \
             {listed:?}"
        );
    }

    // At each published bound the same call is accepted: a tag is refused for being
    // OUTSIDE the bound, never for sitting on it.
    let at_the_bounds: Vec<(&str, Value)> = vec![
        (
            "a key at the maximum of 128",
            tag_map([tag(&"k".repeat(128), "at-the-key-bound")]),
        ),
        (
            "a key at the minimum of 1",
            tag_map([tag("k", "at-the-key-bound")]),
        ),
        (
            "a value at the maximum of 256",
            tag_map([tag("at-the-value-bound", &"v".repeat(256))]),
        ),
        (
            "a multi-byte value of 256 characters",
            tag_map([tag("at-the-value-bound-in-characters", &"é".repeat(256))]),
        ),
        (
            "a value at the minimum of 0",
            tag_map([tag("at-the-empty-value-bound", "")]),
        ),
    ];
    for (case, assigned) in at_the_bounds {
        let pool_id = tagged_pool(&emulator, "bounds-pool", Some(&baseline)).await;
        let (status, body) = tag_call(&emulator, &canonical_arn(&pool_id), &assigned).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let expected = merged(&baseline, &assigned);
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected Tags {expected}, got {listed:?}"
        );
    }
}

/// RULE: the pool a `ResourceArn` names is the `IdentityPoolId` inside it, so a
/// `TagResource` through any well-formed identity-pool ARN for one pool tags THAT pool —
/// whatever partition, region or account the ARN names it in.
#[tokio::test]
async fn tag_resource_tags_the_pool_any_well_formed_arn_for_it_names() {
    let emulator = Emulator::new();
    let pool_id = tagged_pool(&emulator, "arn-forms-tagging", None).await;

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

    let mut expected = tag_map([]);
    for (case, arn) in arns {
        // The key this row writes is its case text with the characters a tag key may not
        // hold taken out (AWS's tagging rules admit letters, numbers, spaces and
        // `_ . : / = + - @`), so what the call is judged on is the ARN rather than the key.
        let key = case.replace(',', "");
        let assigned = tag_map([tag(&key, arn.as_str())]);
        let (status, body) = tag_call(&emulator, &arn, &assigned).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({arn}): expected HTTP 200, got {status} with {body}"
        );
        expected = merged(&expected, &assigned);
        let listed = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?} ({arn}): expected Tags {expected}, got {listed:?}"
        );
    }
}

/// RULE: a `TagResource` whose `ResourceArn` names a pool this service does not hold — one
/// never created, or one already deleted — is the modelled `ResourceNotFoundException`,
/// and one that is not an identity-pool ARN of this service at all is the modelled 400
/// `InvalidParameterException`.
#[tokio::test]
async fn tag_resource_refuses_an_arn_that_names_no_stored_pool() {
    let emulator = Emulator::new();
    let kept = tagged_pool(
        &emulator,
        "kept-while-tagging",
        Some(&tag_map([tag("Environment", "Test")])),
    )
    .await;
    let deleted = tagged_pool(&emulator, "deleted-while-tagging", None).await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": deleted.clone()}),
        )
        .await;

    let assigned = tag_map([tag("Environment", "Production")]);

    let missing: Vec<(&str, String)> = vec![
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
    for (case, arn) in missing {
        let (status, body) = tag_call(&emulator, &arn, &assigned).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({arn}): expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?} ({arn}): expected ResourceNotFoundException, got {body}"
        );
    }

    let malformed: Vec<(&str, String)> = vec![
        ("not an ARN at all", "this-is-not-an-arn-at-all".to_string()),
        (
            "another service's ARN namespace",
            format!("arn:aws:cognito-idp:{REGION}:000000000000:userpool/{REGION}_abcdefghi"),
        ),
        (
            "another resource type in this namespace",
            format!("arn:aws:cognito-identity:{REGION}:000000000000:identity/{kept}"),
        ),
        (
            "no pool id after the resource type",
            format!("arn:aws:cognito-identity:{REGION}:000000000000:identitypool/"),
        ),
        (
            "an account id that is not twelve digits",
            pool_arn("aws", REGION, "12345", &kept),
        ),
        (
            "an empty region",
            pool_arn("aws", "", engine::emulator_account_id(), &kept),
        ),
        (
            "too few ARN fields",
            format!("arn:aws:cognito-identity:identitypool/{kept}"),
        ),
    ];
    for (case, arn) in malformed {
        let (status, body) = tag_call(&emulator, &arn, &assigned).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({arn}): expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterException"),
            "case {case:?} ({arn}): expected InvalidParameterException, got {body}"
        );
    }

    // Every refusal above wrote nothing: the pool that was kept still carries its own tags.
    let listed = listed_tags(&emulator, &kept).await;
    assert_eq!(
        listed,
        tag_map([tag("Environment", "Test")]),
        "the kept pool expected its own tags after the refusals, got {listed:?}"
    );
}
