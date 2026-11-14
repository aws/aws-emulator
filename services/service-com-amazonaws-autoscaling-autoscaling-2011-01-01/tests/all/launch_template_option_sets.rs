// SPDX-License-Identifier: CC0-1.0

//! The OPTION SETS an instance an Auto Scaling group launched reports, which are the ones
//! the launch-template version it was launched from declares.
//!
//! RULE: "the launch template contains the parameters to launch an instance"
//! (`LaunchTemplateVersion$LaunchTemplateData`), so an instance a group's capacity change
//! mints from a version carries that version's own launch data — the instance metadata
//! options it declares, its enclave options, its hibernation setting — exactly as a launch
//! that names the template does. A member the version leaves out falls to the account's
//! own default and then to the member's documented one; nothing the version declares is
//! dropped.
//!
//! The rows span what the rule generalises over: which metadata members the version
//! declares (one, several, all five), the values they carry (both words of each enum, the
//! published hop-limit minimum, maximum and a value between), where the group names the
//! template (its own `LaunchTemplate` or the launch template of a `MixedInstancesPolicy`),
//! and option sets outside the metadata ones (enclave, hibernation). The last test pins
//! the version that declares NOTHING against what `RunInstances` reports for the same
//! version, the defaults being EC2's to decide rather than this suite's.
//!
//! Both services are mounted on ONE engine and driven over their own wire protocols —
//! awsQuery for Auto Scaling, ec2Query for Amazon EC2 — the way the emulator serves them,
//! so the instances these assertions read are the ones a client is given.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{borrowed, ec2_query, p, query, tag_value, tag_values, REGION};

/// The image every launch template in this suite carries, in the long id form Amazon EC2
/// issues one in.
const IMAGE_ID: &str = "ami-00000000000000041";

/// The instance type every launch template in this suite carries.
const INSTANCE_TYPE: &str = "t3.micro";

/// One emulator with both services on one engine: the instances an Auto Scaling group
/// launches are the same account's Amazon EC2 instances.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// The zone a group with no subnets places its instances in: the first of the Region's own
/// catalog, never written out here.
fn zone() -> String {
    ec2::availability_zones_in_region(REGION)
        .first()
        .map(|zone| zone.zone_name.clone())
        .expect("the Region has Availability Zones")
}

/// Create one launch template whose data carries `declared`, as
/// `(LaunchTemplateData member path, value)`, and answer its name.
async fn create_launch_template(
    router: &MultiServiceRouter,
    name: &str,
    declared: &[(&str, &str)],
) -> String {
    let mut params = vec![
        p("LaunchTemplateName", name),
        p("LaunchTemplateData.ImageId", IMAGE_ID),
        p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
    ];
    for (member, value) in declared {
        params.push(p(&format!("LaunchTemplateData.{member}"), *value));
    }
    let (status, body) = ec2_query(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
    name.to_string()
}

/// Where the group names the launch template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The group's own `LaunchTemplate`.
    Own,
    /// The launch template of its `MixedInstancesPolicy`.
    MixedPolicy,
}

impl Source {
    /// The parameter the template's name is carried under.
    fn member(self) -> &'static str {
        match self {
            Self::Own => "LaunchTemplate.LaunchTemplateName",
            Self::MixedPolicy => "MixedInstancesPolicy.LaunchTemplate.\
                                  LaunchTemplateSpecification.LaunchTemplateName",
        }
    }
}

/// Create a group of capacity one from `template` and answer the id of the instance it
/// launched.
async fn launched_instance(
    router: &MultiServiceRouter,
    group: &str,
    template: &str,
    source: Source,
) -> String {
    let params = [
        p("AutoScalingGroupName", group),
        p(source.member(), template),
        p("MinSize", "1"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "1"),
        p("AvailabilityZones.member.1", zone()),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {group} expected 200, got {status}: {body}"
    );
    let (status, described) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups expected 200, got {status}: {described}"
    );
    let instances = tag_values(&described, "InstanceId");
    assert_eq!(
        instances.len(),
        1,
        "the group must have launched its one instance, got {described}"
    );
    instances[0].clone()
}

/// The `metadataOptions` element Amazon EC2 reports for ONE instance.
async fn reported_metadata_options(router: &MultiServiceRouter, instance_id: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances for {instance_id} expected 200, got {status}: {body}"
    );
    let options = tag_value(&body, "metadataOptions");
    assert!(
        !options.is_empty(),
        "Amazon EC2 must report the instance's metadata options, got {body}"
    );
    options
}

/// The element one member of the metadata options is reported under: the model's `xmlName`
/// for every member of `InstanceMetadataOptionsResponse` is that member's name with a
/// lowercased first letter (`HttpTokens` -> `httpTokens`), and
/// `LaunchTemplateInstanceMetadataOptionsRequest` carries the same member names, so the
/// element a declared member comes back under is derived rather than tabulated.
fn reported_member(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// One row: the metadata members the version declares, and where the group names it.
struct Row {
    case: &'static str,
    /// `(member of `MetadataOptions`, value)` the launch template version declares. Every
    /// one of them must come back on the instance.
    metadata: &'static [(&'static str, &'static str)],
    source: Source,
}

#[tokio::test]
async fn an_instance_reports_the_metadata_options_its_launch_template_declares() {
    // The values are the enums' own words and the published hop-limit bounds
    // ("possible values: integers from 1 to 64",
    // `smithy_query(query="doc",shape="LaunchTemplateInstanceMetadataOptionsRequest",
    // member="HttpPutResponseHopLimit")`).
    let rows = [
        Row {
            case: "IMDSv2 required with a hop limit of two",
            metadata: &[("HttpTokens", "required"), ("HttpPutResponseHopLimit", "2")],
            source: Source::Own,
        },
        Row {
            case: "every member of the set, at the published maximum hop limit",
            metadata: &[
                ("HttpTokens", "optional"),
                ("HttpPutResponseHopLimit", "64"),
                ("HttpEndpoint", "disabled"),
                ("HttpProtocolIpv6", "enabled"),
                ("InstanceMetadataTags", "enabled"),
            ],
            source: Source::Own,
        },
        Row {
            case: "the published minimum hop limit, through a mixed instances policy",
            metadata: &[
                ("HttpPutResponseHopLimit", "1"),
                ("InstanceMetadataTags", "enabled"),
                ("HttpTokens", "required"),
            ],
            source: Source::MixedPolicy,
        },
        Row {
            case: "one member alone: the instance tags the metadata service serves",
            metadata: &[("InstanceMetadataTags", "enabled")],
            source: Source::MixedPolicy,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let declared: Vec<(String, String)> = row
            .metadata
            .iter()
            .map(|(member, value)| {
                (format!("MetadataOptions.{member}"), (*value).to_string())
            })
            .collect();
        let template = create_launch_template(
            &router,
            &format!("lt-options-{index}"),
            &borrowed(&declared),
        )
        .await;
        let instance_id =
            launched_instance(&router, &format!("options-{index}"), &template, row.source).await;
        let options = reported_metadata_options(&router, &instance_id).await;

        for (member, value) in row.metadata {
            let element = reported_member(member);            assert_eq!(
                tag_value(&options, &element),
                *value,
                "case {case:?}: the instance must report the {member} the version declares \
                 ({value}), got {options}"
            );
        }
    }
}

/// One row of the option sets outside the metadata ones: the member the version declares,
/// the element it is reported under, and the value the instance must report.
struct OptionSetRow {
    case: &'static str,
    declared: (&'static str, &'static str),
    reported: (&'static str, &'static str),
    expected: &'static str,
}

#[tokio::test]
async fn an_instance_reports_the_other_option_sets_its_launch_template_declares() {
    let rows = [
        OptionSetRow {
            case: "an enclave-enabled version",
            declared: ("EnclaveOptions.Enabled", "true"),
            reported: ("enclaveOptions", "enabled"),
            expected: "true",
        },
        OptionSetRow {
            case: "a version configured for hibernation",
            declared: ("HibernationOptions.Configured", "true"),
            reported: ("hibernationOptions", "configured"),
            expected: "true",
        },
        OptionSetRow {
            case: "a version that configures neither",
            declared: ("EbsOptimized", "true"),
            reported: ("enclaveOptions", "enabled"),
            expected: "false",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let template = create_launch_template(
            &router,
            &format!("lt-sets-{index}"),
            &[(row.declared.0, row.declared.1)],
        )
        .await;
        let instance_id =
            launched_instance(&router, &format!("sets-{index}"), &template, Source::Own).await;
        let (status, body) = ec2_query(
            &router,
            "DescribeInstances",
            &[p("InstanceId.1", instance_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInstances expected 200, got {status}: {body}"
        );
        let (outer, inner) = row.reported;
        assert_eq!(
            tag_value(&tag_value(&body, outer), inner),
            row.expected,
            "case {case:?}: expected {outer}/{inner} to be {}, got {body}",
            row.expected
        );
    }
}

/// A version that declares NO option set leaves them at the defaults Amazon EC2 itself
/// decides — which depend on the AMI's own IMDS requirement — so an instance the group
/// launched reports exactly what `RunInstances` reports for a launch from that same
/// version. Neither set of values is written out here.
#[tokio::test]
async fn a_version_that_declares_nothing_matches_what_run_instances_reports() {
    for (index, source) in [Source::Own, Source::MixedPolicy].into_iter().enumerate() {
        let case = format!("a group that names the version through {source:?}");
        let router = emulator();
        let template = create_launch_template(&router, &format!("lt-plain-{index}"), &[]).await;
        let instance_id =
            launched_instance(&router, &format!("plain-{index}"), &template, source).await;
        let by_the_group = reported_metadata_options(&router, &instance_id).await;

        let (status, body) = ec2_query(
            &router,
            "RunInstances",
            &[
                p("LaunchTemplate.LaunchTemplateName", template.as_str()),
                p("MinCount", "1"),
                p("MaxCount", "1"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: RunInstances expected 200, got {status}: {body}"
        );
        let by_run_instances = tag_value(&body, "metadataOptions");
        assert!(
            !by_run_instances.is_empty(),
            "case {case:?}: RunInstances must report the launch's metadata options, got {body}"
        );
        // The state is the only member that moves on the clock, so it is read off neither:
        // the options a launch CHOSE are what the two must agree on.
        let without_state = |options: &str| -> Vec<String> {
            ["httpTokens", "httpPutResponseHopLimit", "httpEndpoint", "httpProtocolIpv6", "instanceMetadataTags"]
                .iter()
                .map(|member| format!("{member}={}", tag_value(options, member)))
                .collect()
        };
        assert_eq!(
            without_state(&by_the_group),
            without_state(&by_run_instances),
            "case {case:?}: the group's instance must carry the same options a launch from \
             that version does, got {by_the_group} against {by_run_instances}"
        );
    }
}
