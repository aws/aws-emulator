// SPDX-License-Identifier: CC0-1.0

//! What a LAUNCH does with an `ImageId` that names a Systems Manager parameter, and the EBS
//! attachment lifecycle that launch feeds — driven over the ec2Query wire the service speaks,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A LAUNCH RESOLVES A SYSTEMS MANAGER PARAMETER TO AN AMI ID: "alternatively, you can
//!    specify a Systems Manager parameter […] The Systems Manager parameter will resolve to an
//!    AMI ID on launch. To reference a public parameter: `resolve:ssm:public-parameter`"
//!    (`RequestLaunchTemplateData$ImageId`), so a launch given such a value runs from the AMI
//!    the parameter names and the instance reports THAT id — never the parameter. The rows are
//!    the whole published catalog, read out of the store rather than written here, so every
//!    name is covered rather than the one a request happened to carry;
//! 2. ONLY A VALUE THAT NAMES AN AMI DIRECTLY IS READ AS AN ID: such a value is still held to
//!    the issued-id form (`InvalidAMIID.Malformed`), while a reference Parameter Store produces
//!    no value for names no AMI at all and is refused as the invalid ALIAS it is
//!    (`SsmInvalidParameter`);
//! 3. THE EBS ATTACHMENT LIFECYCLE IS THE ONE PROJECTION OF ONE RECORD, end to end from such a
//!    launch: `AttachVolume` records the attachment, so `DescribeVolumes` reports the volume
//!    `in-use` with that attachment — `attaching` until the clock owes it `attached` — and the
//!    matching `DetachVolume` takes it down with 200 and reports it `detaching`;
//! 4. AN ATTACHMENT THE VOLUME DOES NOT HOLD IS NOT A STATE PROBLEM: a detach naming one
//!    answers `InvalidAttachment.NotFound`, whose message names the volume and whichever of the
//!    instance and the device the request gave.
//!
//! Each table row is a distinct input along what the rule generalises over — which published
//! parameter name, which form the `ImageId` value takes, which member of the detach selects an
//! attachment the volume does not hold — so no row is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_INSTANCE_TYPE, SSM_IMAGE_ALIAS_PREFIX,
    VOLUME_ATTACHMENT_STATE_ATTACHED, VOLUME_ATTACHMENT_STATE_ATTACHING,
    VOLUME_ATTACHMENT_STATE_DETACHING, VOLUME_STATE_AVAILABLE, VOLUME_STATE_IN_USE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::volume_lifecycle::{REGION, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// An AMI id of the form EC2 issues, for the rows that name an AMI directly.
const ISSUED_AMI: &str = "ami-0123456789abcdef0";

/// A value inside the generic AMI id form but outside the one EC2 issues — 17 hexadecimal
/// digits under a leading digit EC2's own mint never produces.
const UNISSUABLE_AMI: &str = "ami-1234567890abcdef0";

/// A public parameter name the catalog publishes no image under, in the namespace the public
/// names live in (`Image$PublicSsmParameterName`: "under the `aws/service/` namespace").
const UNPUBLISHED_PARAMETER: &str = "/aws/service/ami-amazon-linux-latest/no-such-image";

/// One `action` call signed for the Region the suite works in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The catalog a launch resolves a parameter against: every public Systems Manager parameter
/// name this account resolves, and the id of the AMI it names. Read out of the store the
/// handler reads, so the table below is the service's own catalog rather than a copy of it.
fn catalog(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .public_ssm_parameter_image_ids(engine::emulator_account_id())
        .into_iter()
        .collect()
}

/// One launch of a single instance from `image_id`, as its HTTP status and XML body.
///
/// `runs` is the AMI the launch will really run from: the same value for a request naming an
/// AMI id, and the AMI the parameter resolves to for one naming a `resolve:ssm:` parameter. An
/// instance runs the machine code its AMI holds, so the launch names an instance type of THAT
/// AMI's architecture, read off the AMI rather than written here.
async fn launch_running(
    router: &MultiServiceRouter,
    image_id: &str,
    runs: &str,
) -> (u16, String) {
    let mut params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
        p("InstanceType", DEFAULT_INSTANCE_TYPE),
    ];
    if let Some((_, instance_type)) =
        super::ec2_query::launch_type_for_image(router, REGION, "the launch", runs)
            .await
            .first()
    {
        params.pop();
        params.push(p("InstanceType", instance_type.clone()));
    }
    op(router, "RunInstances", &params).await
}

/// The same launch for a request whose `ImageId` IS the AMI it runs.
async fn launch(router: &MultiServiceRouter, image_id: &str) -> (u16, String) {
    launch_running(router, image_id, image_id).await
}

/// The `imageId` `DescribeInstances` reports for `instance_id` — the read-back half of rule 1,
/// so the resolved id is asserted where a later caller sees it and not only in the launch's own
/// answer.
async fn described_image_id(case: &str, router: &MultiServiceRouter, instance_id: &str) -> String {
    let (status, body) = op(
        router,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstances, got {status} with body {body}"
    );
    element(&body, "imageId")
        .unwrap_or_else(|| panic!("case {case:?}: no <imageId> in {body}"))
}

/// One 8 GiB volume in the Region's first zone — the zone a launch that names none places its
/// instance in, so an attach of it is admitted.
async fn create_volume(router: &MultiServiceRouter) -> String {
    let (status, body) = op(
        router,
        "CreateVolume",
        &[p("AvailabilityZone", zone_name(0)), p("Size", "8")],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVolume must succeed, got {status} with body {body}"
    );
    element(&body, "volumeId").unwrap_or_else(|| panic!("no <volumeId> in {body}"))
}

/// What `DescribeVolumes` reports for one volume: the volume's status and, for each attachment
/// it holds, the instance, the device and the attachment's own status.
async fn described_volume(
    case: &str,
    router: &MultiServiceRouter,
    volume_id: &str,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (status, body) = op(router, "DescribeVolumes", &indexed("VolumeId", &[volume_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVolumes, got {status} with body {body}"
    );
    (
        elements(&body, "status"),
        elements(&body, "instanceId"),
        elements(&body, "device"),
    )
}

/// RULE 1: every public parameter name the catalog publishes resolves, ON LAUNCH, to the AMI id
/// that name publishes — reported by the launch and by the later describe of the instance it
/// minted, and never as the parameter the request carried.
#[tokio::test]
async fn every_published_parameter_name_launches_from_the_ami_it_resolves_to() {
    let (router, engine) = emulator();
    let published = catalog(&engine);
    assert!(
        published.len() > 1,
        "the image catalog must publish more than one parameter name to span, got {published:?}"
    );

    for (parameter, image_id) in &published {
        let case = format!("{parameter} -> {image_id}");
        let alias = format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}");
        let (status, body) = launch_running(&router, &alias, image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from RunInstances, got {status} with body {body}"
        );
        let reported = element(&body, "imageId")
            .unwrap_or_else(|| panic!("case {case:?}: no <imageId> in {body}"));
        assert_eq!(
            &reported, image_id,
            "case {case:?}: expected the launch to report the resolved AMI {image_id}, got \
             {reported:?}"
        );
        let instance_id = element(&body, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <instanceId> in {body}"));
        let read_back = described_image_id(&case, &router, &instance_id).await;
        assert_eq!(
            &read_back, image_id,
            "case {case:?}: expected DescribeInstances to report {image_id}, got {read_back:?}"
        );
        assert_ne!(
            read_back, alias,
            "case {case:?}: the instance must not report the parameter it was launched from"
        );
    }
}

/// RULE 2: which `ImageId` forms a launch takes and which it refuses — a value naming an AMI
/// directly is still held to the issued-id form, the prefix alone names no parameter at all, and
/// a reference nothing resolves names no AMI.
#[tokio::test]
async fn a_launch_refuses_the_image_id_values_that_name_no_image() {
    let (router, engine) = emulator();
    let published = catalog(&engine);
    let (published_parameter, published_image) = published
        .first()
        .cloned()
        .expect("the image catalog must publish at least one parameter name");

    // (case, the ImageId the launch carries, the expected status, the expected error code, the
    // imageId the instance must report)
    let rows: Vec<(&str, String, u16, &str, Option<String>)> = vec![
        (
            "an AMI id of the form EC2 issues",
            ISSUED_AMI.to_string(),
            200,
            "",
            Some(ISSUED_AMI.to_string()),
        ),
        (
            "a published parameter",
            format!("{SSM_IMAGE_ALIAS_PREFIX}{published_parameter}"),
            200,
            "",
            Some(published_image),
        ),
        (
            "an id outside the form EC2 issues",
            UNISSUABLE_AMI.to_string(),
            400,
            "InvalidAMIID.Malformed",
            None,
        ),
        (
            "the parameter prefix with no parameter after it",
            SSM_IMAGE_ALIAS_PREFIX.to_string(),
            400,
            "InvalidAMIID.Malformed",
            None,
        ),
        (
            "a parameter the catalog publishes no image under",
            format!("{SSM_IMAGE_ALIAS_PREFIX}{UNPUBLISHED_PARAMETER}"),
            400,
            "SsmInvalidParameter",
            None,
        ),
        (
            "a published parameter name with one segment missing",
            format!("{SSM_IMAGE_ALIAS_PREFIX}/aws/service/ami-amazon-linux-latest"),
            400,
            "SsmInvalidParameter",
            None,
        ),
    ];

    for (case, image_id, expected_status, expected_code, expected_reported) in rows {
        // The AMI the launch will really run from, which for a row naming a parameter is the
        // AMI that parameter resolves to — the id the row already expects reported.
        let runs = expected_reported.clone().unwrap_or_else(|| image_id.clone());
        let (status, body) = launch_running(&router, &image_id, &runs).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        match expected_reported {
            Some(expected) => assert_eq!(
                element(&body, "imageId").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the launch to report {expected}, got {body}"
            ),
            None => assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            ),
        }
    }
}

/// RULE 3: the attachment lifecycle, end to end from a launch that named a parameter — the
/// attach is recorded, so the describe reports the volume `in-use` with that attachment, the
/// clock brings it to `attached`, and the matching detach succeeds and reports `detaching`
/// until the clock frees the volume.
#[tokio::test]
async fn a_volume_attached_to_an_instance_launched_from_a_parameter_reads_back_and_detaches() {
    let (router, engine) = emulator();
    let (parameter, resolved_image) = catalog(&engine)
        .first()
        .cloned()
        .expect("the image catalog must publish at least one parameter name");
    let alias = format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}");
    let (status, body) = launch_running(&router, &alias, &resolved_image).await;
    assert_eq!(
        status, 200,
        "the launch must succeed, got {status} with body {body}"
    );
    let instance_id =
        element(&body, "instanceId").unwrap_or_else(|| panic!("no <instanceId> in {body}"));
    let volume_id = create_volume(&router).await;
    // Both the instance's `pending` -> `running` move and the volume's `creating` ->
    // `available` one are owed by the calls above, and an attach needs both of them made.
    past_the_transition().await;

    let device = "/dev/sdf";
    let attach: Params = vec![
        p("VolumeId", &volume_id),
        p("InstanceId", &instance_id),
        p("Device", device),
    ];
    let (status, body) = op(&router, "AttachVolume", &attach).await;
    assert_eq!(
        status, 200,
        "the attach must succeed, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(VOLUME_ATTACHMENT_STATE_ATTACHING),
        "the attach reports the attachment it recorded as \
         {VOLUME_ATTACHMENT_STATE_ATTACHING}, got {body}"
    );

    // The volume holds the attachment from that moment, so the describe reports it in use with
    // the attachment the attach recorded — in the state the clock has it in.
    let (statuses, instances, devices) =
        described_volume("just attached", &router, &volume_id).await;
    assert!(
        statuses.contains(&VOLUME_STATE_IN_USE.to_string()),
        "a volume holding an attachment is {VOLUME_STATE_IN_USE}, got {statuses:?}"
    );
    assert!(
        statuses.contains(&VOLUME_ATTACHMENT_STATE_ATTACHING.to_string()),
        "the attachment is reported {VOLUME_ATTACHMENT_STATE_ATTACHING} inside its span, got \
         {statuses:?}"
    );
    assert_eq!(
        instances,
        vec![instance_id.clone()],
        "the attachment names the instance it was made to, got {instances:?}"
    );
    assert_eq!(
        devices,
        vec![device.to_string()],
        "the attachment names the device the request gave, got {devices:?}"
    );

    past_the_transition().await;
    let (statuses, _, _) = described_volume("past the span", &router, &volume_id).await;
    assert!(
        statuses.contains(&VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
        "the clock, not a caller, brings the attachment to \
         {VOLUME_ATTACHMENT_STATE_ATTACHED}, got {statuses:?}"
    );
    assert!(
        statuses.contains(&VOLUME_STATE_IN_USE.to_string()),
        "the volume is still {VOLUME_STATE_IN_USE}, got {statuses:?}"
    );

    let (status, body) = op(&router, "DetachVolume", &attach).await;
    assert_eq!(
        status, 200,
        "the matching detach must succeed, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(VOLUME_ATTACHMENT_STATE_DETACHING),
        "the detach reports the attachment as {VOLUME_ATTACHMENT_STATE_DETACHING}, got {body}"
    );
    assert_eq!(
        element(&body, "device").as_deref(),
        Some(device),
        "the detach reports the device of the attachment it took down, got {body}"
    );

    past_the_transition().await;
    let (statuses, instances, _) = described_volume("past the detach", &router, &volume_id).await;
    assert_eq!(
        statuses,
        vec![VOLUME_STATE_AVAILABLE.to_string()],
        "once the detach falls due the volume holds no attachment and is \
         {VOLUME_STATE_AVAILABLE}, got {statuses:?}"
    );
    assert!(
        instances.is_empty(),
        "no attachment is left to report an instance, got {instances:?}"
    );
}

/// RULE 4: a detach naming an attachment the volume does not hold answers
/// `InvalidAttachment.NotFound` — the volume is in a state that admits a detach, so what is
/// missing is the attachment — and the message names the volume and whichever of the instance
/// and the device the request gave.
#[tokio::test]
async fn a_detach_of_an_attachment_the_volume_does_not_hold_is_not_found() {
    let (router, engine) = emulator();
    let (parameter, resolved_image) = catalog(&engine)
        .first()
        .cloned()
        .expect("the image catalog must publish at least one parameter name");
    let (status, body) = launch_running(
        &router,
        &format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}"),
        &resolved_image,
    )
    .await;
    assert_eq!(
        status, 200,
        "the launch must succeed, got {status} with body {body}"
    );
    let instance_id =
        element(&body, "instanceId").unwrap_or_else(|| panic!("no <instanceId> in {body}"));
    let volume_id = create_volume(&router).await;
    past_the_transition().await;
    let held_device = "/dev/sdf";
    let (status, body) = op(
        &router,
        "AttachVolume",
        &[
            p("VolumeId", &volume_id),
            p("InstanceId", &instance_id),
            p("Device", held_device),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the attach must succeed, got {status} with body {body}"
    );

    // A second instance, so a row can name one the volume is not attached to.
    let (status, body) = launch(&router, ISSUED_AMI).await;
    assert_eq!(
        status, 200,
        "the second launch must succeed, got {status} with body {body}"
    );
    let other_instance =
        element(&body, "instanceId").unwrap_or_else(|| panic!("no <instanceId> in {body}"));

    // (case, the members selecting the attachment, the sentence the refusal carries)
    let rows: Vec<(&str, Params, String)> = vec![
        (
            "the holding instance at another device",
            vec![
                p("VolumeId", &volume_id),
                p("InstanceId", &instance_id),
                p("Device", "/dev/sdg"),
            ],
            format!(
                "The volume '{volume_id}' is not attached to instance '{instance_id}' as \
                 device '/dev/sdg'."
            ),
        ),
        (
            "another instance at the held device",
            vec![
                p("VolumeId", &volume_id),
                p("InstanceId", &other_instance),
                p("Device", held_device),
            ],
            format!(
                "The volume '{volume_id}' is not attached to instance '{other_instance}' as \
                 device '{held_device}'."
            ),
        ),
        (
            "another instance and no device",
            vec![p("VolumeId", &volume_id), p("InstanceId", &other_instance)],
            format!("The volume '{volume_id}' is not attached to instance '{other_instance}'."),
        ),
        (
            "a device the volume is not attached at and no instance",
            vec![p("VolumeId", &volume_id), p("Device", "/dev/sdh")],
            format!("The volume '{volume_id}' is not attached as device '/dev/sdh'."),
        ),
    ];

    for (case, params, expected_message) in rows {
        let (status, body) = op(&router, "DetachVolume", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAttachment.NotFound",
            "case {case:?}: expected InvalidAttachment.NotFound, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            error_message(&body)
        );
    }

    // And the attachment the volume DOES hold is untouched by every refusal above.
    let (statuses, instances, devices) =
        described_volume("after the refusals", &router, &volume_id).await;
    assert!(
        statuses.contains(&VOLUME_STATE_IN_USE.to_string()),
        "the volume still holds its attachment and is {VOLUME_STATE_IN_USE}, got {statuses:?}"
    );
    assert_eq!(
        (instances, devices),
        (vec![instance_id.clone()], vec![held_device.to_string()]),
        "the held attachment is left exactly as it was"
    );
}
