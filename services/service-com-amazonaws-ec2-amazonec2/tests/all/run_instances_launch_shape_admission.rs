// SPDX-License-Identifier: CC0-1.0

//! Which LAUNCH SHAPES `RunInstances` admits, end to end over the wire the service speaks
//! (`ec2Query`, through the shared [`super::ec2_query`] harness).
//!
//! Four rules, each asked of a SET of inputs rather than of one recorded launch:
//!
//! 1. ARCHITECTURE — an instance runs the machine code its AMI holds, so a launch whose AMI
//!    is built for an architecture its instance type does not run is refused
//!    `InvalidParameterValue` and creates nothing. The verdict and its wording are the
//!    crate's one rule ([`ec2::state::architecture_mismatch_message`]), so no row writes a
//!    sentence of its own.
//! 2. HIBERNATION — "this parameter is valid only if the instance meets the hibernation
//!    prerequisites" (`RunInstancesRequest$HibernationOptions`): the instance type has to be
//!    one hibernation is offered on, the root device has to be an encrypted Amazon EBS volume,
//!    and that volume has to be larger than the memory it would have to store. Each row's
//!    expectation is derived from what `DescribeInstanceTypes` PUBLISHES about the type, so
//!    the table cannot drift from the catalog.
//! 3. SUBNET — a `SubnetId` the account holds no subnet under names a network no instance
//!    could be placed in, so the launch is refused `InvalidSubnetID.NotFound` and creates
//!    nothing, while a subnet the account does hold is served.
//! 4. CLIENT TOKEN — one token names the one launch it was spent on: a replay of that launch
//!    answers the reservation it minted and launches nothing further, and the same token with
//!    any other launch parameters is refused `IdempotentParameterMismatch`.
//!
//! Beside them, the STOP: `StopInstances` with `Hibernate=true` on an instance that was
//! launched for hibernation records the hibernation's own state reason, which is what tells a
//! caller a hibernation from a plain shutdown.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY,
    architecture_mismatch_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{
    call, described_instance_state, described_instance_type, element, elements, error_code,
    error_message, indexed, reported_instance_type_member,
};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The emulator and the engine behind it, so a suite can seed the AMIs its rows launch from.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Store one AMI of the given architecture and answer its id, through the store's own writer.
fn seed_image(engine: &engine::InMemoryEngine, architecture: &str, name: &str) -> String {
    engine.state::<Ec2State>().lock().insert_image(ImageSpec {
        name: name.to_string(),
        description: format!("the launch-admission suite's {architecture} image"),
        architecture: architecture.to_string(),
        ..ImageSpec::default()
    })
}

/// The root device name of a seeded image, which a block device mapping has to name to define
/// the launch's ROOT volume.
fn root_device(engine: &engine::InMemoryEngine, image_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .images
        .get(image_id)
        .and_then(|image| image.root_device_name.clone())
        .unwrap_or_else(|| panic!("the seeded image {image_id} must report a root device"))
}

/// One `RunInstances` of a single instance from `image_id`, with `extra` beside it.
async fn launch(
    router: &MultiServiceRouter,
    image_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    params.extend_from_slice(extra);
    call(router, REGION, "RunInstances", &params).await
}

/// How many instances the account holds, so a row can assert that a refused launch created
/// none.
async fn held_instances(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, body) = call(router, REGION, "DescribeInstances", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must succeed, got {status} with body {body}"
    );
    elements(&body, "instanceId").len()
}

/// Look again until `instance_id` reports the state `wanted`, or a generous deadline passes.
///
/// The move is work the CLOCK owes, so the condition is looked at AGAIN in a loop rather than
/// waited out: nothing here depends on how fast the host runs, and the deadline is a multiple
/// of the service's own transition span.
async fn poll_until_state(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    wanted: &str,
) {
    let deadline = std::time::Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let state = described_instance_state(router, REGION, instance_id).await;
        if state == wanted {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {instance_id} must reach {wanted:?}, last read {state:?}"
        );
        tokio::time::sleep(TRANSITION_DELAY / 10).await;
    }
}

/// RULE 1: a launch is admitted on whether its AMI's architecture is one its instance type
/// runs, and a refused launch creates no instance.
///
/// The rows span both directions of the mismatch and both of the agreements, over three
/// instance families — so neither the arm64 nor the x86_64 side is one recorded pair.
#[tokio::test]
async fn a_launch_is_admitted_on_the_amis_architecture_against_its_instance_type() {
    let (router, engine) = emulator();
    let arm64 = seed_image(&engine, "arm64", "admission-arm64");
    let x86_64 = seed_image(&engine, "x86_64", "admission-x86-64");

    // (the AMI's architecture, the AMI, the instance type) — the expectation is DERIVED from
    // the crate's one architecture rule rather than written per row.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("x86_64", x86_64.as_str(), "t3.micro"),
        ("arm64", arm64.as_str(), "t3.micro"),
        ("x86_64", x86_64.as_str(), "t4g.micro"),
        ("arm64", arm64.as_str(), "t4g.micro"),
        // An entry beyond the two the pair above names: another Graviton family, and a
        // compute type of an x86 one.
        ("arm64", arm64.as_str(), "c6g.large"),
        ("x86_64", x86_64.as_str(), "c6g.large"),
        ("x86_64", x86_64.as_str(), "m5.large"),
        ("arm64", arm64.as_str(), "m5.large"),
    ];

    for (architecture, image_id, instance_type) in rows {
        let case = format!("a {architecture} AMI on {instance_type}");
        let before = held_instances(&router, &case).await;
        let (status, body) = launch(&router, image_id, &[p("InstanceType", instance_type)]).await;
        match architecture_mismatch_message(architecture, instance_type) {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: matching architectures must be served, got {status} with \
                     body {body}"
                );
                assert_eq!(
                    element(&body, "architecture").as_deref(),
                    Some(architecture),
                    "case {case:?}: the instance must report the AMI's architecture, got {body}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidParameterValue"),
                    "case {case:?}: a mismatched architecture must be refused, got {status} \
                     with body {body}"
                );
                assert_eq!(
                    error_message(&body),
                    expected,
                    "case {case:?}: expected the architecture sentence, got body {body}"
                );
                assert_eq!(
                    held_instances(&router, &case).await,
                    before,
                    "case {case:?}: a refused launch must create no instance"
                );
            }
        }
    }
}

/// What the catalog publishes about one instance type, for the hibernation rows: whether
/// hibernation is offered on it and the memory an instance of it runs with, in GiB rounded up
/// to the whole GiB a volume is sized in.
async fn published_hibernation(
    router: &MultiServiceRouter,
    case: &str,
    instance_type: &str,
) -> (bool, i64) {
    let body = described_instance_type(case, router, REGION, instance_type).await;
    let supported = reported_instance_type_member(case, &body, &["hibernationSupported"])
        .unwrap_or_else(|| panic!("case {case:?}: {instance_type} publishes no hibernation support"))
        == "true";
    let memory_mib = reported_instance_type_member(case, &body, &["memoryInfo", "sizeInMiB"])
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: {instance_type} publishes no memory size"));
    (supported, memory_mib.max(0).div_euclid(1_024) + i64::from(memory_mib % 1_024 != 0))
}

/// RULE 2: a hibernation-enabled launch is admitted on the three prerequisites together, and
/// a refused one creates nothing.
///
/// The rows span the instance type (offered, not offered), the root device's encryption, and
/// the root volume's size against the memory it would have to store — with the expectation
/// derived from what the catalog publishes for the type, so a type whose published facts
/// change is still covered.
#[tokio::test]
async fn a_hibernation_launch_is_admitted_on_the_published_prerequisites() {
    let (router, engine) = emulator();
    let image_id = seed_image(&engine, "x86_64", "admission-hibernation");
    let device = root_device(&engine, &image_id);

    // (instance type, the root volume's size in GiB, whether the mapping encrypts it)
    let rows: Vec<(&str, i32, bool)> = vec![
        // A burstable type hibernation IS offered on, whose memory a default-sized root
        // volume holds.
        ("t3.micro", 8, true),
        ("t3.micro", 8, false),
        // A type whose memory is exactly the size asked for: the volume has to hold the
        // memory AND what the instance boots from, so the same size is admitted for the
        // smaller type above and refused here.
        ("m5.large", 8, true),
        ("m5.large", 40, true),
        ("m5.large", 40, false),
        // Types the published prerequisites do not name: an AMD variant of a compute family
        // and a GPU family — neither of them the type the reported case named.
        ("c5a.large", 40, true),
        ("g4dn.xlarge", 200, true),
    ];

    for (instance_type, size_gib, encrypted) in rows {
        let case = format!(
            "hibernation on {instance_type} over a {size_gib} GiB root volume \
             (encrypted={encrypted})"
        );
        let (supported, memory_gib) = published_hibernation(&router, &case, instance_type).await;
        let before = held_instances(&router, &case).await;
        let (status, body) = launch(
            &router,
            &image_id,
            &[
                p("InstanceType", instance_type),
                p("HibernationOptions.Configured", "true"),
                p("BlockDeviceMapping.1.DeviceName", device.clone()),
                p("BlockDeviceMapping.1.Ebs.VolumeSize", size_gib.to_string()),
                p(
                    "BlockDeviceMapping.1.Ebs.Encrypted",
                    if encrypted { "true" } else { "false" },
                ),
            ],
        )
        .await;

        let admitted = supported && encrypted && i64::from(size_gib) > memory_gib;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: a launch meeting every prerequisite must be served, got \
                 {status} with body {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "UnsupportedHibernationConfiguration"),
            "case {case:?}: an unmet prerequisite must be refused, got {status} with body {body}"
        );
        let message = error_message(&body);
        if !supported {
            assert!(
                message.contains(instance_type),
                "case {case:?}: the refusal must name the type, got {message:?}"
            );
        } else if !encrypted {
            assert!(
                message.contains("encrypted"),
                "case {case:?}: the refusal must name the encryption, got {message:?}"
            );
        } else {
            assert!(
                message.contains("memory"),
                "case {case:?}: the refusal must name the memory the volume has to store, \
                 got {message:?}"
            );
        }
        assert_eq!(
            held_instances(&router, &case).await,
            before,
            "case {case:?}: a refused launch must create no instance"
        );
    }
}

/// RULE 3: the `SubnetId` a launch names has to be a subnet the account holds.
///
/// The rows span both id forms EC2 issues a subnet id in and a value no subnet of the account
/// has, beside the subnet the account really holds — which is read out of
/// `DescribeSubnets` rather than written here.
#[tokio::test]
async fn a_launch_is_admitted_on_the_subnet_it_names() {
    let (router, engine) = emulator();
    let image_id = seed_image(&engine, "x86_64", "admission-subnet");

    let (status, body) = call(&router, REGION, "DescribeSubnets", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeSubnets must report the Region's default subnets, got {status} with body {body}"
    );
    let held = elements(&body, "subnetId");
    let held_subnet = held
        .first()
        .unwrap_or_else(|| panic!("the Region must have a default subnet: {body}"))
        .clone();

    // (case, the SubnetId the launch names, whether the account holds it)
    let rows: Vec<(&str, String, bool)> = vec![
        ("the subnet the account holds", held_subnet.clone(), true),
        (
            "a short-form subnet id the account has none under",
            "subnet-1a2b3c4d".to_string(),
            false,
        ),
        (
            "a long-form subnet id the account has none under",
            "subnet-0123456789abcdef0".to_string(),
            false,
        ),
        (
            "the id of another resource kind entirely",
            "subnet-00000000000000000".to_string(),
            false,
        ),
    ];

    for (case, subnet_id, exists) in rows {
        let before = held_instances(&router, case).await;
        let (status, body) = launch(&router, &image_id, &[p("SubnetId", subnet_id.clone())]).await;
        if exists {
            assert_eq!(
                status, 200,
                "case {case:?}: a launch into a held subnet must be served, got {status} with \
                 body {body}"
            );
            assert_eq!(
                element(&body, "subnetId").as_deref(),
                Some(subnet_id.as_str()),
                "case {case:?}: the instance must report the subnet it was placed in, got {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidSubnetID.NotFound"),
            "case {case:?}: an unknown subnet must be refused, got {status} with body {body}"
        );
        assert!(
            error_message(&body).contains(&subnet_id),
            "case {case:?}: the refusal must name the subnet, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            held_instances(&router, case).await,
            before,
            "case {case:?}: a refused launch must create no instance"
        );
    }
}

/// RULE 4: one `ClientToken` names one launch — a replay answers that launch's reservation,
/// and the same token with any other parameters is refused.
///
/// The rows span the member that differs: the instance type, the count, the AMI and the user
/// data, so the comparison is over the whole launch rather than over one member.
#[tokio::test]
async fn a_client_token_names_the_one_launch_it_was_spent_on() {
    let (router, engine) = emulator();
    let image_id = seed_image(&engine, "x86_64", "admission-token");
    let other_image = seed_image(&engine, "x86_64", "admission-token-other");

    let token = "admission-suite-token";
    // The whole launch, spelled out: the rows below differ from it in ONE member each, so no
    // row can carry a member twice.
    let spelled_out = |extra: &[(&str, String)]| -> Vec<(String, String)> {
        let mut params = vec![
            p("ClientToken", token),
            p("ImageId", image_id.clone()),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("InstanceType", "t3.micro"),
            p("UserData", "IyEvYmluL3No"),
        ];
        for (name, value) in extra {
            if let Some(slot) = params.iter_mut().find(|(held, _)| held == name) {
                slot.1 = value.clone();
            } else {
                params.push(p(name, value.clone()));
            }
        }
        params
    };
    let first = spelled_out(&[]);
    let (status, body) = call(&router, REGION, "RunInstances", &first).await;
    assert_eq!(
        status, 200,
        "the first launch under the token must be served, got {status} with body {body}"
    );
    let reservation = element(&body, "reservationId")
        .unwrap_or_else(|| panic!("the launch must report its reservation: {body}"));
    let after_first = held_instances(&router, "the first launch").await;

    // The SAME launch again: the reservation the token already minted, and nothing more.
    let (status, body) = call(&router, REGION, "RunInstances", &first).await;
    assert_eq!(
        (status, element(&body, "reservationId").as_deref()),
        (200, Some(reservation.as_str())),
        "a replay of the same launch must answer the reservation the token minted, got \
         {status} with body {body}"
    );
    assert_eq!(
        held_instances(&router, "the replay").await,
        after_first,
        "a replay must launch nothing further"
    );

    // (case, the member of the launch that differs under the same token)
    let rows: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "another instance type",
            vec![("InstanceType", "m5.large".to_string())],
        ),
        (
            "another count",
            vec![("MaxCount", "2".to_string())],
        ),
        (
            "other user data",
            vec![("UserData", "IyEvYmluL2Jhc2g=".to_string())],
        ),
        (
            "another AMI",
            vec![("ImageId", other_image.clone())],
        ),
        (
            "a member the first launch did not carry at all",
            vec![("EbsOptimized", "true".to_string())],
        ),
    ];

    for (case, difference) in rows {
        let (status, body) = call(&router, REGION, "RunInstances", &spelled_out(&difference)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "IdempotentParameterMismatch"),
            "case {case:?}: a token replayed with other parameters must be refused, got \
             {status} with body {body}"
        );
        assert_eq!(
            held_instances(&router, case).await,
            after_first,
            "case {case:?}: a refused replay must launch nothing"
        );
    }

    // A DIFFERENT token is a different launch, which is served.
    let mut other_token = spelled_out(&[]);
    other_token[0] = p("ClientToken", "admission-suite-token-2");
    let (status, body) = call(&router, REGION, "RunInstances", &other_token).await;
    assert_eq!(
        status, 200,
        "another token must name another launch, got {status} with body {body}"
    );
    assert_ne!(
        element(&body, "reservationId").as_deref(),
        Some(reservation.as_str()),
        "another token must mint its own reservation, got {body}"
    );
}

/// THE STOP: a stop that HIBERNATES an instance records the hibernation's own state reason,
/// and a plain stop records the shutdown's — so a caller reading the instance back can tell
/// the two apart.
///
/// Both rows drive the same instance shape, differing only in the `Hibernate` the stop asks
/// for.
#[tokio::test]
async fn a_hibernating_stop_records_the_hibernations_own_state_reason() {
    let (router, engine) = emulator();
    let image_id = seed_image(&engine, "x86_64", "admission-stop");
    let device = root_device(&engine, &image_id);

    // (case, whether the stop asks to hibernate, the state reason code it must record)
    let rows: Vec<(&str, bool, &str)> = vec![
        ("a hibernating stop", true, "Client.UserInitiatedHibernate"),
        ("a plain stop", false, "Client.UserInitiatedShutdown"),
    ];

    for (case, hibernate, expected_code) in rows {
        let (status, body) = launch(
            &router,
            &image_id,
            &[
                p("InstanceType", "t3.micro"),
                p("HibernationOptions.Configured", "true"),
                p("BlockDeviceMapping.1.DeviceName", device.clone()),
                p("BlockDeviceMapping.1.Ebs.VolumeSize", "8"),
                p("BlockDeviceMapping.1.Ebs.Encrypted", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the hibernation-enabled launch must be served, got {status} with \
             body {body}"
        );
        let instance_id = element(&body, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"));
        poll_until_state(&router, case, &instance_id, "running").await;

        let mut stop = vec![p("InstanceId.1", instance_id.clone())];
        if hibernate {
            stop.push(p("Hibernate", "true"));
        }
        let (status, body) = call(&router, REGION, "StopInstances", &stop).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stop must be served, got {status} with body {body}"
        );
        poll_until_state(&router, case, &instance_id, "stopped").await;

        let described = call(
            &router,
            REGION,
            "DescribeInstances",
            &indexed("InstanceId", &[&instance_id]),
        )
        .await;
        let reported = super::wire::document(case, &described.1);
        let reason = reported
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "stateReason")
            .and_then(|reason| super::wire::child_text(reason, "code"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stopped instance must report a state reason: {:?}", described.1)
            });
        assert_eq!(
            reason, expected_code,
            "case {case:?}: expected the state reason {expected_code:?}, got {reason:?}"
        );
    }
}
