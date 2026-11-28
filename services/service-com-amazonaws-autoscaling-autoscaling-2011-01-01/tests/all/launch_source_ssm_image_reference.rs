// SPDX-License-Identifier: CC0-1.0

//! THE AMI AN AUTO SCALING GROUP LAUNCHES when its launch source names the image through a
//! Systems Manager parameter instead of an AMI id.
//!
//! RULE under test: "Alternatively, you can specify a Systems Manager parameter, using one of
//! the following formats. The Systems Manager parameter WILL RESOLVE TO AN AMI ID ON LAUNCH"
//! (`RequestLaunchTemplateData$ImageId`) — Amazon EC2 stores such a reference unresolved, so
//! this service, as the CONSUMER of the launch template, resolves it when it launches: the
//! instance a capacity change places runs the AMI the version the reference selects holds, and
//! reports THAT id rather than the reference. A reference the account's Parameter Store
//! produces no value for leaves the launch with no AMI at all, so the capacity change places
//! nothing and records a terminal `Failed` activity carrying Amazon EC2's own sentence for an
//! alias it could not resolve, followed by the sentence this service ends every failed launch
//! with. The resolution is made at the instant of the LAUNCH, so a parameter a client deletes
//! stops resolving and one it restores resolves again, with nobody asking for the capacity a
//! second time.
//!
//! The rows span what the rule generalises over: WHICH documented form the reference takes (a
//! parameter name, that name with a `:<version>` selector, that name with a `:<label>`
//! selector, and the parameter ARN) and WHICH version of the parameter each selects — the
//! latest, or an older one that holds a DIFFERENT AMI, so no row can pass by resolving "the
//! parameter" rather than the version it names.
//!
//! Auto Scaling, Amazon EC2 and Systems Manager are mounted on ONE engine and driven over
//! their own wire protocols — awsQuery, ec2Query and `awsJson1_1` — so every image, parameter,
//! template, instance and activity is written and read where a client writes and reads it, and
//! every AMI id asserted is one Amazon EC2 minted for an image this suite registered.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state as ec2;
use service_com_amazonaws_ssm_amazonssm as ssm_service;

use crate::support::{
    borrowed, ec2_query, emulator, json_rpc, list_members, p, query, tag_value, tag_values,
    xml_unescape, REGION,
};

/// How long a case keeps looking for the outcome it waits on before it gives up — generous on
/// purpose, so a loaded host is never the reason a case fails.
const DEADLINE: Duration = Duration::from_secs(120);

/// How long one look-again cycle pauses before asking again.
const POLL_PAUSE: Duration = Duration::from_millis(25);

/// The status codes a scaling activity has FINISHED in, from the model's own
/// `ScalingActivityStatusCode`.
const TERMINAL_STATUS_CODES: &[&str] = &["Successful", "Failed", "Cancelled"];

/// The prefix an `ImageId` carries when it names a Systems Manager parameter instead of an AMI,
/// written out here as `RequestLaunchTemplateData$ImageId` documents it rather than imported,
/// so a wrong constant in either implementation cannot make this suite pass.
const ALIAS_PREFIX: &str = "resolve:ssm:";

/// The FIXED text Amazon EC2 states for an image alias it could not resolve, and the sentence
/// Amazon EC2 Auto Scaling ends every failed launch with — likewise written out here.
const INVALID_ALIASES_PREFIX: &str = "The following aliases are invalid: ";
const LAUNCH_FAILED: &str = "Launching EC2 instance failed.";

/// The data type a parameter whose value is an AMI id is created with — "when you create a
/// String parameter and specify aws:ec2:image, Amazon Web Services Systems Manager validates
/// the parameter value is in the required format" (`PutParameter$DataType`).
const IMAGE_DATA_TYPE: &str = "aws:ec2:image";

/// The architecture every image here is registered for, and the instance type that runs it: an
/// instance runs the machine code its AMI holds, so the pair has to agree.
const ARCHITECTURE: &str = "x86_64";
const INSTANCE_TYPE: &str = "t3.micro";

/// The label a `<name>:<label>` reference selects a version by.
const LABEL: &str = "released";

/// The zone every group here places its instances in: the first of the Region's own catalog,
/// never written out.
fn zone() -> String {
    ec2::availability_zones_in_region(REGION)
        .first()
        .map(|zone| zone.zone_name.clone())
        .expect("the Region has Availability Zones")
}

/// What a capacity change records for a reference Parameter Store produces no value for.
fn invalid_alias_message(reference: &str) -> String {
    format!("{INVALID_ALIASES_PREFIX}{reference}. {LAUNCH_FAILED}")
}

/// The shared three-service emulator with Systems Manager mounted beside them, on that same
/// engine: the parameter a launch source's `ImageId` names is that service's resource, so a
/// launch that resolves one resolves it where it lives.
fn emulator_with_parameter_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator();
    router.register_service(ssm_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One Systems Manager call, which must answer 200.
async fn ssm(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = json_rpc(router, "ssm", "AmazonSSM", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Systems Manager {operation} expected 200, got {status}: {parsed}"
    );
    parsed
}

/// Register one AMI of the account for [`ARCHITECTURE`], over Amazon EC2's own wire, and answer
/// the id EC2 minted for it.
async fn register_image(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "RegisterImage",
        &[
            p("Name", name),
            p("Architecture", ARCHITECTURE),
            p("RootDeviceName", "/dev/xvda"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RegisterImage for {name:?} expected 200, got {status}: {body}"
    );
    let image_id = tag_value(&body, "imageId");
    assert!(
        !image_id.is_empty(),
        "case {case:?}: RegisterImage must report the image's id, got {body}"
    );
    image_id
}

/// Put `value` as the next version of the parameter `name`, and answer the version number
/// Systems Manager allocated for it.
async fn put_image_parameter(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    value: &str,
) -> i64 {
    let answer = ssm(
        router,
        case,
        "PutParameter",
        json!({
            "Name": name,
            "Value": value,
            "Type": "String",
            "DataType": IMAGE_DATA_TYPE,
            "Overwrite": true,
        }),
    )
    .await;
    answer
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: no Version in {answer}"))
}

/// The ARN Systems Manager reports for the parameter `name` — taken off its own read rather
/// than written here, so the ARN row addresses the parameter by the ARN that service answers.
async fn parameter_arn(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let answer = ssm(router, case, "GetParameter", json!({ "Name": name })).await;
    answer
        .get("Parameter")
        .and_then(|parameter| parameter.get("ARN"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Parameter.ARN in {answer}"))
        .to_string()
}

/// Create one launch template naming `image_id` as its `ImageId`, over Amazon EC2's own wire.
async fn create_launch_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    image_id: &str,
) {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate for {name:?} expected 200, got {status}: {body}"
    );
}

/// Create one group holding nothing yet that launches from `template`.
async fn create_group(router: &MultiServiceRouter, case: &str, group: &str, template: &str) {
    let params = [
        p("AutoScalingGroupName", group),
        p("MinSize", "0"),
        p("MaxSize", "2"),
        p("DesiredCapacity", "0"),
        p("AvailabilityZones.member.1", zone()),
        p("LaunchTemplate.LaunchTemplateName", template),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup for {group:?} expected 200, got {status}: {body}"
    );
}

/// Ask the group for `capacity` units.
async fn ask_for(router: &MultiServiceRouter, case: &str, group: &str, capacity: &str) {
    let (status, body) = query(
        router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", group),
            ("DesiredCapacity", capacity),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetDesiredCapacity for {group:?} expected 200, got {status}: {body}"
    );
}

/// What one scaling activity reported.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    status_code: String,
    status_message: String,
}

/// The group's NEWEST scaling activity once it has reached a terminal status code, read through
/// `DescribeScalingActivities` the way a client polls for the outcome of the capacity change it
/// just asked for.
///
/// Looked for again until it is terminal or [`DEADLINE`] passes, so this waits for the OUTCOME
/// rather than for any length of time.
async fn settled_activity(router: &MultiServiceRouter, case: &str, group: &str) -> Outcome {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, body) = query(
            router,
            "DescribeScalingActivities",
            &[("AutoScalingGroupName", group)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeScalingActivities expected 200, got {status}: {body}"
        );
        let members = list_members(&body, "Activities");
        let newest = members.first().copied().unwrap_or_default();
        let status_code = tag_value(newest, "StatusCode");
        if TERMINAL_STATUS_CODES.contains(&status_code.as_str()) {
            return Outcome {
                status_code,
                status_message: xml_unescape(&tag_value(newest, "StatusMessage")),
            };
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the capacity change must record a terminal activity, got {body}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The ids of the instances the group holds, read through `DescribeAutoScalingGroups`.
async fn group_instances(router: &MultiServiceRouter, case: &str, group: &str) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}"
    );
    tag_values(&body, "InstanceId")
}

/// The group's instances once it holds `wanted` of them, or whatever it holds when [`DEADLINE`]
/// passes — a launch the clock still owes is not yet reported, so this waits for the CONDITION.
async fn instances_once_held(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    wanted: usize,
) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let held = group_instances(router, case, group).await;
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The `imageId` Amazon EC2 reports for `instance_id` — the read-back half of the rule, so the
/// resolved AMI is asserted where a client sees it rather than only inside this service.
async fn instance_image_id(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let (status, body) = ec2_query(router, "DescribeInstances", &[p("InstanceId.1", instance_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances expected 200, got {status}: {body}"
    );
    tag_value(&body, "imageId")
}

/// The instance ids the account's own Amazon EC2 store holds, so a refused launch is shown to
/// have minted nothing anywhere rather than merely to be unreported by the group.
async fn account_instances(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = ec2_query(router, "DescribeInstances", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances expected 200, got {status}: {body}"
    );
    tag_values(&body, "instanceId")
}

/// One row: which documented form the launch source's reference takes, and which of the
/// parameter's two versions it selects.
struct Row {
    case: &'static str,
    group: &'static str,
    /// Whether the reference names the parameter by its ARN rather than by its name.
    by_arn: bool,
    /// How the version is selected: `None` for the latest, `Some(true)` for the OLDER version
    /// by number, `Some(false)` for the older version by its label.
    older_by_number: Option<bool>,
}

/// RULE: a group whose launch source names its image through a Systems Manager parameter
/// launches the AMI the version that reference selects holds, in every documented form of the
/// reference.
#[tokio::test]
async fn a_group_launches_the_ami_its_image_reference_resolves_to() {
    let rows = [
        Row {
            case: "a parameter name, latest version",
            group: "ssm-image-name",
            by_arn: false,
            older_by_number: None,
        },
        Row {
            case: "a parameter name with an older version number",
            group: "ssm-image-name-version",
            by_arn: false,
            older_by_number: Some(true),
        },
        Row {
            case: "a parameter name with a label on the older version",
            group: "ssm-image-name-label",
            by_arn: false,
            older_by_number: Some(false),
        },
        Row {
            case: "the parameter ARN, latest version",
            group: "ssm-image-arn",
            by_arn: true,
            older_by_number: None,
        },
        Row {
            case: "the parameter ARN with an older version number",
            group: "ssm-image-arn-version",
            by_arn: true,
            older_by_number: Some(true),
        },
    ];

    for row in &rows {
        let case = row.case;
        // One emulator per row: each row is its own account, so the instances of one row never
        // explain another's.
        let (router, _engine) = emulator_with_parameter_store();
        let parameter = format!("/launch-source/{}/image-id", row.group);
        let older_image = register_image(&router, case, &format!("{}-older", row.group)).await;
        let newer_image = register_image(&router, case, &format!("{}-newer", row.group)).await;
        let older_version = put_image_parameter(&router, case, &parameter, &older_image).await;
        let newer_version = put_image_parameter(&router, case, &parameter, &newer_image).await;
        assert!(
            newer_version > older_version,
            "case {case:?}: the second put must allocate a later version, got {newer_version} \
             after {older_version}"
        );
        ssm(
            &router,
            case,
            "LabelParameterVersion",
            json!({
                "Name": parameter,
                "ParameterVersion": older_version,
                "Labels": [LABEL],
            }),
        )
        .await;

        let addressed = if row.by_arn {
            parameter_arn(&router, case, &parameter).await
        } else {
            parameter.clone()
        };
        let (reference, expected_image) = match row.older_by_number {
            None => (addressed.clone(), newer_image.clone()),
            Some(true) => (
                format!("{addressed}:{older_version}"),
                older_image.clone(),
            ),
            Some(false) => (format!("{addressed}:{LABEL}"), older_image.clone()),
        };
        let template = format!("{}-template", row.group);
        create_launch_template(
            &router,
            case,
            &template,
            &format!("{ALIAS_PREFIX}{reference}"),
        )
        .await;
        create_group(&router, case, row.group, &template).await;
        ask_for(&router, case, row.group, "1").await;

        let outcome = settled_activity(&router, case, row.group).await;
        assert_eq!(
            outcome.status_code, "Successful",
            "case {case:?}: a launch whose image reference resolves must succeed, got {outcome:?}"
        );
        let held = instances_once_held(&router, case, row.group, 1).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the group must hold the instance it launched, got {held:?}"
        );
        let reported = instance_image_id(&router, case, &held[0]).await;
        assert_eq!(
            reported, expected_image,
            "case {case:?}: expected the instance to run {expected_image}, got {reported:?}"
        );
        assert_ne!(
            reported, reference,
            "case {case:?}: the instance must not report the reference it was launched from"
        );
    }
}

/// RULE, second half: the reference is resolved at the instant of the LAUNCH, so a scale-out
/// made after the parameter is deleted places nothing and records Amazon EC2's own sentence for
/// the alias, and the capacity it left owed is placed once the parameter is restored — with
/// nobody asking for the capacity again.
#[tokio::test]
async fn a_scale_out_fails_while_the_parameter_is_absent_and_is_placed_once_it_is_back() {
    let case = "a group whose image parameter is deleted and restored";
    let (router, _engine) = emulator_with_parameter_store();
    let group = "ssm-image-absent";
    let parameter = format!("/launch-source/{group}/image-id");
    let image_id = register_image(&router, case, "ssm-image-absent-image").await;
    put_image_parameter(&router, case, &parameter, &image_id).await;
    let reference = format!("{ALIAS_PREFIX}{parameter}");
    create_launch_template(&router, case, "ssm-image-absent-template", &reference).await;
    create_group(&router, case, group, "ssm-image-absent-template").await;

    // While the parameter is there the group launches from the AMI it holds.
    ask_for(&router, case, group, "1").await;
    let outcome = settled_activity(&router, case, group).await;
    assert_eq!(
        outcome.status_code, "Successful",
        "case {case:?}: the first scale-out must succeed, got {outcome:?}"
    );
    let first = instances_once_held(&router, case, group, 1).await;
    assert_eq!(
        instance_image_id(&router, case, &first[0]).await,
        image_id,
        "case {case:?}: the first instance must run the AMI the parameter held"
    );

    // The client deletes the parameter. Nothing about the template or the group changes, and
    // the next scale-out has no AMI to launch.
    ssm(
        &router,
        case,
        "DeleteParameter",
        json!({ "Name": parameter }),
    )
    .await;
    ask_for(&router, case, group, "2").await;
    let refused = settled_activity(&router, case, group).await;
    assert_eq!(
        (refused.status_code.as_str(), refused.status_message.as_str()),
        ("Failed", invalid_alias_message(&parameter).as_str()),
        "case {case:?}: the scale-out must fail while the parameter is absent, got {refused:?}"
    );
    assert_eq!(
        group_instances(&router, case, group).await,
        first,
        "case {case:?}: a refused scale-out places no further instance, got the group's own \
         report"
    );
    assert_eq!(
        account_instances(&router, case).await.len(),
        first.len(),
        "case {case:?}: a launch Amazon EC2 would refuse mints no instance of the account either"
    );

    // The client's own remediation: the parameter is restored. Nothing asks for the capacity
    // again — it was never withdrawn.
    put_image_parameter(&router, case, &parameter, &image_id).await;
    let held = instances_once_held(&router, case, group, 2).await;
    assert_eq!(
        held.len(),
        2,
        "case {case:?}: the group must place the capacity it was owed once the parameter is \
         back, got {held:?}"
    );
    for instance_id in &held {
        assert_eq!(
            instance_image_id(&router, case, instance_id).await,
            image_id,
            "case {case:?}: every instance of the group runs the AMI the parameter holds"
        );
    }
    let settled = settled_activity(&router, case, group).await;
    assert_eq!(
        settled.status_code, "Successful",
        "case {case:?}: the launch made after the repair must succeed, got {settled:?}"
    );
}
