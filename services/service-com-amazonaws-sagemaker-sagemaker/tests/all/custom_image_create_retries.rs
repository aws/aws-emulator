// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the two answers a name already in use gets:
//! `CreateImage` repeated with the SAME request, and `CreateImage` repeating a name for a
//! DIFFERENT image.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! `Emulator` harness `inference_deployment_lifecycle` owns.
//!
//! The RULE, asserted over a TABLE of inputs rather than one recorded call: a create that
//! REPEATS a request this account has already satisfied is answered with the existing
//! image's ARN — the SDK's own retry path — and creates nothing new, while a name re-used
//! for an image whose configuration differs in ANY member is refused with the modelled
//! `ResourceInUse`. The rows differ along which member of the create the repeat restates
//! or changes, the tag set's size and order, and the shape of the name.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// A `RoleArn` satisfying the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$` (smithy_query
/// query="operation" operation="CreateImage").
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/SageMakerExecution";

/// A second role, so a row can change the role the image is built under.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/OtherExecution";

/// `ImageName` is `len 1..63` with pattern `^[a-zA-Z0-9]([-.]?[a-zA-Z0-9]){0,62}$`
/// (smithy_query query="operation" operation="CreateImage"): one character, a hyphenated
/// name, a dotted name, and the modelled 63-character maximum.
const IMAGE_NAMES: [&str; 4] = [
    "i",
    "retried-image",
    "dotted.image.name",
    "a234567890123456789012345678901234567890123456789012345678901b",
];

/// The ARN an image named `name` carries in the region every call below signs for.
fn image_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/{name}")
}

/// A `CreateImage` body for `name`, with `extra`'s members merged in.
fn create_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({ "ImageName": name, "RoleArn": ROLE_ARN });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// RULE: a `CreateImage` that repeats the request the stored image was registered by is
/// answered 200 with that image's ARN, mints no second image, and leaves the image exactly
/// as it was — whatever members the create states.
#[tokio::test]
async fn an_identical_create_is_answered_with_the_existing_images_arn() {
    // Rows differ along which optional members the create states, and along the tag set:
    // none, one, several, and the same several in another order (a tag set is a set, so
    // the order the request lists them in is not another request).
    let rows: [(&str, Value, Value); 6] = [
        ("the required members alone", json!({}), json!({})),
        (
            "a description",
            json!({ "Description": "the ranking image" }),
            json!({ "Description": "the ranking image" }),
        ),
        (
            "a description and a display name",
            json!({ "Description": "the ranking image", "DisplayName": "Ranking" }),
            json!({ "Description": "the ranking image", "DisplayName": "Ranking" }),
        ),
        (
            "one tag",
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
        ),
        (
            "several tags",
            json!({
                "DisplayName": "Tagged",
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "Owner", "Value": "ml-platform" },
                ],
            }),
            json!({
                "DisplayName": "Tagged",
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "Owner", "Value": "ml-platform" },
                ],
            }),
        ),
        (
            "the same tags in another order",
            json!({
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "Owner", "Value": "ml-platform" },
                ],
            }),
            json!({
                "Tags": [
                    { "Key": "Owner", "Value": "ml-platform" },
                    { "Key": "env", "Value": "prod" },
                ],
            }),
        ),
    ];

    for (index, (case, created, repeated)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = IMAGE_NAMES[index % IMAGE_NAMES.len()];
        let expected_arn = image_arn(name);

        let first = emulator.ok("CreateImage", &create_body(name, &created)).await;
        assert_eq!(
            str_field(&first, "ImageArn"),
            expected_arn,
            "case {case:?}: expected the created ARN {expected_arn}, got {first}"
        );
        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": name }))
            .await;

        // The repeat: the same 200, the same ARN, and no second image.
        let (status, again) = emulator.call("CreateImage", &create_body(name, &repeated)).await;
        assert_eq!(
            (status, str_field(&again, "ImageArn")),
            (200, expected_arn.clone()),
            "case {case:?}: a repeated create must answer the stored image's ARN, got \
             {status} with {again}"
        );
        let reread = emulator
            .ok("DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            reread.get("CreationTime"),
            described.get("CreationTime"),
            "case {case:?}: a repeated create must not re-create the image, got {reread}"
        );
        let listed = emulator
            .ok("ListImages", &json!({ "NameContains": name }))
            .await;
        assert_eq!(
            listed
                .get("Images")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
            "case {case:?}: the account must hold exactly one image of this name, got \
             {listed}"
        );
    }
}

/// RULE: a name already in use for an image whose configuration DIFFERS in any member is
/// refused with the modelled `ResourceInUse` (HTTP 400), naming the `ImageName` the request
/// sent, and the stored image is left exactly as it was.
#[tokio::test]
async fn a_name_re_used_for_a_different_image_is_refused_as_in_use() {
    // Rows differ along WHICH member differs from the stored image: each optional member
    // in turn, added, dropped or changed, and the role the image is built under.
    let rows: [(&str, Value, Value); 7] = [
        (
            "a different description",
            json!({ "Description": "the ranking image" }),
            json!({ "Description": "another image" }),
        ),
        (
            "a description where the image has none",
            json!({}),
            json!({ "Description": "the ranking image" }),
        ),
        (
            "no description where the image has one",
            json!({ "Description": "the ranking image" }),
            json!({}),
        ),
        (
            "a different display name",
            json!({ "DisplayName": "Ranking" }),
            json!({ "DisplayName": "Recall" }),
        ),
        (
            "a different role",
            json!({}),
            json!({ "RoleArn": OTHER_ROLE_ARN }),
        ),
        (
            "a different tag value",
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
            json!({ "Tags": [{ "Key": "env", "Value": "staging" }] }),
        ),
        (
            "one tag more",
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
            json!({
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "Owner", "Value": "ml-platform" },
                ],
            }),
        ),
    ];

    for (index, (case, created, differing)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = IMAGE_NAMES[index % IMAGE_NAMES.len()];
        emulator.ok("CreateImage", &create_body(name, &created)).await;
        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": name }))
            .await;

        let (status, refused) = emulator
            .call("CreateImage", &create_body(name, &differing))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceInUse"),
            "case {case:?}: a name taken by a different image must be refused, got \
             {status} with {refused}"
        );
        let expected = format!(
            "The Image with ImageName {name} already exists. Please use a different \
             ImageName."
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );
        let reread = emulator
            .ok("DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            reread.get("RoleArn"),
            described.get("RoleArn"),
            "case {case:?}: a refused create must leave the image as it was, got {reread}"
        );
        assert_eq!(
            reread.get("Description"),
            described.get("Description"),
            "case {case:?}: a refused create must leave the image as it was, got {reread}"
        );
    }
}
