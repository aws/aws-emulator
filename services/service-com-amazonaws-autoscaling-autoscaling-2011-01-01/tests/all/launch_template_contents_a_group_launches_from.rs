// SPDX-License-Identifier: CC0-1.0

//! The launch-template CONTENTS a group may be configured against — driven end to end over
//! the wire this service speaks (`awsQuery`), with Amazon EC2 on the same engine so every
//! template, key pair and launch parameter under test is the account's own, written by a
//! client through the service that owns it.
//!
//! RULE: Amazon EC2 Auto Scaling reads the launch-template version a group would launch from
//! when the group is CONFIGURED, and refuses the request when those launch parameters are ones
//! no launch could apply — the group would otherwise be left permanently unable to hold
//! capacity with nothing telling the caller why. Two such parameters here:
//!
//! * the security groups belong INSIDE the network interface a version declares: "if you
//!   specify a network interface, you must specify any security groups as part of the network
//!   interface instead of using this parameter"
//!   (`RequestLaunchTemplateData$SecurityGroupIds`, and the same on `$SecurityGroups`), so a
//!   version declaring an interface and naming groups on its own members is refused
//!   `InvalidQueryParameter`;
//! * the key pair a version names has to be one the account HOLDS ("the name of the key
//!   pair", `RequestLaunchTemplateData$KeyName`), since a launch into a key pair Amazon EC2
//!   does not have places nothing — refused `ValidationError`, with that service's own
//!   sentence for the name after this service's preamble for a launch source it cannot use.
//!
//! The rows differ along what the rule generalises over: WHICH member names the groups (the
//! ids, the names), WHERE they are named (inside the interface, on the template's own
//! members, both places and neither), whether a version declares an interface at all, and
//! whether the key pair the version names is one the account holds. Each refusal is taken
//! twice — once by the create, once by the update that would point a standing group at the
//! same version — and the served rows sit in the same tables, so a rule that refused too much
//! could not pass either.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, ec2_query, emulator, p, query, tag_value, tag_values, xml_unescape, SEEDED_IMAGE_ID,
    SEEDED_INSTANCE_TYPE,
};

/// The template every group here is created from — the one a row then tries to move the group
/// ONTO is the row's own.
const GOOD_TEMPLATE: &str = "lt-contents-good";

/// The zone every group here places its instances in.
const ZONE: &str = "us-west-2a";

/// A security group id and a security group name a version may name. Neither has to resolve
/// to a group of the account: WHERE a version names its groups is a rule about the launch
/// parameters, decided before any group is looked up.
const GROUP_ID: &str = "sg-0123456789abcdef0";
const GROUP_NAME: &str = "web";

/// The key pair this suite creates through Amazon EC2, and a name no key pair of the account
/// is held under.
const HELD_KEY_PAIR: &str = "kp-contents-held";
const ABSENT_KEY_PAIR: &str = "kp-contents-absent";

/// The codes Amazon EC2 Auto Scaling answers these two refusals with.
const VALIDATION_ERROR: &str = "ValidationError";
const INVALID_QUERY_PARAMETER: &str = "InvalidQueryParameter";

/// The sentence it answers a version naming its security groups outside the interface with,
/// written out here rather than imported so a wrong constant in the implementation cannot make
/// this suite pass. Entirely fixed text.
const GROUPS_BELONG_TO_THE_INTERFACE: &str = "Invalid launch template: When a network interface \
                                              is provided, the security groups must be a part \
                                              of it.";

/// The sentence it answers a version naming a key pair the account does not hold with: this
/// service's preamble for a launch source it cannot use, then Amazon EC2's own sentence for
/// the name, whose one variable part is the name the version carries.
fn absent_key_pair_message(key_name: &str) -> String {
    format!(
        "You must use a valid fully-formed launch template. The key pair '{key_name}' does not \
         exist"
    )
}

/// One row: the case, the launch parameters the template under test carries beside the image
/// and the instance type, and the `(code, message)` a request pointing a group at it is
/// refused with — `None` where the request is served.
struct Row {
    case: &'static str,
    parameters: fn() -> Vec<(String, String)>,
    refusal: Option<(&'static str, &'static str)>,
}

/// One network interface, with the security groups a row names INSIDE it.
fn interface(groups: &[&str]) -> Vec<(String, String)> {
    let mut params = vec![p("LaunchTemplateData.NetworkInterface.1.DeviceIndex", "0")];
    for (position, group) in groups.iter().enumerate() {
        params.push(p(
            &format!(
                "LaunchTemplateData.NetworkInterface.1.SecurityGroupId.{}",
                position + 1
            ),
            *group,
        ));
    }
    params
}

/// The security groups a row names on the template's OWN members, by id and by name.
fn groups_outside(ids: &[&str], names: &[&str]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (position, id) in ids.iter().enumerate() {
        params.push(p(
            &format!("LaunchTemplateData.SecurityGroupId.{}", position + 1),
            *id,
        ));
    }
    for (position, name) in names.iter().enumerate() {
        params.push(p(
            &format!("LaunchTemplateData.SecurityGroup.{}", position + 1),
            *name,
        ));
    }
    params
}

fn key_pair(key_name: &str) -> Vec<(String, String)> {
    vec![p("LaunchTemplateData.KeyName", key_name)]
}

const ROWS: &[Row] = &[
    Row {
        case: "no interface and no security groups",
        parameters: Vec::new,
        refusal: None,
    },
    Row {
        case: "security groups on the template's own members, with no interface",
        parameters: || groups_outside(&[GROUP_ID], &[GROUP_NAME]),
        refusal: None,
    },
    Row {
        case: "an interface carrying its own security groups",
        parameters: || interface(&[GROUP_ID]),
        refusal: None,
    },
    Row {
        case: "an interface carrying no security group at all",
        parameters: || interface(&[]),
        refusal: None,
    },
    Row {
        case: "an interface beside security group IDS on the template's own member",
        parameters: || [interface(&[GROUP_ID]), groups_outside(&[GROUP_ID], &[])].concat(),
        refusal: Some((INVALID_QUERY_PARAMETER, GROUPS_BELONG_TO_THE_INTERFACE)),
    },
    Row {
        case: "an interface beside security group NAMES on the template's own member",
        parameters: || [interface(&[GROUP_ID]), groups_outside(&[], &[GROUP_NAME])].concat(),
        refusal: Some((INVALID_QUERY_PARAMETER, GROUPS_BELONG_TO_THE_INTERFACE)),
    },
    Row {
        case: "an interface with no groups of its own, beside groups on the template's member",
        parameters: || [interface(&[]), groups_outside(&[GROUP_ID], &[])].concat(),
        refusal: Some((INVALID_QUERY_PARAMETER, GROUPS_BELONG_TO_THE_INTERFACE)),
    },
    Row {
        case: "a key pair the account holds",
        parameters: || key_pair(HELD_KEY_PAIR),
        refusal: None,
    },
    Row {
        case: "a key pair the account does not hold",
        parameters: || key_pair(ABSENT_KEY_PAIR),
        refusal: Some((VALIDATION_ERROR, ABSENT_KEY_PAIR)),
    },
    Row {
        case: "a key pair the account does not hold, beside an interface of its own",
        parameters: || [interface(&[GROUP_ID]), key_pair(ABSENT_KEY_PAIR)].concat(),
        refusal: Some((VALIDATION_ERROR, ABSENT_KEY_PAIR)),
    },
];

/// The `(code, message)` one row is refused with: the key-pair rows name the KEY PAIR, since
/// that sentence carries it.
fn expected_refusal(row: &Row) -> Option<(String, String)> {
    let (code, detail) = row.refusal?;
    let message = if detail == ABSENT_KEY_PAIR {
        absent_key_pair_message(ABSENT_KEY_PAIR)
    } else {
        detail.to_string()
    };
    Some((code.to_string(), message))
}

/// The `(code, message)` of a refusal, with the message unescaped as it was worded.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// Create one launch template over Amazon EC2's own wire with `parameters` beside the image
/// and instance type every template here carries, and answer the name it was created under.
///
/// Amazon EC2 STORES such a version whatever this suite's rows carry — a template holds launch
/// parameters rather than placing anything — so the rows are templates a client really has.
async fn template_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    parameters: &[(String, String)],
) {
    let mut params = vec![
        p("LaunchTemplateName", name),
        p("LaunchTemplateData.ImageId", SEEDED_IMAGE_ID),
        p("LaunchTemplateData.InstanceType", SEEDED_INSTANCE_TYPE),
    ];
    params.extend(parameters.iter().cloned());
    let (status, body) = ec2_query(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate for {name:?} expected 200, got {status}: {body}"
    );
}

/// Put the key pair this suite's served row names in the account's own Amazon EC2 store,
/// through that service's own writer.
async fn create_key_pair(router: &MultiServiceRouter, case: &str) {
    let (status, body) = ec2_query(router, "CreateKeyPair", &[p("KeyName", HELD_KEY_PAIR)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateKeyPair for {HELD_KEY_PAIR:?} expected 200, got {status}: {body}"
    );
}

/// One emulator holding the key pair, the template every group is created from, and the
/// template one row puts under test.
async fn seeded(case: &str, row: &Row) -> MultiServiceRouter {
    let (router, _engine) = emulator();
    create_key_pair(&router, case).await;
    template_with(&router, case, GOOD_TEMPLATE, &[]).await;
    template_with(&router, case, "lt-contents-row", &(row.parameters)()).await;
    router
}

/// The template one row puts under test.
const ROW_TEMPLATE: &str = "lt-contents-row";

/// Whether the account holds a group under `name`, read the way a client reads it.
async fn group_names(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups must answer 200, got {status}: {body}"
    );
    tag_values(&body, "AutoScalingGroupName")
}

#[tokio::test]
async fn a_group_is_created_only_from_launch_parameters_a_launch_could_apply() {
    for row in ROWS {
        let case = row.case;
        let router = seeded(case, row).await;
        let name = "asg-template-contents";
        let params = vec![
            p("AutoScalingGroupName", name),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", ZONE),
            p("LaunchTemplate.LaunchTemplateName", ROW_TEMPLATE),
        ];

        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        let Some((code, message)) = expected_refusal(row) else {
            assert_eq!(
                status, 200,
                "case {case:?}: launch parameters a launch could apply must be accepted, got \
                 {status}: {body}"
            );
            assert!(
                group_names(&router, case, name).await.contains(&name.to_string()),
                "case {case:?}: the accepted create must leave the group behind"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: launch parameters no launch could apply must be refused, got \
             {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            answered,
            (code.clone(), message.clone()),
            "case {case:?}: expected this rule's refusal, got {answered:?} with {body}"
        );
        assert!(
            group_names(&router, case, name).await.is_empty(),
            "case {case:?}: the refused create must leave no group behind"
        );

        // Nothing of that name exists, so the identical request is refused on the SAME
        // grounds rather than for a name it never took.
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        let retried = refusal(&body);
        assert_eq!(
            (status, retried),
            (400, (code, message)),
            "case {case:?}: the retry must be refused the same way, got {status}: {body}"
        );
    }
}

/// RULE, of the other writer of a group's launch source: an update that points a standing
/// group at a version is held to the same rules, and a refused update leaves the group on the
/// template it was created with.
#[tokio::test]
async fn an_update_points_a_group_only_at_launch_parameters_a_launch_could_apply() {
    for row in ROWS {
        let case = row.case;
        let router = seeded(case, row).await;
        let name = "asg-template-contents-update";
        let standing = vec![
            p("AutoScalingGroupName", name),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", ZONE),
            p("LaunchTemplate.LaunchTemplateName", GOOD_TEMPLATE),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the update drives must be created, got {status}: {body}"
        );

        let params = vec![
            p("AutoScalingGroupName", name),
            p("LaunchTemplate.LaunchTemplateName", ROW_TEMPLATE),
        ];
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&params)).await;
        let Some((code, message)) = expected_refusal(row) else {
            assert_eq!(
                status, 200,
                "case {case:?}: an update onto launch parameters a launch could apply must be \
                 served, got {status}: {body}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: an update onto launch parameters no launch could apply must be \
             refused, got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            answered,
            (code, message),
            "case {case:?}: expected this rule's refusal, got {answered:?} with {body}"
        );

        let (status, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups must answer 200, got {status}: {described}"
        );
        assert_eq!(
            tag_value(&described, "LaunchTemplateName"),
            GOOD_TEMPLATE,
            "case {case:?}: the refused update must leave the group's launch template \
             untouched, got {described}"
        );
    }
}

/// RULE, of the third request that states a group's configuration: a refresh's desired
/// configuration names a launch-template version under the same rules, and a refused refresh
/// starts nothing.
#[tokio::test]
async fn a_desired_configuration_names_only_launch_parameters_a_launch_could_apply() {
    for row in ROWS {
        let case = row.case;
        let router = seeded(case, row).await;
        let name = "asg-template-contents-refresh";
        let standing = vec![
            p("AutoScalingGroupName", name),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", ZONE),
            p("LaunchTemplate.LaunchTemplateName", GOOD_TEMPLATE),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the refresh drives must be created, got {status}: {body}"
        );

        let params = vec![
            p("AutoScalingGroupName", name),
            p(
                "DesiredConfiguration.LaunchTemplate.LaunchTemplateName",
                ROW_TEMPLATE,
            ),
        ];
        let (status, body) = query(&router, "StartInstanceRefresh", &borrowed(&params)).await;
        let Some((code, message)) = expected_refusal(row) else {
            assert_eq!(
                status, 200,
                "case {case:?}: a desired configuration naming launch parameters a launch \
                 could apply must be served, got {status}: {body}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: a desired configuration naming launch parameters no launch could \
             apply must be refused, got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            answered,
            (code, message),
            "case {case:?}: expected this rule's refusal, got {answered:?} with {body}"
        );

        let (status, described) = query(
            &router,
            "DescribeInstanceRefreshes",
            &[("AutoScalingGroupName", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInstanceRefreshes must answer 200, got {status}: {described}"
        );
        assert!(
            tag_values(&described, "InstanceRefreshId").is_empty(),
            "case {case:?}: the refused refresh must leave no instance refresh behind, got \
             {described}"
        );
    }
}
