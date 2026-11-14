// SPDX-License-Identifier: CC0-1.0

//! The EC2 instance a WorkSpace Instance IS exists in Amazon EC2.
//!
//! RULE under test: a WorkSpace Instance is a managed EC2 instance launched in the caller's
//! own account, so `CreateWorkspaceInstance` launches it through the service that OWNS
//! instances with the configuration `ManagedInstance` carries, reports the `InstanceId` EC2
//! minted as `EC2ManagedInstance.InstanceId`, surfaces a launch EC2 refuses through the
//! modelled `EC2InstanceError` beside the terminal `ERROR_ALLOCATING`, and shuts that
//! instance down when `DeleteWorkspaceInstance` tears the WorkSpace Instance down.
//!
//! Nothing here reaches into either store: the instance is believed to exist only because
//! `ec2:DescribeInstances` reported it, and its configuration only because EC2 reported
//! that too. EC2 is driven through the AWS SDK over the router's own in-process callback —
//! the same seam the create's launch reaches it through.
//!
//! The rows span what the rule generalises over: the image, the instance type, how the
//! network is named (nothing, a subnet of the account, a subnet with security groups, a
//! placement zone), the storage and user data a launch carries, and where the instance's
//! tags come from (the top-level `Tags`, a launch `TagSpecification`, or both).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::workspace_instance_lifecycle::{
    call, create, harness, list_all_ids, ok, stored_tags, ACCOUNT_SECURITY_GROUPS,
    ACCOUNT_SUBNETS, ACCOUNT_ZONE, DEFAULT_TEST_REGION,
};

/// Two images a launch may boot, both inside the modelled
/// `^ami-[0-9a-zA-Z]{1,63}$` (`smithy_query(query="shape",shape="ImageId")`): the rows
/// differ in the image as well as in everything else, so nothing can pass by reporting one
/// fixed value.
const IMAGE_A: &str = "ami-0123456789abcdef0";
const IMAGE_B: &str = "ami-0fedcba9876543210";

/// A subnet and a security group the account does not hold — both inside the id forms EC2
/// issues, so only their absence from the account can be what the launch is refused for.
const UNHELD_SUBNET: &str = "subnet-0f00000000000000f";
const UNHELD_SECURITY_GROUP: &str = "sg-0f00000000000000f";

/// Every instance id Amazon EC2 holds for the account.
async fn held_instances(ec2: &::aws_sdk_ec2::Client) -> Vec<String> {
    let answered = ec2
        .describe_instances()
        .send()
        .await
        .expect("ec2:DescribeInstances answers");
    answered
        .reservations()
        .iter()
        .flat_map(::aws_sdk_ec2::types::Reservation::instances)
        .filter_map(|instance| instance.instance_id().map(str::to_string))
        .collect()
}

/// How long Amazon EC2 is given to bring an instance to a state its own clock owes it — a
/// launch to `running`, a termination to `terminated`. Nothing in the emulator sleeps; this
/// is the test looking again until the condition holds, with generous room so a loaded host
/// cannot fail it.
const EC2_BUDGET: Duration = Duration::from_secs(30);

/// How long this suite waits between looks while polling for a condition.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `InstanceState$Name` values this suite names.
const STATE_RUNNING: &str = "running";
const STATE_SHUTTING_DOWN: &str = "shutting-down";
const STATE_TERMINATED: &str = "terminated";

/// An EC2 client over the router's own in-process callback: no socket is opened and
/// nothing leaves the process, and the endpoint below is only the address that transport
/// routes on.
///
/// `pub(crate)` because the sibling `launch_image_resolution` suite registers an AMI
/// through the same seam rather than building a second client.
pub(crate) fn ec2_client(router: &MultiServiceRouter) -> ::aws_sdk_ec2::Client {
    let callback = EmbeddedEmulator::from_router(router.clone());
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(
            DEFAULT_TEST_REGION.to_string(),
        ))
        .endpoint_url(format!("https://ec2.{DEFAULT_TEST_REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// The instance Amazon EC2 holds for `instance_id`, or `None` when it holds none.
async fn described(
    ec2: &::aws_sdk_ec2::Client,
    instance_id: &str,
) -> Option<::aws_sdk_ec2::types::Instance> {
    let answered = ec2
        .describe_instances()
        .instance_ids(instance_id)
        .send()
        .await
        .ok()?;
    answered
        .reservations()
        .iter()
        .flat_map(::aws_sdk_ec2::types::Reservation::instances)
        .find(|instance| instance.instance_id() == Some(instance_id))
        .cloned()
}

/// The `EC2ManagedInstance.InstanceId` `GetWorkspaceInstance` reports for `id`, and the
/// `ProvisionState` beside it.
async fn reported_managed_instance(
    router: &MultiServiceRouter,
    id: &str,
) -> (Option<String>, String, Value) {
    let got = ok(
        router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    let managed = got["EC2ManagedInstance"]["InstanceId"]
        .as_str()
        .map(str::to_string);
    let state = got["ProvisionState"].as_str().unwrap_or_default().to_string();
    (managed, state, got)
}

/// Look again until EC2 reports `instance_id` in one of `states`, and answer the state it
/// reported. Waits for the CONDITION, not for a span: the deadline is only the point past
/// which the condition is taken to be unreachable.
async fn settled_in(
    ec2: &::aws_sdk_ec2::Client,
    case: &str,
    instance_id: &str,
    states: &[&str],
) -> String {
    let deadline = Instant::now() + EC2_BUDGET;
    loop {
        let reported = described(ec2, instance_id)
            .await
            .and_then(|instance| {
                instance
                    .state()
                    .and_then(|state| state.name().map(|name| name.as_str().to_string()))
            })
            .unwrap_or_default();
        if states.contains(&reported.as_str()) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected EC2 to report {instance_id} in one of {states:?}, it \
             reports {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The tags EC2 reports on an instance, as a map.
fn instance_tags(instance: &::aws_sdk_ec2::types::Instance) -> BTreeMap<String, String> {
    instance
        .tags()
        .iter()
        .filter_map(|tag| {
            Some((
                tag.key()?.to_string(),
                tag.value().unwrap_or_default().to_string(),
            ))
        })
        .collect()
}

/// One launch the rule is asserted on.
struct Row {
    case: &'static str,
    /// The `ManagedInstance` the create carries.
    managed_instance: Value,
    /// The top-level `Tags` the create carries.
    tags: Value,
    /// The `ImageId` and `InstanceType` the launched instance must report.
    image_id: &'static str,
    instance_type: &'static str,
    /// The subnet the launched instance must sit in, when the request named one.
    subnet_id: Option<&'static str>,
    /// The Availability Zone the launched instance must be placed in.
    zone: &'static str,
    /// The `KeyName` the launched instance must report, when the request named one.
    key_name: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "only the members a launch cannot be made without",
            managed_instance: json!({ "ImageId": IMAGE_A, "InstanceType": "t3.medium" }),
            tags: json!([]),
            image_id: IMAGE_A,
            instance_type: "t3.medium",
            // The launch named no subnet, so EC2 chooses the account's default subnet of
            // the zone it placed the instance in — which is the Region's first zone.
            subnet_id: None,
            zone: ACCOUNT_ZONE,
            key_name: None,
        },
        Row {
            case: "another image, another instance type, and one tag",
            managed_instance: json!({ "ImageId": IMAGE_B, "InstanceType": "m5.large" }),
            tags: json!([{ "Key": "env", "Value": "dev" }]),
            image_id: IMAGE_B,
            instance_type: "m5.large",
            subnet_id: None,
            zone: ACCOUNT_ZONE,
            key_name: None,
        },
        Row {
            case: "placed in a subnet of the account",
            managed_instance: json!({
                "ImageId": IMAGE_A,
                "InstanceType": "c5.large",
                "SubnetId": ACCOUNT_SUBNETS[0],
            }),
            tags: json!([{ "Key": "owner", "Value": "alice" }]),
            image_id: IMAGE_A,
            instance_type: "c5.large",
            subnet_id: Some(ACCOUNT_SUBNETS[0]),
            zone: ACCOUNT_ZONE,
            key_name: None,
        },
        Row {
            case: "a subnet, two security groups, a key pair and user data",
            managed_instance: json!({
                "ImageId": IMAGE_B,
                "InstanceType": "t3.large",
                "SubnetId": ACCOUNT_SUBNETS[1],
                "SecurityGroupIds": [ACCOUNT_SECURITY_GROUPS[0], ACCOUNT_SECURITY_GROUPS[1]],
                "KeyName": "my-key",
                "UserData": "echo hello",
            }),
            tags: json!([]),
            image_id: IMAGE_B,
            instance_type: "t3.large",
            subnet_id: Some(ACCOUNT_SUBNETS[1]),
            zone: ACCOUNT_ZONE,
            key_name: Some("my-key"),
        },
        Row {
            case: "placed by Placement.AvailabilityZone, with block devices",
            managed_instance: json!({
                "ImageId": IMAGE_A,
                "InstanceType": "m5.xlarge",
                "Placement": { "AvailabilityZone": ACCOUNT_ZONE, "Tenancy": "default" },
                "BlockDeviceMappings": [
                    { "DeviceName": "/dev/sda1", "Ebs": { "VolumeType": "gp3", "VolumeSize": 100 } }
                ],
            }),
            tags: json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "team", "Value": "eu" },
            ]),
            image_id: IMAGE_A,
            instance_type: "m5.xlarge",
            subnet_id: None,
            zone: ACCOUNT_ZONE,
            key_name: None,
        },
        Row {
            case: "tags from a launch TagSpecification as well as from Tags",
            managed_instance: json!({
                "ImageId": IMAGE_B,
                "InstanceType": "t3.medium",
                "SubnetId": ACCOUNT_SUBNETS[2],
                "TagSpecifications": [
                    { "ResourceType": "instance", "Tags": [{ "Key": "from-spec", "Value": "1" }] },
                    { "ResourceType": "volume", "Tags": [{ "Key": "disk", "Value": "root" }] },
                ],
            }),
            tags: json!([{ "Key": "from-tags", "Value": "2" }]),
            image_id: IMAGE_B,
            instance_type: "t3.medium",
            subnet_id: Some(ACCOUNT_SUBNETS[2]),
            zone: ACCOUNT_ZONE,
            key_name: None,
        },
    ]
}

#[tokio::test]
async fn every_created_workspace_instance_resolves_a_launched_ec2_instance() {
    let (router, _engine) = harness();
    let ec2 = ec2_client(&router);

    let mut reported_ids: Vec<String> = Vec::new();
    for row in rows() {
        let case = row.case;
        let id = create(
            &router,
            json!({ "ManagedInstance": row.managed_instance, "Tags": row.tags }),
        )
        .await;

        // The id this service reports for the managed instance.
        let (managed, _state, got) = reported_managed_instance(&router, &id).await;
        let managed = managed.unwrap_or_else(|| {
            panic!("case {case:?}: expected an EC2ManagedInstance.InstanceId, got {got}")
        });
        assert!(
            managed.starts_with("i-"),
            "case {case:?}: expected an EC2 instance id, got {managed:?}"
        );
        assert!(
            !reported_ids.contains(&managed),
            "case {case:?}: expected an instance of its own, got the already-reported \
             {managed:?}"
        );

        // And the service that OWNS instances holds it: this is the whole point of
        // reporting the id.
        let instance = described(&ec2, &managed).await.unwrap_or_else(|| {
            panic!("case {case:?}: ec2:DescribeInstances resolves no instance {managed:?}")
        });
        assert_eq!(
            instance.image_id(),
            Some(row.image_id),
            "case {case:?}: expected the requested ImageId, got {instance:?}"
        );
        assert_eq!(
            instance
                .instance_type()
                .map(|instance_type| instance_type.as_str()),
            Some(row.instance_type),
            "case {case:?}: expected the requested InstanceType, got {instance:?}"
        );
        assert_eq!(
            instance
                .placement()
                .and_then(::aws_sdk_ec2::types::Placement::availability_zone),
            Some(row.zone),
            "case {case:?}: expected the instance in {:?}, got {instance:?}",
            row.zone
        );
        match row.subnet_id {
            Some(subnet_id) => assert_eq!(
                instance.subnet_id(),
                Some(subnet_id),
                "case {case:?}: expected the instance in the subnet the request named, got \
                 {instance:?}"
            ),
            // The launch named no subnet, so EC2 placed it in the account's default subnet
            // of the zone: a subnet it really holds, whichever id that is.
            None => assert!(
                instance.subnet_id().is_some(),
                "case {case:?}: expected the instance in a subnet of the account, got \
                 {instance:?}"
            ),
        }
        if let Some(key_name) = row.key_name {
            assert_eq!(
                instance.key_name(),
                Some(key_name),
                "case {case:?}: expected the requested KeyName, got {instance:?}"
            );
        }

        // The tags the WorkSpace Instance carries are the launched instance's own.
        let expected_tags = stored_tags(&router, &id).await;
        let carried = instance_tags(&instance);
        for (key, value) in &expected_tags {
            assert_eq!(
                carried.get(key),
                Some(value),
                "case {case:?}: expected the launched instance to carry {key}={value}, got \
                 {carried:?}"
            );
        }

        // The id is the record's, not the answer's: a later read and the listing report the
        // same one.
        let (again, _state, got) = reported_managed_instance(&router, &id).await;
        assert_eq!(
            again.as_deref(),
            Some(managed.as_str()),
            "case {case:?}: expected the same managed instance on a second read, got {got}"
        );
        let listed = ok(&router, "ListWorkspaceInstances", json!({})).await;
        let in_listing = listed["WorkspaceInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstances in {listed}"))
            .iter()
            .find(|entry| entry["WorkspaceInstanceId"].as_str() == Some(id.as_str()))
            .and_then(|entry| entry["EC2ManagedInstance"]["InstanceId"].as_str())
            .map(str::to_string);
        assert_eq!(
            in_listing.as_deref(),
            Some(managed.as_str()),
            "case {case:?}: expected the listing to report the same managed instance, got \
             {listed}"
        );

        // EC2's own clock brings the launched instance up, which is what makes it a
        // running instance of the account rather than a record.
        settled_in(&ec2, case, &managed, &[STATE_RUNNING]).await;

        reported_ids.push(managed);
    }

    // Every WorkSpace Instance created above is still reported, so none of the rows
    // displaced another.
    let ids = list_all_ids(&router, json!({})).await;
    assert_eq!(
        ids.len(),
        reported_ids.len(),
        "case the whole table: expected one WorkSpace Instance per row, got {ids:?}"
    );
}

#[tokio::test]
async fn deleting_a_workspace_instance_shuts_the_managed_instance_down() {
    let (router, _engine) = harness();
    let ec2 = ec2_client(&router);

    // Rows differing in how the launch was configured, so the teardown is shown for an
    // instance EC2 placed itself and for ones the request placed.
    let configurations = [
        (
            "a launch that named no network",
            json!({ "ImageId": IMAGE_A, "InstanceType": "t3.medium" }),
        ),
        (
            "a launch placed in a subnet of the account",
            json!({
                "ImageId": IMAGE_B,
                "InstanceType": "m5.large",
                "SubnetId": ACCOUNT_SUBNETS[0],
            }),
        ),
        (
            "a launch placed by its Availability Zone",
            json!({
                "ImageId": IMAGE_A,
                "InstanceType": "c5.large",
                "Placement": { "AvailabilityZone": ACCOUNT_ZONE },
            }),
        ),
    ];

    for (case, managed_instance) in configurations {
        let id = create(&router, json!({ "ManagedInstance": managed_instance })).await;
        let (managed, _state, got) = reported_managed_instance(&router, &id).await;
        let managed = managed.unwrap_or_else(|| {
            panic!("case {case:?}: expected an EC2ManagedInstance.InstanceId, got {got}")
        });
        settled_in(&ec2, case, &managed, &[STATE_RUNNING]).await;

        ok(
            &router,
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &id }),
        )
        .await;

        // The WorkSpace Instance is tearing down, and so is the instance behind it: the
        // delete reached the service that owns it.
        let reported = settled_in(
            &ec2,
            case,
            &managed,
            &[STATE_SHUTTING_DOWN, STATE_TERMINATED],
        )
        .await;
        assert!(
            [STATE_SHUTTING_DOWN, STATE_TERMINATED].contains(&reported.as_str()),
            "case {case:?}: expected the managed instance to be shut down, EC2 reports \
             {reported:?}"
        );
        // And EC2's own clock takes it to the terminal state.
        settled_in(&ec2, case, &managed, &[STATE_TERMINATED]).await;

        let (_managed, state, got) = reported_managed_instance(&router, &id).await;
        assert!(
            [
                wsinst::state::PROVISION_STATE_DEALLOCATING,
                wsinst::state::PROVISION_STATE_DEALLOCATED,
            ]
            .contains(&state.as_str()),
            "case {case:?}: expected the WorkSpace Instance to be tearing down, got {got}"
        );
    }
}

#[tokio::test]
async fn a_launch_ec2_refuses_is_reported_as_an_ec2_instance_error() {
    let (router, _engine) = harness();
    let ec2 = ec2_client(&router);

    // Each row names an EC2 resource the account does not hold, so EC2 refuses the launch
    // with its own error code — the code the WorkSpace Instance must report back.
    let rows = [
        (
            "a network interface in a subnet the account does not hold",
            json!({
                "ImageId": IMAGE_A,
                "InstanceType": "t3.medium",
                "NetworkInterfaces": [{ "DeviceIndex": 0, "SubnetId": UNHELD_SUBNET }],
            }),
            "InvalidSubnetID.NotFound",
        ),
        (
            "a security group the account does not hold",
            json!({
                "ImageId": IMAGE_B,
                "InstanceType": "m5.large",
                "SubnetId": ACCOUNT_SUBNETS[0],
                "SecurityGroupIds": [UNHELD_SECURITY_GROUP],
            }),
            "InvalidGroup.NotFound",
        ),
    ];

    for (case, managed_instance, expected_code) in rows {
        // The create itself is accepted: the launch is asynchronous work, so its refusal is
        // reported on the instance rather than as a failure of this call.
        let (status, payload) = call(
            &router,
            "CreateWorkspaceInstance",
            json!({ "ManagedInstance": managed_instance }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to be accepted, got {status} with {payload}"
        );
        let id = payload["WorkspaceInstanceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstanceId in {payload}"))
            .to_string();

        let (managed, state, got) = reported_managed_instance(&router, &id).await;
        assert_eq!(
            state,
            wsinst::state::PROVISION_STATE_ERROR_ALLOCATING,
            "case {case:?}: expected the failed allocation to be reported, got {got}"
        );
        assert_eq!(
            managed, None,
            "case {case:?}: a launch that was refused has no managed instance to report, \
             got {got}"
        );
        let errors = got["EC2InstanceErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no EC2InstanceErrors in {got}"));
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected the one refusal EC2 answered, got {got}"
        );
        assert_eq!(
            errors[0]["EC2ErrorCode"].as_str(),
            Some(expected_code),
            "case {case:?}: expected EC2's own error code, got {got}"
        );
        assert_eq!(
            errors[0]["EC2ExceptionType"].as_str(),
            Some("ServiceError"),
            "case {case:?}: expected the class of failure EC2 answered with, got {got}"
        );
        assert!(
            errors[0]["EC2ErrorMessage"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected EC2's own sentence, got {got}"
        );

        // Nothing was launched: the listing reports the WorkSpace Instance with no managed
        // instance, and the delete of it terminates nothing.
        let listed = ok(&router, "ListWorkspaceInstances", json!({})).await;
        let entry = listed["WorkspaceInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstances in {listed}"))
            .iter()
            .find(|entry| entry["WorkspaceInstanceId"].as_str() == Some(id.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: {id} is not in the listing: {listed}"));
        assert!(
            entry["EC2ManagedInstance"].is_null(),
            "case {case:?}: the listing must report no managed instance, got {entry}"
        );

        ok(
            &router,
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &id }),
        )
        .await;
        assert!(
            held_instances(&ec2).await.is_empty(),
            "case {case:?}: a refused launch leaves the account holding no instance"
        );
    }
}

#[tokio::test]
async fn a_spent_idempotency_token_launches_no_second_instance() {
    let (router, _engine) = harness();
    let ec2 = ec2_client(&router);
    let token = "tok-one-launch-per-workspace-instance";
    let body = json!({
        "ClientToken": token,
        "ManagedInstance": { "ImageId": IMAGE_A, "InstanceType": "t3.medium" },
    });

    let first = create(&router, body.clone()).await;
    let (managed, _state, got) = reported_managed_instance(&router, &first).await;
    let managed = managed
        .unwrap_or_else(|| panic!("case the first create: no managed instance in {got}"));

    // Three re-submissions of the same request: each is refused for the spent token, and
    // the one WorkSpace Instance keeps the one instance behind it.
    for repeat in 1..=3 {
        let (status, payload) = call(&router, "CreateWorkspaceInstance", body.clone()).await;
        assert_eq!(
            status, 409,
            "case reuse {repeat}: expected HTTP 409 for the spent token, got {status} with {payload}"
        );
        assert_eq!(
            payload["ResourceId"].as_str(),
            Some(first.as_str()),
            "case reuse {repeat}: expected the conflict to name {first:?}, got {payload}"
        );
        let (again, _state, got) = reported_managed_instance(&router, &first).await;
        assert_eq!(
            again.as_deref(),
            Some(managed.as_str()),
            "case reuse {repeat}: expected the same managed instance, got {got}"
        );
    }

    // And EC2 holds exactly one instance for the account: a refused reuse launched none.
    let held = held_instances(&ec2).await;
    assert_eq!(
        held,
        vec![managed.clone()],
        "case the whole sequence: expected exactly the one launched instance, got {held:?}"
    );
}
