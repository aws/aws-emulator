// SPDX-License-Identifier: CC0-1.0

//! A launch source has to NAME the launch template it launches from — driven end to end over
//! the wire this service speaks (`awsQuery`), with Amazon EC2 on the same engine so the
//! templates under test are the account's own.
//!
//! RULE: every launch-template specification a request configuring a group carries states one
//! half of the template's identity ("Conditional: You must specify either a
//! `LaunchTemplateId` or a `LaunchTemplateName`", `LaunchTemplateSpecification`), and a
//! `MixedInstancesPolicy` carries such a specification ("one or more launch templates and the
//! instance types (overrides) that are used to launch EC2 instances",
//! `MixedInstancesPolicy$LaunchTemplate`) — a request that leaves that pair of members out
//! names no template to launch from and is refused `ValidationError`, leaving the account
//! exactly as it was. Which SENTENCE it is refused with says which MEMBER the caller left out:
//! a policy carrying no `LaunchTemplate` at all is told to add one to the policy, while a
//! launch template that names no template — a specification carrying neither half, or a
//! policy's launch template naming one in neither of the places it may — is told which pair of
//! members to state
//! (`Valid requests must contain either launchTemplateId or LaunchTemplateName`).
//!
//! The rows differ along what the rule generalises over: WHERE the specification sits (the
//! group's own `LaunchTemplate`, the launch template of a `MixedInstancesPolicy`, and the one
//! an override of that policy names), HOW the launch source fails to name a template (a
//! specification carrying nothing but its `Version`, a policy whose launch template carries
//! only overrides, a policy carrying nothing but its instances distribution), and WHICH
//! operation takes the request (`CreateAutoScalingGroup`, `UpdateAutoScalingGroup` and the
//! desired configuration of `StartInstanceRefresh`). Served rows stating each half of the
//! identity sit in the same tables, so a rule that refused too much could not pass either.
//!
//! The suite then pins what a READ of such a group reports, because the identity a request
//! states is the identity a client reads back: the described group carries BOTH halves of the
//! template and the `Version` its stored reference names — not the template's first version
//! and not its newest, which the rows span by giving the template four versions — and a group
//! created with a `MixedInstancesPolicy` is described with one, its specification carrying
//! both halves, its overrides one member per entry in request order, and its instances
//! distribution carrying the defaults the group spreads capacity under.
//!
//! A launch source must also be one an instance could be launched FROM, so the last table
//! spans a launch-template version that asks for a public address on a launch of several
//! network interfaces.
//!
//! The neighbouring suites cover the same reads from their own angle:
//! `tests/all/launch_template_specifications.rs` (which half the request states),
//! `tests/all/launch_template_reference_version_selector.rs` (the selector's form) and
//! `tests/all/mixed_instances_policy_defaults.rs` (every distribution member's default).

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    borrowed, create_launch_template, default_service_linked_role_arn, ec2_query, element_body,
    emulator, list_members, p, query, tag_value, xml_unescape, REGION,
};

/// The launch template the groups of this suite launch from, and a second one an update moves
/// a group onto. Both are created over Amazon EC2's own wire.
const TEMPLATE: &str = "lt-names-its-template";
const OTHER_TEMPLATE: &str = "lt-names-its-template-next";

/// An instance type an override names. Any type is a valid `InstanceType` string
/// (`LaunchTemplateOverrides$InstanceType`, `XmlStringMaxLen255`); which types exist is not
/// what this rule is about.
const OVERRIDE_TYPE: &str = "t3.micro";

/// The sentence Amazon EC2 Auto Scaling refuses a launch source that names no launch template
/// with, spelled out here rather than imported so a wrong constant in the implementation
/// cannot make this suite pass.
const NAMES_NO_TEMPLATE: &str =
    "Valid requests must contain either launchTemplateId or LaunchTemplateName";

/// The sentence it refuses a `MixedInstancesPolicy` carrying no `LaunchTemplate` member at all
/// with — a different verdict, because the caller left out the policy's launch template rather
/// than the halves that name a template inside one. Spelled out here for the same reason.
const POLICY_NAMES_NO_TEMPLATE: &str =
    "A launch template is required when configuring an Auto Scaling group with a mixed \
     instances policy. Add a launch template to the mixed instances policy and try again.";

/// One row of a table whose request is built per case: the case, the launch-source members
/// the request carries, and the sentence it is refused with — empty where it is served.
type SourceRow = (&'static str, Vec<(String, String)>, &'static str);

/// One row: the case, the launch-source members the request carries beside the group's size
/// and placement, and the sentence the request is refused with — empty where it is served.
struct Row {
    case: &'static str,
    /// Built per row, because the served rows name the id Amazon EC2 minted.
    source: fn(&Minted) -> Vec<(String, String)>,
    refusal: &'static str,
}

/// What Amazon EC2 assigned the templates this suite creates: the values a row naming a
/// template by id takes from that service rather than writing out.
struct Minted {
    template_id: String,
    other_template_id: String,
}

/// The `<Message>` of an answered refusal, decoded the way a client reads it.
fn message(body: &str) -> String {
    xml_unescape(&tag_value(body, "Message"))
}

/// The name of the Region's first Availability Zone, from Amazon EC2's own catalog.
fn first_zone() -> String {
    let zones = ec2::availability_zones_in_region(REGION);
    zones
        .first()
        .expect("the Region must have Availability Zones for a group to be placed in")
        .zone_name
        .clone()
}

/// The parameters of a create that is valid apart from its launch source: a group of the
/// Region's first zone that holds no capacity.
fn base_create(name: &str) -> Vec<(String, String)> {
    vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("AvailabilityZones.member.1", first_zone()),
    ]
}

/// An emulator holding this suite's two launch templates, and the ids Amazon EC2 minted for
/// them.
async fn emulator_with_templates() -> (MultiServiceRouter, Minted) {
    let (router, _engine) = emulator();
    let (template_id, _) = create_launch_template(&router, TEMPLATE).await;
    let (other_template_id, _) = create_launch_template(&router, OTHER_TEMPLATE).await;
    (
        router,
        Minted {
            template_id,
            other_template_id,
        },
    )
}

/// Every way of stating a launch source this suite spans, with the two served rows that pin
/// the rule's other side.
const ROWS: &[Row] = &[
    Row {
        case: "the group's own specification states nothing but its version",
        source: |_| vec![p("LaunchTemplate.Version", "1")],
        refusal: NAMES_NO_TEMPLATE,
    },
    Row {
        case: "a mixed instances policy whose launch template carries only overrides",
        source: |_| {
            vec![p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                OVERRIDE_TYPE,
            )]
        },
        refusal: NAMES_NO_TEMPLATE,
    },
    Row {
        case: "a mixed instances policy carrying nothing but its instances distribution",
        source: |_| {
            vec![p(
                "MixedInstancesPolicy.InstancesDistribution.SpotAllocationStrategy",
                "lowest-price",
            )]
        },
        refusal: POLICY_NAMES_NO_TEMPLATE,
    },
    Row {
        case: "a mixed instances policy whose specification states nothing but its version",
        source: |_| {
            vec![
                p(
                    "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.Version",
                    "1",
                ),
                p(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                    OVERRIDE_TYPE,
                ),
            ]
        },
        refusal: NAMES_NO_TEMPLATE,
    },
    Row {
        case: "an override's own specification states nothing but its version",
        source: |_| {
            vec![
                p(
                    "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                     LaunchTemplateName",
                    TEMPLATE,
                ),
                p(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                    OVERRIDE_TYPE,
                ),
                p(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.\
                     LaunchTemplateSpecification.Version",
                    "1",
                ),
            ]
        },
        refusal: NAMES_NO_TEMPLATE,
    },
    Row {
        case: "the group's own specification names the template by name",
        source: |_| vec![p("LaunchTemplate.LaunchTemplateName", TEMPLATE)],
        refusal: "",
    },
    Row {
        case: "the group's own specification names the template by the id Amazon EC2 minted",
        source: |minted| {
            vec![
                p("LaunchTemplate.LaunchTemplateId", minted.template_id.clone()),
                p("LaunchTemplate.Version", "1"),
            ]
        },
        refusal: "",
    },
    Row {
        case: "a mixed instances policy whose specification names the template by id",
        source: |minted| {
            vec![
                p(
                    "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                     LaunchTemplateId",
                    minted.template_id.clone(),
                ),
                p(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                    OVERRIDE_TYPE,
                ),
            ]
        },
        refusal: "",
    },
];

/// RULE: a create whose launch source names no launch template is refused with AWS's sentence
/// and leaves no group; one that names it, by either half and wherever the specification
/// sits, is served and reports both halves back.
#[tokio::test]
async fn a_create_names_the_launch_template_it_launches_from() {
    for (index, row) in ROWS.iter().enumerate() {
        let (router, minted) = emulator_with_templates().await;
        let name = format!("asg-names-template-{index}");
        let mut params = base_create(&name);
        params.extend((row.source)(&minted));
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;

        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {:?}: expected the create to be served, got {status}: {body}",
                row.case
            );
            let described = describe(&router, &name).await;
            assert_eq!(
                tag_value(&described, "LaunchTemplateName"),
                TEMPLATE,
                "case {:?}: the group must report the template's name, got {described}",
                row.case
            );
            assert_eq!(
                tag_value(&described, "LaunchTemplateId"),
                minted.template_id,
                "case {:?}: the group must report the id Amazon EC2 minted, got {described}",
                row.case
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?}: expected the create to be refused, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {:?}: expected ValidationError, got {body}",
            row.case
        );
        assert_eq!(
            message(&body),
            row.refusal,
            "case {:?}: expected AWS's sentence for what the launch source left out",
            row.case
        );
        let described = describe(&router, &name).await;
        assert!(
            tag_value(&described, "AutoScalingGroupName").is_empty(),
            "case {:?}: the refused create must leave no group, got {described}",
            row.case
        );
    }
}

/// RULE: the same rule guards an UPDATE, and a refused update leaves the group launching from
/// the template it already had.
#[tokio::test]
async fn an_update_names_the_launch_template_it_repoints_the_group_at() {
    let (router, minted) = emulator_with_templates().await;
    let group = "asg-names-template-update";
    let mut params = base_create(group);
    params.push(p("LaunchTemplate.LaunchTemplateName", TEMPLATE));
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(status, 200, "the group must be created, got {status}: {body}");

    // (case, the launch-source members the update carries, the sentence it is refused with)
    let rows: Vec<SourceRow> = vec![
        (
            "a specification stating nothing but its version",
            vec![p("LaunchTemplate.Version", "1")],
            NAMES_NO_TEMPLATE,
        ),
        (
            "a mixed instances policy whose launch template carries only overrides",
            vec![p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                OVERRIDE_TYPE,
            )],
            NAMES_NO_TEMPLATE,
        ),
        (
            "a mixed instances policy carrying nothing but its instances distribution",
            vec![p(
                "MixedInstancesPolicy.InstancesDistribution.SpotAllocationStrategy",
                "lowest-price",
            )],
            POLICY_NAMES_NO_TEMPLATE,
        ),
        (
            "a specification naming the other template by id",
            vec![p(
                "LaunchTemplate.LaunchTemplateId",
                minted.other_template_id.clone(),
            )],
            "",
        ),
    ];

    for (case, source, refusal) in rows {
        let mut params = vec![p("AutoScalingGroupName", group)];
        params.extend(source);
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&params)).await;
        let described = describe(&router, group).await;
        if refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            );
            assert_eq!(
                tag_value(&described, "LaunchTemplateName"),
                OTHER_TEMPLATE,
                "case {case:?}: the group must report the template it was repointed at, got \
                 {described}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {body}"
        );
        assert_eq!(
            message(&body),
            refusal,
            "case {case:?}: expected AWS's sentence for what the launch source left out"
        );
        assert_eq!(
            tag_value(&described, "LaunchTemplateName"),
            TEMPLATE,
            "case {case:?}: the refused update must leave the group's launch source alone, got \
             {described}"
        );
    }
}

/// RULE: the desired configuration of an instance refresh carries a launch source, so the
/// same rule holds over it — a refresh naming no template is refused with the same sentence
/// and starts nothing.
#[tokio::test]
async fn a_refresh_names_the_launch_template_it_moves_instances_onto() {
    let (router, minted) = emulator_with_templates().await;
    let group = "asg-names-template-refresh";
    let mut params = base_create(group);
    params.push(p("LaunchTemplate.LaunchTemplateName", TEMPLATE));
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(status, 200, "the group must be created, got {status}: {body}");

    // (case, the desired configuration's launch-source members, the sentence it is refused
    // with)
    let rows: Vec<SourceRow> = vec![
        (
            "a desired configuration stating nothing but its version",
            vec![p("DesiredConfiguration.LaunchTemplate.Version", "1")],
            NAMES_NO_TEMPLATE,
        ),
        (
            "a desired configuration whose policy carries only overrides",
            vec![p(
                "DesiredConfiguration.MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.\
                 InstanceType",
                OVERRIDE_TYPE,
            )],
            NAMES_NO_TEMPLATE,
        ),
        (
            "a desired configuration whose policy carries nothing but its distribution",
            vec![p(
                "DesiredConfiguration.MixedInstancesPolicy.InstancesDistribution.\
                 SpotAllocationStrategy",
                "lowest-price",
            )],
            POLICY_NAMES_NO_TEMPLATE,
        ),
        (
            "a desired configuration naming the other template by id",
            vec![p(
                "DesiredConfiguration.LaunchTemplate.LaunchTemplateId",
                minted.other_template_id.clone(),
            )],
            "",
        ),
    ];

    for (case, source, refusal) in rows {
        let mut params = vec![p("AutoScalingGroupName", group)];
        params.extend(source);
        let (status, body) = query(&router, "StartInstanceRefresh", &borrowed(&params)).await;
        if refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the refresh to start, got {status}: {body}"
            );
            assert!(
                !tag_value(&body, "InstanceRefreshId").is_empty(),
                "case {case:?}: a started refresh must report its id, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refresh to be refused, got {status}: {body}"
        );
        assert_eq!(
            message(&body),
            refusal,
            "case {case:?}: expected AWS's sentence for what the launch source left out"
        );
    }
}

/// The group `name` as `DescribeAutoScalingGroups` reports it — the body itself, so a caller
/// reads whichever member it is about off it.
async fn describe(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups must answer 200, got {status}: {body}"
    );
    body
}

/// RULE: a described group reports the launch-template reference it was CONFIGURED with —
/// both halves of the template's identity and the `Version` as the request stated it — so the
/// version a read reports follows the group's stored reference and neither the template's
/// first version nor its newest.
#[tokio::test]
async fn a_described_group_reports_the_launch_template_version_it_references() {
    let (router, _engine) = emulator();
    let (template_id, first_version) = create_launch_template(&router, TEMPLATE).await;
    assert_eq!(
        first_version, "1",
        "a freshly created launch template holds its first version, got {first_version}"
    );
    // Two more versions, so a reference pinned to one of them names a different version from
    // both the template's first and its newest: a read that resolved the reference against
    // either end of the template could not pass every row.
    for expected in ["2", "3"] {
        let minted = add_template_version(&router, &template_id).await;
        assert_eq!(
            minted, expected,
            "the template's versions are minted in order, got {minted}"
        );
    }

    // (case, the group, the Version the create states, the Version a read must report)
    let rows: &[(&str, &str, Option<&str>, &str)] = &[
        (
            "pinned to the template's first version",
            "asg-version-first",
            Some("1"),
            "1",
        ),
        (
            "pinned to the newest version the template holds",
            "asg-version-newest",
            Some("3"),
            "3",
        ),
        (
            "pinned to a version in the middle",
            "asg-version-middle",
            Some("2"),
            "2",
        ),
        (
            "floating on the latest version",
            "asg-version-latest",
            Some("$Latest"),
            "$Latest",
        ),
        (
            "stating no version at all",
            "asg-version-unstated",
            None,
            "",
        ),
    ];

    for (case, group, stated, reported) in rows {
        let mut params = base_create(group);
        params.push(p("LaunchTemplate.LaunchTemplateId", template_id.clone()));
        if let Some(version) = stated {
            params.push(p("LaunchTemplate.Version", *version));
        }
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created, got {status}: {body}"
        );
        let described = describe(&router, group).await;
        let launch_template = element_body(&described, "LaunchTemplate")
            .unwrap_or_else(|| panic!("case {case:?}: the group must report a LaunchTemplate, got {described}"));
        assert_eq!(
            (
                tag_value(launch_template, "LaunchTemplateId"),
                tag_value(launch_template, "LaunchTemplateName"),
                tag_value(launch_template, "Version"),
            ),
            (
                template_id.clone(),
                TEMPLATE.to_string(),
                (*reported).to_string()
            ),
            "case {case:?}: expected the reference the group was configured with, got \
             {launch_template}"
        );
    }

    // A version the TEMPLATE gains afterwards changes no group's reference: each group keeps
    // reporting the version it points at.
    let minted = add_template_version(&router, &template_id).await;
    assert_eq!(
        minted, "4",
        "the fourth version must be minted next, got {minted}"
    );
    for (case, group, _, reported) in rows {
        let described = describe(&router, group).await;
        let launch_template = element_body(&described, "LaunchTemplate")
            .unwrap_or_else(|| panic!("case {case:?}: the group must report a LaunchTemplate"));
        assert_eq!(
            tag_value(launch_template, "Version"),
            *reported,
            "case {case:?}: a version the template gained must not move the group's \
             reference, got {launch_template}"
        );
    }

    // And an UPDATE that repoints a group reports the version it was repointed to.
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-version-first"),
            ("LaunchTemplate.LaunchTemplateName", TEMPLATE),
            ("LaunchTemplate.Version", "4"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the repointing update must be served, got {status}: {body}"
    );
    let described = describe(&router, "asg-version-first").await;
    let launch_template =
        element_body(&described, "LaunchTemplate").expect("the group reports a LaunchTemplate");
    assert_eq!(
        (
            tag_value(launch_template, "Version"),
            tag_value(launch_template, "LaunchTemplateName"),
            tag_value(launch_template, "LaunchTemplateId"),
        ),
        ("4".to_string(), TEMPLATE.to_string(), template_id.clone()),
        "a repointed group reports the version it was repointed to, beside both halves of \
         the template's identity, got {launch_template}"
    );
}

/// RULE: a group created with a `MixedInstancesPolicy` is described WITH one — the policy's
/// launch-template specification carrying both halves of the template's identity, one
/// override per entry in request order, and the instances distribution the group spreads its
/// capacity under.
#[tokio::test]
async fn a_described_group_reports_the_mixed_instances_policy_it_was_created_with() {
    let (router, _engine) = emulator();
    let (template_id, _) = create_launch_template(&router, TEMPLATE).await;
    // Two overrides, in the order the request writes them: the read reports one member each,
    // in that order.
    let types = ["t3.micro", "t3a.micro"];
    let group = "asg-mixed-round-trip";
    let mut params = base_create(group);
    params.push(p(
        "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateId",
        template_id.clone(),
    ));
    for (position, instance_type) in types.iter().enumerate() {
        params.push(p(
            &format!(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.{}.InstanceType",
                position + 1
            ),
            *instance_type,
        ));
    }
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(status, 200, "the group must be created, got {status}: {body}");

    let described = describe(&router, group).await;
    let policy = element_body(&described, "MixedInstancesPolicy")
        .unwrap_or_else(|| panic!("the group must report its MixedInstancesPolicy, got {described}"));
    let specification = element_body(policy, "LaunchTemplateSpecification")
        .unwrap_or_else(|| panic!("the policy must report its launch template, got {policy}"));
    assert_eq!(
        (
            tag_value(specification, "LaunchTemplateId"),
            tag_value(specification, "LaunchTemplateName"),
        ),
        (template_id.clone(), TEMPLATE.to_string()),
        "the policy's specification must carry both halves of the template's identity, got \
         {specification}"
    );
    let reported_types: Vec<String> = list_members(policy, "Overrides")
        .into_iter()
        .map(|member| tag_value(member, "InstanceType"))
        .collect();
    assert_eq!(
        reported_types,
        types.map(str::to_string).to_vec(),
        "the policy must report one override per entry, in request order, got {policy}"
    );
    // The distribution members the request left out, each the default its own documentation
    // states (`smithy_query(query="shape",shape="InstancesDistribution")`): the group spreads
    // its capacity one way whether or not the request said which.
    let distribution = element_body(policy, "InstancesDistribution")
        .unwrap_or_else(|| panic!("the policy must report its InstancesDistribution, got {policy}"));
    for (member, expected) in [
        ("OnDemandAllocationStrategy", "prioritized"),
        ("OnDemandBaseCapacity", "0"),
        ("OnDemandPercentageAboveBaseCapacity", "100"),
        ("SpotAllocationStrategy", "lowest-price"),
        ("SpotInstancePools", "2"),
    ] {
        assert_eq!(
            tag_value(distribution, member),
            expected,
            "member {member:?}: expected the documented default, got {distribution}"
        );
    }
}

/// RULE: a group launches only from a launch-template version an instance could be launched
/// from, so a request pointing it at a version that asks for a public address on a launch of
/// several network interfaces is refused with Amazon EC2's own sentence for that conflict,
/// and the group is left as it was.
#[tokio::test]
async fn a_group_is_configured_only_against_a_launchable_template_version() {
    // (case, how many network interfaces the version declares, whether the first one asks
    // for a public address, whether the create is served)
    let rows: &[(&str, usize, bool, bool)] = &[
        ("one interface asking for a public address", 1, true, true),
        ("two interfaces, neither asking for one", 2, false, true),
        ("two interfaces, the first asking for one", 2, true, false),
        ("three interfaces, the first asking for one", 3, true, false),
    ];
    // The two-part sentence: this service's preamble for a launch source it cannot use,
    // followed by Amazon EC2's own detail for the conflict. Spelled out here rather than
    // imported, so a wrong constant in the implementation cannot make this pass.
    let refusal = "You must use a valid fully-formed launch template. The \
                   associatePublicIPAddress parameter cannot be specified when launching with \
                   multiple network interfaces.";

    for (index, (case, interfaces, public_address, served)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let template = format!("lt-launchable-{index}");
        let mut data = vec![
            p("LaunchTemplateName", template.clone()),
            p("LaunchTemplateData.ImageId", "ami-0123456789abcdef0"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ];
        for device_index in 0..*interfaces {
            data.push(p(
                &format!("LaunchTemplateData.NetworkInterface.{}.DeviceIndex", device_index + 1),
                device_index.to_string(),
            ));
        }
        if *public_address {
            data.push(p(
                "LaunchTemplateData.NetworkInterface.1.AssociatePublicIpAddress",
                "true",
            ));
        }
        let (status, body) = ec2_query(&router, "CreateLaunchTemplate", &data).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateLaunchTemplate must answer 200, got {status}: {body}"
        );

        let group = format!("asg-launchable-{index}");
        let mut params = base_create(&group);
        params.push(p("LaunchTemplate.LaunchTemplateName", template.clone()));
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be served, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            message(&body),
            refusal,
            "case {case:?}: expected the service's sentence for a launch source it cannot use"
        );
        let described = describe(&router, &group).await;
        assert!(
            tag_value(&described, "AutoScalingGroupName").is_empty(),
            "case {case:?}: the refused create must leave no group, got {described}"
        );
    }
}

/// RULE: a described group also carries the members the SERVICE decides — the option sets
/// every group has whether or not the request stated them, and the collections it reports as
/// present-but-empty when it holds nothing in them — so a client reads the configuration in
/// force rather than a null it would have to know the default of.
#[tokio::test]
async fn a_described_group_carries_the_members_the_service_decides() {
    let (router, _engine) = emulator();
    let (template_id, _) = create_launch_template(&router, TEMPLATE).await;
    let group = "asg-service-decided-members";
    let mut params = base_create(group);
    params.push(p("LaunchTemplate.LaunchTemplateId", template_id));
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(status, 200, "the group must be created, got {status}: {body}");
    let described = describe(&router, group).await;

    // (the element the value sits in, the member, the value in force) — each default from
    // that member's own documentation: `CapacityReservationPreference` `default`,
    // `CapacityDistributionStrategy` "the default is balanced-best-effort", and
    // `TerminateHookAbandon` "set to terminate for default termination behavior".
    for (element, member, expected) in [
        (
            "CapacityReservationSpecification",
            "CapacityReservationPreference",
            "default",
        ),
        (
            "AvailabilityZoneDistribution",
            "CapacityDistributionStrategy",
            "balanced-best-effort",
        ),
        ("RetentionTriggers", "TerminateHookAbandon", "terminate"),
    ] {
        let body = element_body(&described, element).unwrap_or_else(|| {
            panic!("the group must report its {element}, got {described}")
        });
        assert_eq!(
            tag_value(body, member),
            expected,
            "member {member:?}: expected the value in force, got {body}"
        );
    }
    assert!(
        element_body(&described, "InstanceLifecyclePolicy").is_some(),
        "the retention triggers sit inside the group's InstanceLifecyclePolicy, got {described}"
    );
    // The role the group calls other services under, which every group has: the account's
    // default service-linked role for this service when the create named none.
    assert_eq!(
        tag_value(&described, "ServiceLinkedRoleARN"),
        default_service_linked_role_arn(),
        "the group must report the role it runs under, got {described}"
    );
    // The zones in BOTH spellings: a group placed by zone name reports the ids of those
    // zones too.
    assert_eq!(
        list_members(&described, "AvailabilityZoneIds").len(),
        1,
        "the group must report the id of the one zone it is placed in, got {described}"
    );
    // A collection the group holds nothing in is reported as an EMPTY list, not as an absent
    // member: "attached to nothing" has to be distinguishable from "not reported".
    for collection in [
        "EnabledMetrics",
        "TargetGroupARNs",
        "TrafficSources",
        "LoadBalancerNames",
        "SuspendedProcesses",
    ] {
        let body = element_body(&described, collection).unwrap_or_else(|| {
            panic!("the group must report {collection} as an element, got {described}")
        });
        assert!(
            body.trim().is_empty(),
            "collection {collection:?}: a group holding nothing in it reports an empty \
             element, got {body}"
        );
    }
}

/// RULE: the launchability of a launch source is read from the VERSION the group references,
/// so a group pointed at an unlaunchable version is refused while the same template's
/// launchable version is served.
#[tokio::test]
async fn the_version_the_group_references_is_the_one_held_to_the_rule() {
    let (router, _engine) = emulator();
    let template = "lt-unlaunchable-version";
    let (status, body) = ec2_query(
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", template),
            p("LaunchTemplateData.ImageId", "ami-0123456789abcdef0"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
            p("LaunchTemplateData.NetworkInterface.1.DeviceIndex", "0"),
            p(
                "LaunchTemplateData.NetworkInterface.1.AssociatePublicIpAddress",
                "true",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate must answer 200, got {status}: {body}"
    );
    // A second version that asks for the same public address on a launch of TWO interfaces:
    // the template holds one launchable version and one that is not.
    let (status, body) = ec2_query(
        &router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateName", template),
            p("LaunchTemplateData.ImageId", "ami-0123456789abcdef0"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
            p("LaunchTemplateData.NetworkInterface.1.DeviceIndex", "0"),
            p(
                "LaunchTemplateData.NetworkInterface.1.AssociatePublicIpAddress",
                "true",
            ),
            p("LaunchTemplateData.NetworkInterface.2.DeviceIndex", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplateVersion must answer 200, got {status}: {body}"
    );
    assert_eq!(
        tag_value(&body, "versionNumber"),
        "2",
        "the second version must be minted next, got {body}"
    );

    // (case, the version the request names, whether it is served)
    let rows: &[(&str, &str, bool)] = &[
        ("the launchable first version", "1", true),
        ("the version that asks for both", "2", false),
        ("the newest version, which is that one", "$Latest", false),
    ];
    for (index, (case, version, served)) in rows.iter().enumerate() {
        let group = format!("asg-unlaunchable-version-{index}");
        let mut params = base_create(&group);
        params.push(p("LaunchTemplate.LaunchTemplateName", template));
        params.push(p("LaunchTemplate.Version", *version));
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be served, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert!(
            message(&body).contains(
                "The associatePublicIPAddress parameter cannot be specified when launching \
                 with multiple network interfaces."
            ),
            "case {case:?}: expected Amazon EC2's detail for the conflict, got {}",
            message(&body)
        );
    }
}

/// Mint one more version of the launch template `template_id` over Amazon EC2's own wire, and
/// answer the version number that service assigned it.
async fn add_template_version(router: &MultiServiceRouter, template_id: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateId", template_id),
            p("LaunchTemplateData.ImageId", "ami-0123456789abcdef0"),
            p("LaunchTemplateData.InstanceType", "t3.small"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplateVersion must answer 200, got {status}: {body}"
    );
    let version = tag_value(&body, "versionNumber");
    assert!(
        !version.is_empty(),
        "CreateLaunchTemplateVersion must report the version it minted, got {body}"
    );
    version
}
