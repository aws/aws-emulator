// SPDX-License-Identifier: CC0-1.0

//! The image a launch boots has to be an AMI the account really has, driven over the wire
//! this service speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`).
//!
//! RULE under test: `ManagedInstance.ImageId` names an Amazon Machine Image of the
//! caller's own account in the Region the call was signed for — `ManagedInstanceRequest`
//! carries the parameters of an EC2 launch
//! (`smithy_query(query="shape",shape="ManagedInstanceRequest")`) — so a
//! `CreateWorkspaceInstance` whose `ImageId` resolves to no image the account can see is
//! refused at request time with the modelled `ValidationException` (HTTP 400,
//! `smithy_query(query="operation",operation="CreateWorkspaceInstance")`) and creates
//! nothing: no instance id is minted, no record is written and the `ClientToken` the call
//! carried is not spent.
//!
//! The rows span what the rule generalises over: the id FORMS the modelled `ImageId`
//! (`^ami-[0-9a-zA-Z]{1,63}$`, `smithy_query(query="shape",shape="ImageId")`) admits —
//! EC2's long and short id forms, a length between them, an upper-case one and the
//! shortest the model allows — all of them ids no image in this account has.
//!
//! The positive side is what makes the check real rather than a blanket refusal: an image
//! the account holds launches, and an AMI REGISTERED through `ec2:RegisterImage` during
//! the test launches too — so the answer follows Amazon EC2's own image state rather than
//! anything this crate keeps.

use serde_json::{json, Value};

use crate::ec2_managed_instance_propagation::ec2_client;
use crate::workspace_instance_lifecycle::{
    call, create, error_code, harness, list_all_ids, ok, TEST_IMAGE_ID, TEST_INSTANCE_TYPE,
};

/// The `Message` AWS answers a create whose image cannot be resolved with: fixed text in
/// full, echoing none of what the caller sent — the offending id included.
const INVALID_IMAGE_MESSAGE: &str = "Invalid AMI ID provided";

/// The HTTP status the model gives `ValidationException`
/// (`smithy_query(query="shape",shape="ValidationException")` → `httpError 400`).
const VALIDATION_STATUS: u16 = 400;

/// A create body booting `image_id`, with `token` as its `ClientToken`.
fn create_body(image_id: &str, token: &str) -> Value {
    json!({
        "ClientToken": token,
        "ManagedInstance": { "ImageId": image_id, "InstanceType": TEST_INSTANCE_TYPE },
    })
}

/// The id forms the modelled `ImageId` admits, none of which names an image this account
/// has: EC2's long form (17 hexadecimal digits), its short form (8), a length between the
/// two, an upper-case id, and the shortest id the modelled pattern allows.
fn unresolvable_image_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("the long id form", "ami-0000000000deadbee"),
        ("another long id form", "ami-0fedfedfedfedfed0"),
        ("the short id form", "ami-12345678"),
        ("a length between the two id forms", "ami-0123456789"),
        ("an upper-case id", "ami-ABCDEF0123456789A"),
        ("the shortest id the model admits", "ami-z"),
    ]
}

#[tokio::test]
async fn a_create_naming_an_image_the_account_has_not_got_is_refused() {
    for (case, image_id) in unresolvable_image_ids() {
        // One store per row, so what the store holds afterwards is this row's doing.
        let (router, _engine) = harness();
        // One instance the account really has, so the id set a refusal must leave
        // untouched is not the empty one.
        let existing = create(&router, create_body(TEST_IMAGE_ID, "tok-existing")).await;

        let token = "tok-refused";
        let (status, payload) =
            call(&router, "CreateWorkspaceInstance", create_body(image_id, token)).await;
        assert_eq!(
            status, VALIDATION_STATUS,
            "case {case:?} ({image_id:?}): expected HTTP {VALIDATION_STATUS}, got {status} \
             with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?} ({image_id:?}): expected ValidationException, got {payload}"
        );
        let message = payload["Message"]
            .as_str()
            .or_else(|| payload["message"].as_str());
        assert_eq!(
            message,
            Some(INVALID_IMAGE_MESSAGE),
            "case {case:?} ({image_id:?}): expected {INVALID_IMAGE_MESSAGE:?}, got {payload}"
        );
        assert!(
            !payload.to_string().contains(image_id),
            "case {case:?}: the refusal must echo none of the submitted id, got {payload}"
        );

        // Nothing was created for it: the id set is the one the accepted create left.
        let ids = list_all_ids(&router, json!({})).await;
        assert_eq!(
            ids,
            vec![existing.clone()],
            "case {case:?} ({image_id:?}): expected no instance for the refused create, got \
             {ids:?}"
        );

        // And the refused call spent no `ClientToken`: the same token now carries a create
        // this service can launch, which it could not if the refusal had claimed it.
        let created = create(&router, create_body(TEST_IMAGE_ID, token)).await;
        assert_ne!(
            created, existing,
            "case {case:?}: expected the token to be unspent and mint a new instance"
        );
        let mut ids = list_all_ids(&router, json!({})).await;
        ids.sort();
        let mut expected = vec![existing, created];
        expected.sort();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected exactly the two accepted creates, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_create_naming_an_image_the_account_holds_launches() {
    let (router, _engine) = harness();

    // The images the account's own Amazon EC2 holds, each launched and read back.
    for image_id in crate::workspace_instance_lifecycle::ACCOUNT_IMAGES {
        let id = create(&router, create_body(image_id, &format!("tok-{image_id}"))).await;
        let got = ok(
            &router,
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &id }),
        )
        .await;
        assert_eq!(
            got["WorkspaceInstanceId"].as_str(),
            Some(id.as_str()),
            "case image {image_id:?}: expected the created instance to read back, got {got}"
        );
    }
}

#[tokio::test]
async fn an_ami_registered_through_ec2_becomes_launchable() {
    let (router, _engine) = harness();
    let ec2 = ec2_client(&router);

    // An AMI the account did not have until this call: `ec2:RegisterImage` mints the id,
    // which is why the test cannot know it in advance.
    let registered = ec2
        .register_image()
        .name("workspaces-instances-launchable-image")
        // "Either imageLocation or rootDeviceName must be specified" — the register names
        // the root device of an EBS-backed AMI.
        .root_device_name("/dev/xvda")
        .send()
        .await
        .expect("ec2:RegisterImage answers")
        .image_id()
        .expect("a registered image carries its id")
        .to_string();

    // A create naming it is accepted, and the instance reads back — so the resolution
    // follows EC2's image state rather than a fixed list inside this service.
    let id = create(&router, create_body(&registered, "tok-registered")).await;
    let got = ok(
        &router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": &id }),
    )
    .await;
    assert_eq!(
        got["WorkspaceInstanceId"].as_str(),
        Some(id.as_str()),
        "case the registered image {registered:?}: expected the instance to read back, got {got}"
    );

    // A well-formed id of the same form that no register minted is still refused: what the
    // create above was accepted for is the registered IMAGE, not the shape of its id.
    let unregistered = "ami-0999999999999999a";
    assert_ne!(
        unregistered, registered,
        "case the unregistered id: the fixture must not name the registered image"
    );
    let (status, payload) = call(
        &router,
        "CreateWorkspaceInstance",
        create_body(unregistered, "tok-unregistered"),
    )
    .await;
    assert_eq!(
        status, VALIDATION_STATUS,
        "case the unregistered id {unregistered:?}: expected HTTP {VALIDATION_STATUS}, got \
         {status} with {payload}"
    );
}
