// SPDX-License-Identifier: CC0-1.0

//! A launch-template specification states EXACTLY ONE half of the template's identity —
//! driven end to end over the wire this service speaks (`awsQuery`), with Amazon EC2 on the
//! same engine so the template under test is the account's own.
//!
//! RULE: "Conditional: You must specify either a `LaunchTemplateId` or a
//! `LaunchTemplateName`" (`LaunchTemplateSpecification`, which both halves carry): the two
//! members are alternative spellings of ONE template's identity, so a specification carrying
//! neither names no template to launch from and one carrying both names two templates where a
//! launch source has one. Either way the request that would configure the group is refused
//! `ValidationError` with the sentence naming that pair of members, and the account is left
//! exactly as it was.
//!
//! The rows differ along what the rule generalises over: HOW the specification states the
//! identity (the id alone, the name alone, both halves, neither — nothing but a `Version`),
//! WHERE the specification sits (the group's own `LaunchTemplate`, the launch template of a
//! `MixedInstancesPolicy`, and the one an override of that policy names), and WHICH request
//! carries it (`CreateAutoScalingGroup`, `UpdateAutoScalingGroup` and the desired
//! configuration of `StartInstanceRefresh`). The served rows — each half on its own — sit in
//! the same tables, so a rule that refused too much could not pass either, and the create's
//! served rows are read back through `DescribeAutoScalingGroups`: the identity a request
//! states is the identity a client reads off the group.
//!
//! `tests/all/launch_source_names_its_template.rs` covers the same pair of members from the
//! other side, where a launch source names no template anywhere.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, create_launch_template, emulator, p, query, tag_value, tag_values, xml_unescape,
};

/// The launch template every row resolves against, created over Amazon EC2's own wire.
const TEMPLATE: &str = "lt-identity-exclusive";

/// The zone every group here places its instances in.
const ZONE: &str = "us-west-2a";

/// The instance type an override names, where a row puts its specification on one. Any type is
/// a valid `InstanceType` string (`LaunchTemplateOverrides$InstanceType`); which types exist is
/// not what this suite is about.
const OVERRIDE_TYPE: &str = "t3.micro";

/// The code Amazon EC2 Auto Scaling answers a request whose members it cannot read as one
/// launch source, and the sentence it answers this pair of members with — both written out
/// here rather than imported, so a wrong constant in the implementation cannot make this suite
/// pass.
const VALIDATION_ERROR: &str = "ValidationError";
const NAMES_NO_ONE_TEMPLATE: &str =
    "Valid requests must contain either launchTemplateId or LaunchTemplateName";

/// HOW one row states the template's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    /// `LaunchTemplateId` alone, the id Amazon EC2 minted for the template.
    Id,
    /// `LaunchTemplateName` alone.
    Name,
    /// Both halves, each naming the same template the other does.
    Both,
    /// Neither half — a specification carrying nothing but its `Version`.
    Neither,
}

impl Identity {
    /// Whether a specification stating the identity this way names exactly one template.
    fn served(self) -> bool {
        matches!(self, Self::Id | Self::Name)
    }
}

/// WHERE the specification under test sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// The group's own `LaunchTemplate`.
    Group,
    /// The `LaunchTemplateSpecification` of a `MixedInstancesPolicy`'s launch template.
    Policy,
    /// The specification one override of that policy names, beside a policy launch template
    /// that names the template properly — so the verdict is about the override's own
    /// specification.
    Override,
}

/// One row of every table here.
struct Row {
    case: &'static str,
    place: Place,
    identity: Identity,
}

const ROWS: &[Row] = &[
    Row {
        case: "the group's own launch template, by the id Amazon EC2 minted",
        place: Place::Group,
        identity: Identity::Id,
    },
    Row {
        case: "the group's own launch template, by name",
        place: Place::Group,
        identity: Identity::Name,
    },
    Row {
        case: "the group's own launch template, by id AND name",
        place: Place::Group,
        identity: Identity::Both,
    },
    Row {
        case: "the group's own launch template, by neither half",
        place: Place::Group,
        identity: Identity::Neither,
    },
    Row {
        case: "a mixed instances policy's launch template, by id",
        place: Place::Policy,
        identity: Identity::Id,
    },
    Row {
        case: "a mixed instances policy's launch template, by name",
        place: Place::Policy,
        identity: Identity::Name,
    },
    Row {
        case: "a mixed instances policy's launch template, by id AND name",
        place: Place::Policy,
        identity: Identity::Both,
    },
    Row {
        case: "a mixed instances policy's launch template, by neither half",
        place: Place::Policy,
        identity: Identity::Neither,
    },
    Row {
        case: "the template an override names, by id",
        place: Place::Override,
        identity: Identity::Id,
    },
    Row {
        case: "the template an override names, by name",
        place: Place::Override,
        identity: Identity::Name,
    },
    Row {
        case: "the template an override names, by id AND name",
        place: Place::Override,
        identity: Identity::Both,
    },
    Row {
        case: "the template an override names, by neither half",
        place: Place::Override,
        identity: Identity::Neither,
    },
];

/// The launch-source parameters one row states, under `root` — empty for the two operations
/// that configure the group, `DesiredConfiguration.` for the refresh that states one.
fn launch_source(root: &str, row: &Row, id: &str, version: &str) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = Vec::new();
    if row.place == Place::Override {
        params.push(p(
            &format!(
                "{root}MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                 LaunchTemplateName"
            ),
            TEMPLATE,
        ));
        params.push(p(
            &format!("{root}MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType"),
            OVERRIDE_TYPE,
        ));
    }
    let specification = match row.place {
        Place::Group => format!("{root}LaunchTemplate"),
        Place::Policy => {
            format!("{root}MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification")
        }
        Place::Override => format!(
            "{root}MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.\
             LaunchTemplateSpecification"
        ),
    };
    match row.identity {
        Identity::Id => params.push(p(&format!("{specification}.LaunchTemplateId"), id)),
        Identity::Name => params.push(p(&format!("{specification}.LaunchTemplateName"), TEMPLATE)),
        Identity::Both => {
            params.push(p(&format!("{specification}.LaunchTemplateId"), id));
            params.push(p(&format!("{specification}.LaunchTemplateName"), TEMPLATE));
        }
        Identity::Neither => params.push(p(&format!("{specification}.Version"), version)),
    }
    params
}

/// One emulator holding the template every row resolves against, and the `(id, version)`
/// Amazon EC2 assigned it.
async fn seeded() -> (MultiServiceRouter, String, String) {
    let (router, _engine) = emulator();
    let (id, version) = create_launch_template(&router, TEMPLATE).await;
    (router, id, version)
}

/// The `(code, message)` of a refusal, with the message unescaped as it was worded.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// Assert `body` is this suite's refusal of a specification that names no one template.
fn assert_refused(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a specification that does not name exactly one launch template must be \
         refused, got {status}: {body}"
    );
    let answered = refusal(body);
    assert_eq!(
        (answered.0.as_str(), answered.1.as_str()),
        (VALIDATION_ERROR, NAMES_NO_ONE_TEMPLATE),
        "case {case:?}: expected the identity refusal, got {answered:?} with {body}"
    );
}

/// The parameters of a create under `name` carrying `source`.
fn create_params(name: &str, source: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("AvailabilityZones.member.1", ZONE),
    ];
    params.extend(source);
    params
}

/// Whether `DescribeAutoScalingGroups` reports a group under `name`, and its body.
async fn described(router: &MultiServiceRouter, case: &str, name: &str) -> String {
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
    body
}

#[tokio::test]
async fn a_create_states_one_half_of_the_launch_template_identity() {
    for row in ROWS {
        let case = row.case;
        let (router, id, version) = seeded().await;
        let name = "asg-identity-exclusive";
        let params = create_params(name, launch_source("", row, &id, &version));

        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if row.identity.served() {
            assert_eq!(
                status, 200,
                "case {case:?}: a specification stating one half of the identity must be \
                 accepted, got {status}: {body}"
            );
            // The identity a request states is the identity a client reads back: the group
            // reports the template by BOTH halves, whichever one the request wrote.
            let read = described(&router, case, name).await;
            assert!(
                tag_values(&read, "LaunchTemplateName").contains(&TEMPLATE.to_string())
                    && tag_values(&read, "LaunchTemplateId").contains(&id),
                "case {case:?}: the created group must report the template's name and the id \
                 {id:?}, got {read}"
            );
            continue;
        }
        assert_refused(case, status, &body);
        assert!(
            tag_values(&described(&router, case, name).await, "AutoScalingGroupName").is_empty(),
            "case {case:?}: the refused create must leave no group of that name behind"
        );

        // Nothing of that name exists, so the identical request is refused on the SAME
        // grounds rather than for a name it never took.
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_refused(case, status, &body);
    }
}

#[tokio::test]
async fn an_update_states_one_half_of_the_launch_template_identity() {
    for row in ROWS {
        let case = row.case;
        let (router, id, version) = seeded().await;
        let name = "asg-identity-exclusive-update";
        let standing = create_params(
            name,
            vec![p("LaunchTemplate.LaunchTemplateName", TEMPLATE)],
        );
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the update drives must be created, got {status}: {body}"
        );

        let mut params = vec![p("AutoScalingGroupName", name)];
        params.extend(launch_source("", row, &id, &version));
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&params)).await;
        if row.identity.served() {
            assert_eq!(
                status, 200,
                "case {case:?}: an update stating one half of the identity must be served, got \
                 {status}: {body}"
            );
            continue;
        }
        assert_refused(case, status, &body);
        // The refused update changed nothing: the group still launches from the template it
        // was created with, read back the way a client reads it.
        let read = described(&router, case, name).await;
        assert_eq!(
            tag_value(&read, "LaunchTemplateName"),
            TEMPLATE,
            "case {case:?}: the refused update must leave the group's launch template \
             untouched, got {read}"
        );
    }
}

#[tokio::test]
async fn a_desired_configuration_states_one_half_of_the_launch_template_identity() {
    for row in ROWS {
        let case = row.case;
        let (router, id, version) = seeded().await;
        let name = "asg-identity-exclusive-refresh";
        let standing = create_params(
            name,
            vec![p("LaunchTemplate.LaunchTemplateName", TEMPLATE)],
        );
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the refresh drives must be created, got {status}: {body}"
        );

        let mut params = vec![p("AutoScalingGroupName", name)];
        params.extend(launch_source("DesiredConfiguration.", row, &id, &version));
        let (status, body) = query(&router, "StartInstanceRefresh", &borrowed(&params)).await;
        if row.identity.served() {
            assert_eq!(
                status, 200,
                "case {case:?}: a desired configuration stating one half of the identity must \
                 be served, got {status}: {body}"
            );
            continue;
        }
        assert_refused(case, status, &body);
        // The refused refresh started nothing: a client reading the group's refreshes sees
        // none.
        let (status, body) = query(
            &router,
            "DescribeInstanceRefreshes",
            &[("AutoScalingGroupName", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInstanceRefreshes must answer 200, got {status}: {body}"
        );
        assert!(
            tag_values(&body, "InstanceRefreshId").is_empty(),
            "case {case:?}: the refused refresh must leave no instance refresh behind, got {body}"
        );
    }
}
