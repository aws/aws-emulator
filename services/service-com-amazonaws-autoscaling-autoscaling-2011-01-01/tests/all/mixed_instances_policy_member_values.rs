// SPDX-License-Identifier: CC0-1.0

//! The VALUES a mixed instances policy's own members may carry — driven end to end over the
//! wire this service speaks (`awsQuery`), with Amazon EC2 on the same engine so the launch
//! template the policy names is the account's own.
//!
//! RULE: a member of a mixed instances policy carries a value the member PUBLISHES, and the
//! two members that say which instance types one override launches are alternatives:
//!
//! * "if you specify `InstanceRequirements`, you can't specify `InstanceType`"
//!   (`LaunchTemplateOverrides$InstanceRequirements`), so an override carrying both says it
//!   twice and differently;
//! * an override names an instance type that EXISTS — "you must specify an instance type that
//!   is supported in your requested Region and Availability Zones"
//!   (`LaunchTemplateOverrides$InstanceType`) — since an instance is placed as a type the
//!   service that owns instance types publishes;
//! * an allocation strategy is one of the values its own member lists
//!   (`InstancesDistribution$OnDemandAllocationStrategy`: `lowest-price`, `prioritized`;
//!   `$SpotAllocationStrategy`: `capacity-optimized`, `capacity-optimized-prioritized`,
//!   `lowest-price`, `price-capacity-optimized`), and the percentage that splits the capacity
//!   above the base is a PERCENTAGE ("expressed as a number … if set to 100, only On-Demand
//!   Instances are used", `$OnDemandPercentageAboveBaseCapacity`).
//!
//! A request outside any of those is refused HTTP 400 and nothing is stored; a request inside
//! all of them is served and the group is there to read.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the value, the
//! FORM of the value (a published strategy, an unpublished one, a type the catalog holds, two
//! it does not), WHERE in the policy it sits (the first override, a later one), and — for the
//! percentage — each published bound and one past it. Every value set written out here names
//! the member it comes from, and each is exercised on an entry the trace that found this did
//! not cite.
//!
//! The neighbouring suites cover the policy's other rules: `tests/all/launch_template_
//! instance_type.rs` (where the type has to be stated at all) and
//! `tests/all/launch_source_names_its_template.rs` (which template the policy launches from).

use crate::support::{
    borrowed, create_launch_template, emulator, p, query, tag_value, tag_values, xml_unescape,
};

/// The launch template every policy here launches from, created over Amazon EC2's own wire.
const TEMPLATE: &str = "lt-policy-member-values";

/// The zone every group here places its instances in.
const ZONE: &str = "us-west-2a";

/// Two instance types Amazon EC2's own catalog publishes, and two it does not — the second
/// unpublished one so the row that proves the rule is not the one value a recording carried.
const PUBLISHED_TYPE: &str = "t3.micro";
const OTHER_PUBLISHED_TYPE: &str = "m5.large";
const UNPUBLISHED_TYPE: &str = "t3.bogus";
const OTHER_UNPUBLISHED_TYPE: &str = "c9.enormous";

/// The codes Amazon EC2 Auto Scaling answers a request whose values it cannot use.
const VALIDATION_ERROR: &str = "ValidationError";

/// The sentences AWS answers each of these rules with, written out here rather than imported
/// so a wrong constant in the implementation cannot make this suite pass.
const OVERRIDE_STATES_BOTH: &str = "The parameter InstanceType cannot be used in a launch \
                                    template override using instance requirements.";
const ON_DEMAND_STRATEGY_NOT_VALID: &str =
    "OnDemandAllocationStrategy is not valid. Valid options are: [prioritized, lowest-price].";
const SPOT_STRATEGY_NOT_VALID: &str = "SpotAllocationStrategy is not valid. Valid options are: \
                                       [lowest-price, capacity-optimized, \
                                       capacity-optimized-prioritized, price-capacity-optimized].";

/// What a value outside the published range of one member is answered: the boundary
/// validator's own clause, naming the member by the path it spells and the bound it missed.
fn percentage_out_of_range(value: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at \
         'mixedInstancesPolicy.launchStrategy.onDemandCapacityOverBasePercentage' failed to \
         satisfy constraint: {constraint}"
    )
}

/// The first and a later override of one policy, and its instances distribution.
const FIRST_OVERRIDE: &str = "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1";
const SECOND_OVERRIDE: &str = "MixedInstancesPolicy.LaunchTemplate.Overrides.member.2";
const DISTRIBUTION: &str = "MixedInstancesPolicy.InstancesDistribution";

/// One row: the case, the policy members it states beside the launch template, and the
/// sentence it is refused with — empty where the request is served.
struct Row {
    case: &'static str,
    members: fn() -> Vec<(String, String)>,
    refusal: &'static str,
}

/// The requirements an override states where a row chooses its types by attributes rather
/// than by name: the two members `InstanceRequirements` requires
/// (`InstanceRequirements$VCpuCount` and `$MemoryMiB`).
fn requirements(prefix: &str) -> Vec<(String, String)> {
    vec![
        p(&format!("{prefix}.InstanceRequirements.VCpuCount.Min"), "2"),
        p(
            &format!("{prefix}.InstanceRequirements.MemoryMiB.Min"),
            "1024",
        ),
    ]
}

fn override_type(prefix: &str, instance_type: &str) -> Vec<(String, String)> {
    vec![p(&format!("{prefix}.InstanceType"), instance_type)]
}

fn distribution(member: &str, value: &str) -> Vec<(String, String)> {
    vec![p(&format!("{DISTRIBUTION}.{member}"), value)]
}

const ROWS: &[Row] = &[
    Row {
        case: "an override naming its instance type alone",
        members: || override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
        refusal: "",
    },
    Row {
        case: "an override stating instance requirements alone",
        members: || requirements(FIRST_OVERRIDE),
        refusal: "",
    },
    Row {
        case: "an override stating both ways of choosing its types",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                requirements(FIRST_OVERRIDE),
            ]
            .concat()
        },
        refusal: OVERRIDE_STATES_BOTH,
    },
    Row {
        case: "a later override stating both ways of choosing its types",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                override_type(SECOND_OVERRIDE, OTHER_PUBLISHED_TYPE),
                requirements(SECOND_OVERRIDE),
            ]
            .concat()
        },
        refusal: OVERRIDE_STATES_BOTH,
    },
    Row {
        case: "an override naming a second published type",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                override_type(SECOND_OVERRIDE, OTHER_PUBLISHED_TYPE),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "an override naming an unpublished instance type",
        members: || override_type(FIRST_OVERRIDE, UNPUBLISHED_TYPE),
        refusal: UNPUBLISHED_TYPE,
    },
    Row {
        case: "a later override naming another unpublished instance type",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                override_type(SECOND_OVERRIDE, OTHER_UNPUBLISHED_TYPE),
            ]
            .concat()
        },
        refusal: OTHER_UNPUBLISHED_TYPE,
    },
    Row {
        case: "the On-Demand allocation strategy that prioritises the overrides' order",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandAllocationStrategy", "prioritized"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "the On-Demand allocation strategy that takes the lowest price",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandAllocationStrategy", "lowest-price"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "an On-Demand allocation strategy the member does not publish",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandAllocationStrategy", "cheapest"),
            ]
            .concat()
        },
        refusal: ON_DEMAND_STRATEGY_NOT_VALID,
    },
    Row {
        case: "the Spot allocation strategy that optimises for capacity",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("SpotAllocationStrategy", "capacity-optimized"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "the Spot allocation strategy that optimises for capacity by priority",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("SpotAllocationStrategy", "capacity-optimized-prioritized"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "the Spot allocation strategy that optimises for price and capacity",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("SpotAllocationStrategy", "price-capacity-optimized"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "a Spot allocation strategy the member does not publish",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("SpotAllocationStrategy", "capacity"),
            ]
            .concat()
        },
        refusal: SPOT_STRATEGY_NOT_VALID,
    },
    Row {
        case: "the lowest percentage above the base the member admits",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandPercentageAboveBaseCapacity", "0"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "a percentage inside the range",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandPercentageAboveBaseCapacity", "50"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "the highest percentage the member admits",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandPercentageAboveBaseCapacity", "100"),
            ]
            .concat()
        },
        refusal: "",
    },
    Row {
        case: "one past the highest percentage",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandPercentageAboveBaseCapacity", "101"),
            ]
            .concat()
        },
        refusal: "101",
    },
    Row {
        case: "one past the lowest percentage",
        members: || {
            [
                override_type(FIRST_OVERRIDE, PUBLISHED_TYPE),
                distribution("OnDemandPercentageAboveBaseCapacity", "-1"),
            ]
            .concat()
        },
        refusal: "-1",
    },
];

/// The sentence one row is refused with: the rows whose `refusal` is a VALUE of the request
/// name the value, since those two sentences carry it.
fn expected_message(row: &Row) -> String {
    match row.refusal {
        UNPUBLISHED_TYPE | OTHER_UNPUBLISHED_TYPE => {
            format!("The specified instance type {} is not valid", row.refusal)
        }
        "101" => percentage_out_of_range("101", "Member must have value less than or equal to 100"),
        "-1" => {
            percentage_out_of_range("-1", "Member must have value greater than or equal to 0")
        }
        sentence => sentence.to_string(),
    }
}

/// The policy one row states, launching from the template the account holds.
fn policy(row: &Row) -> Vec<(String, String)> {
    let mut params = vec![p(
        "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
        TEMPLATE,
    )];
    params.extend((row.members)());
    params
}

/// The same policy as the DESIRED CONFIGURATION of one instance refresh: the members an
/// operation that states a group's configuration carries are the same members, under that
/// request's own prefix.
fn desired_configuration(row: &Row) -> Vec<(String, String)> {
    policy(row)
        .into_iter()
        .map(|(member, value)| (format!("DesiredConfiguration.{member}"), value))
        .collect()
}

/// The `(code, message)` of a refusal, with the message unescaped as it was worded.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

#[tokio::test]
async fn a_created_group_carries_only_policy_values_its_members_publish() {
    for row in ROWS {
        let case = row.case;
        let (router, _engine) = emulator();
        create_launch_template(&router, TEMPLATE).await;
        let name = "asg-policy-member-values";
        let mut params = vec![
            p("AutoScalingGroupName", name),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", ZONE),
        ];
        params.extend(policy(row));

        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        let (describe_status, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name)],
        )
        .await;
        assert_eq!(
            describe_status, 200,
            "case {case:?}: DescribeAutoScalingGroups must answer 200, got {describe_status}: \
             {described}"
        );
        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a policy whose values its members publish must be accepted, got \
                 {status}: {body}"
            );
            assert!(
                tag_values(&described, "AutoScalingGroupName").contains(&name.to_string()),
                "case {case:?}: the accepted create must leave the group behind, got {described}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a policy carrying a value its member does not publish must be \
             refused, got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            (answered.0.as_str(), answered.1.as_str()),
            (VALIDATION_ERROR, expected_message(row).as_str()),
            "case {case:?}: expected this rule's refusal, got {answered:?} with {body}"
        );
        assert!(
            tag_values(&described, "AutoScalingGroupName").is_empty(),
            "case {case:?}: the refused create must leave no group behind, got {described}"
        );
    }
}

/// RULE, of the other writer of a group's policy: an update that states a mixed instances
/// policy is held to the same rules, and a refused update leaves the group exactly as it was.
#[tokio::test]
async fn an_update_carries_only_policy_values_its_members_publish() {
    for row in ROWS {
        let case = row.case;
        let (router, _engine) = emulator();
        create_launch_template(&router, TEMPLATE).await;
        let name = "asg-policy-member-values-update";
        let standing = vec![
            p("AutoScalingGroupName", name),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", ZONE),
            p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the update drives must be created, got {status}: {body}"
        );

        let mut params = vec![p("AutoScalingGroupName", name)];
        params.extend(policy(row));
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&params)).await;
        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: an update whose policy values its members publish must be \
                 served, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: an update carrying a value its member does not publish must be \
             refused, got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            (answered.0.as_str(), answered.1.as_str()),
            (VALIDATION_ERROR, expected_message(row).as_str()),
            "case {case:?}: expected this rule's refusal, got {answered:?} with {body}"
        );
        let (describe_status, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name)],
        )
        .await;
        assert_eq!(
            describe_status, 200,
            "case {case:?}: DescribeAutoScalingGroups must answer 200, got {describe_status}: \
             {described}"
        );
        assert!(
            !described.contains("<MixedInstancesPolicy")
                && tag_value(&described, "LaunchTemplateName") == TEMPLATE,
            "case {case:?}: the refused update must leave the group on the launch template it \
             was created with and give it no policy, got {described}"
        );
    }
}

/// RULE, of the third request that states a group's configuration: the desired configuration
/// of one instance refresh carries the same policy members under the same rules, and a refused
/// refresh starts nothing.
#[tokio::test]
async fn a_desired_configuration_carries_only_policy_values_its_members_publish() {
    for row in ROWS {
        let case = row.case;
        let (router, _engine) = emulator();
        create_launch_template(&router, TEMPLATE).await;
        let name = "asg-policy-member-values-refresh";
        let standing = vec![
            p("AutoScalingGroupName", name),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", ZONE),
            p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the refresh drives must be created, got {status}: {body}"
        );

        let mut params = vec![p("AutoScalingGroupName", name)];
        params.extend(desired_configuration(row));
        let (status, body) = query(&router, "StartInstanceRefresh", &borrowed(&params)).await;
        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a desired configuration whose policy values its members \
                 publish must be served, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a desired configuration carrying a value its member does not \
             publish must be refused, got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            (answered.0.as_str(), answered.1.as_str()),
            (VALIDATION_ERROR, expected_message(row).as_str()),
            "case {case:?}: expected this rule's refusal, got {answered:?} with {body}"
        );
        let (describe_status, described) = query(
            &router,
            "DescribeInstanceRefreshes",
            &[("AutoScalingGroupName", name)],
        )
        .await;
        assert_eq!(
            describe_status, 200,
            "case {case:?}: DescribeInstanceRefreshes must answer 200, got {describe_status}: \
             {described}"
        );
        assert!(
            tag_values(&described, "InstanceRefreshId").is_empty(),
            "case {case:?}: the refused refresh must leave no instance refresh behind, got \
             {described}"
        );
    }
}
