// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE a request that configures a group against a launch template the account does
//! not hold is refused with.
//!
//! RULE under test: Amazon EC2 Auto Scaling resolves a launch-template reference through the
//! service that OWNS launch templates, so a reference that does not resolve is reported with
//! that service's own sentence for what did not resolve, under HTTP 400 and `ValidationError`,
//! wherever the reference is carried and whichever request carries it, and nothing the refused
//! request asked for is stored:
//!
//! * a NAME nothing holds, and an id Amazon EC2's mint ISSUED whose template has since been
//!   deleted, are a template that does not exist — `The specified launch template, with
//!   template name <name>, does not exist.` and `… with template ID <id>, does not exist.`;
//! * an id that mint never issued is no launch template id at all: Amazon EC2 screens an id
//!   before it looks it up and refuses one it cannot read as malformed (`Malformed launch
//!   template Id: <id>`), so that is what a group configured against such an id is told.
//!
//! The rows span what the rule generalises over: which half of the reference the request
//! states, where the reference is carried (the group's own `LaunchTemplate`, the launch
//! template of a `MixedInstancesPolicy`, the one an override of that policy names, the desired
//! configuration of an instance refresh), which request carries it (the create, the update,
//! the refresh), and what became of the template the reference names — held and then deleted,
//! or never minted at all — which is what decides WHICH of the two sentences it earns. A
//! second table spans the id FORMS the mint never produces.
//!
//! Auto Scaling and Amazon EC2 are mounted on ONE engine and driven over their own wire
//! protocols — awsQuery and ec2Query — so every template this suite creates and deletes is
//! written where a client writes it.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    borrowed, delete_launch_template, ec2_query, emulator, p, query, tag_value, tag_values,
    xml_unescape, REGION,
};

/// The AMI and instance type every launch template in this suite carries.
const IMAGE_ID: &str = "ami-00000000000000081";
const INSTANCE_TYPE: &str = "t3.micro";

/// The template the account holds for the whole suite, which the group under test launches
/// from before a row points it at a reference that resolves to nothing.
const HELD_TEMPLATE_NAME: &str = "dangling-held-template";

/// The template a row DELETES after the account held it.
const DELETED_TEMPLATE_NAME: &str = "dangling-deleted-template";

/// A name nothing in this suite creates, and an id of the published form (`lt-` and 17 hex
/// digits) no template of the account carries: either names a template that never existed.
const NEVER_CREATED_NAME: &str = "dangling-template-never-created";
const NEVER_CREATED_ID: &str = "lt-00000000000000999";

/// The group every update and refresh row drives, created from the held template.
const GROUP: &str = "dangling-template-group";

/// The error code AWS answers a request naming a resource that does not resolve.
const VALIDATION_ERROR: &str = "ValidationError";

/// Which half of the reference the row's request states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// `LaunchTemplateId`.
    Id,
    /// `LaunchTemplateName`.
    Name,
}

/// What became of the template the row's reference names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum History {
    /// The account held it and a client deleted it through Amazon EC2.
    Deleted,
    /// Nothing ever created it.
    NeverCreated,
}

/// Where the row's request carries the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// The group's own `LaunchTemplate`.
    Own,
    /// The launch template of a `MixedInstancesPolicy`.
    PolicyTemplate,
    /// The template an OVERRIDE of that policy names.
    PolicyOverride,
}

/// Which request carries the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    /// `CreateAutoScalingGroup`, under a name nothing holds yet.
    Create,
    /// `UpdateAutoScalingGroup`, on the group created from the held template.
    Update,
    /// `StartInstanceRefresh`, whose `DesiredConfiguration` carries the reference.
    Refresh,
}

/// One row of the table.
struct Row {
    case: &'static str,
    request: Request,
    carried: Carried,
    named: Named,
    history: History,
}

const ROWS: &[Row] = &[
    Row {
        case: "a create naming a deleted template by id",
        request: Request::Create,
        carried: Carried::Own,
        named: Named::Id,
        history: History::Deleted,
    },
    Row {
        case: "a create naming a deleted template by name",
        request: Request::Create,
        carried: Carried::Own,
        named: Named::Name,
        history: History::Deleted,
    },
    Row {
        case: "a create whose mixed instances policy names a deleted template by name",
        request: Request::Create,
        carried: Carried::PolicyTemplate,
        named: Named::Name,
        history: History::Deleted,
    },
    Row {
        case: "a create whose policy override names a deleted template by id",
        request: Request::Create,
        carried: Carried::PolicyOverride,
        named: Named::Id,
        history: History::Deleted,
    },
    Row {
        case: "a create naming a template that never existed, by id",
        request: Request::Create,
        carried: Carried::Own,
        named: Named::Id,
        history: History::NeverCreated,
    },
    Row {
        case: "a create naming a template that never existed, by name",
        request: Request::Create,
        carried: Carried::Own,
        named: Named::Name,
        history: History::NeverCreated,
    },
    Row {
        case: "an update moving a group onto a deleted template by name",
        request: Request::Update,
        carried: Carried::Own,
        named: Named::Name,
        history: History::Deleted,
    },
    Row {
        case: "an update whose mixed instances policy names a deleted template by id",
        request: Request::Update,
        carried: Carried::PolicyTemplate,
        named: Named::Id,
        history: History::Deleted,
    },
    Row {
        case: "an update naming a template that never existed, by name",
        request: Request::Update,
        carried: Carried::Own,
        named: Named::Name,
        history: History::NeverCreated,
    },
    Row {
        case: "a refresh whose desired configuration names a deleted template by id",
        request: Request::Refresh,
        carried: Carried::Own,
        named: Named::Id,
        history: History::Deleted,
    },
    Row {
        case: "a refresh whose desired configuration names a template that never existed, by \
               name",
        request: Request::Refresh,
        carried: Carried::Own,
        named: Named::Name,
        history: History::NeverCreated,
    },
];

/// The zone the group places instances in: the first of the Region's own catalog, read from
/// Amazon EC2 rather than written out here.
fn zone() -> String {
    ec2::availability_zones_in_region(REGION)
        .first()
        .map(|zone| zone.zone_name.clone())
        .expect("the Region has Availability Zones")
}

/// Create one launch template over Amazon EC2's own wire and answer the id that service
/// assigned it — never one this suite spells out.
async fn create_template(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", IMAGE_ID),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate for {name:?} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    assert!(
        !id.is_empty(),
        "case {case:?}: CreateLaunchTemplate must report the template's id, got {body}"
    );
    id
}

/// Create the group every update and refresh row drives, from the template the account holds.
async fn create_group(router: &MultiServiceRouter, case: &str) {
    let zone = zone();
    let params = vec![
        p("AutoScalingGroupName", GROUP),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "0"),
        p("AvailabilityZones.member.1", zone.as_str()),
        p("LaunchTemplate.LaunchTemplateName", HELD_TEMPLATE_NAME),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the group the row drives must be created, got {status}: {body}"
    );
}

/// The parameter prefix the reference's members are carried under, inside the request the row
/// makes.
fn prefix(request: Request, carried: Carried) -> String {
    let root = match request {
        Request::Refresh => "DesiredConfiguration.",
        Request::Create | Request::Update => "",
    };
    let tail = match carried {
        Carried::Own => "LaunchTemplate".to_string(),
        Carried::PolicyTemplate => {
            "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification".to_string()
        }
        Carried::PolicyOverride => "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.\
                                    LaunchTemplateSpecification"
            .to_string(),
    };
    format!("{root}{tail}")
}

/// The sentence AWS answers the row with, written out here rather than taken from the
/// implementation: fixed text around the ONE identifier the request stated.
///
/// Which sentence depends on what became of the template the reference names, because Amazon
/// EC2 SCREENS an id against its own mint before it looks it up: an id it issued names a
/// template that may since have gone, while an id it never issued is one it cannot read at
/// all.
fn expected_message(named: Named, history: History, value: &str) -> String {
    match (named, history) {
        (Named::Id, History::NeverCreated) => format!("Malformed launch template Id: {value}"),
        (Named::Id, History::Deleted) => {
            format!("The specified launch template, with template ID {value}, does not exist.")
        }
        (Named::Name, _) => {
            format!("The specified launch template, with template name {value}, does not exist.")
        }
    }
}

/// Whether `DescribeAutoScalingGroups` reports a group under `name`.
async fn group_exists(router: &MultiServiceRouter, name: &str) -> bool {
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
    !tag_values(&body, "AutoScalingGroupName").is_empty()
}

/// The `(LaunchTemplateId, LaunchTemplateName)` pair the group reports, read back through
/// `DescribeAutoScalingGroups`.
async fn reported_template(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", GROUP)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups must answer 200, got {status}: {body}"
    );
    (
        tag_value(&body, "LaunchTemplateId"),
        tag_value(&body, "LaunchTemplateName"),
    )
}

/// How many instance refreshes the group holds.
async fn refresh_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = query(
        router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", GROUP)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstanceRefreshes must answer 200, got {status}: {body}"
    );
    tag_values(&body, "InstanceRefreshId").len()
}

#[tokio::test]
async fn a_reference_that_resolves_to_no_template_is_refused_by_the_half_it_states() {
    for row in ROWS {
        let case = row.case;
        let (router, _engine) = emulator();
        let held_id = create_template(&router, case, HELD_TEMPLATE_NAME).await;

        // What the row's reference names, and the value it states it by: a template the
        // account held until a client deleted it, or one nothing ever created.
        let value = match (row.history, row.named) {
            (History::Deleted, named) => {
                let id = create_template(&router, case, DELETED_TEMPLATE_NAME).await;
                assert_ne!(
                    id, held_id,
                    "case {case:?}: the two templates must be two resources, got {id:?}"
                );
                delete_launch_template(&router, case, &id).await;
                match named {
                    Named::Id => id,
                    Named::Name => DELETED_TEMPLATE_NAME.to_string(),
                }
            }
            (History::NeverCreated, Named::Id) => NEVER_CREATED_ID.to_string(),
            (History::NeverCreated, Named::Name) => NEVER_CREATED_NAME.to_string(),
        };
        let member = match row.named {
            Named::Id => "LaunchTemplateId",
            Named::Name => "LaunchTemplateName",
        };
        let reference = format!("{}.{member}", prefix(row.request, row.carried));

        let zone = zone();
        let created_name = format!("{GROUP}-{}", row.case.len());
        let mut params = match row.request {
            Request::Create => vec![
                p("AutoScalingGroupName", created_name.as_str()),
                p("MinSize", "0"),
                p("MaxSize", "1"),
                p("DesiredCapacity", "0"),
                p("AvailabilityZones.member.1", zone.as_str()),
            ],
            Request::Update | Request::Refresh => {
                create_group(&router, case).await;
                vec![p("AutoScalingGroupName", GROUP)]
            }
        };
        params.push(p(&reference, value.clone()));
        if row.carried == Carried::PolicyOverride {
            // An override names the instance type it overrides, beside the template it
            // launches that type from, and the policy carries a launch template of its own.
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                 LaunchTemplateName",
                HELD_TEMPLATE_NAME,
            ));
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                INSTANCE_TYPE,
            ));
        }
        let action = match row.request {
            Request::Create => "CreateAutoScalingGroup",
            Request::Update => "UpdateAutoScalingGroup",
            Request::Refresh => "StartInstanceRefresh",
        };

        let (status, body) = query(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reference resolving to no template must be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            VALIDATION_ERROR,
            "case {case:?}: expected {VALIDATION_ERROR}, got {body}"
        );
        let answered = xml_unescape(&tag_value(&body, "Message"));
        let expected = expected_message(row.named, row.history, &value);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );

        // Nothing the refused request asked for was stored: no group was created, a group
        // that exists still launches from the template it resolved, and no refresh was
        // started.
        match row.request {
            Request::Create => assert!(
                !group_exists(&router, &created_name).await,
                "case {case:?}: the refused create must leave no group"
            ),
            Request::Update => {
                let reported = reported_template(&router).await;
                assert_eq!(
                    (reported.0.as_str(), reported.1.as_str()),
                    (held_id.as_str(), HELD_TEMPLATE_NAME),
                    "case {case:?}: the refused update must leave the group on the template it \
                     resolved, got {reported:?}"
                );
            }
            Request::Refresh => {
                let started = refresh_count(&router).await;
                assert_eq!(
                    started, 0,
                    "case {case:?}: the refused refresh must leave no instance refresh, got \
                     {started}"
                );
            }
        }
    }
}

/// The id FORMS that resolve to no launch template, each paired with whether Amazon EC2
/// reports it MALFORMED rather than absent: that service's LONG id form is an obfuscated
/// identifier it decodes before it resolves anything
/// ([`ec2::ComAmazonawsEc2Amazonec2State::long_form_id_was_never_issued`], where that clause
/// lives), so a long-form value above its mint's counter is an identifier it never issued,
/// while a value in any other form carries no identifier to decode and is looked up like any
/// other reference.
const UNRESOLVABLE_IDS: &[(&str, &str, bool)] = &[
    (
        "a long-form id far above every value the mint can reach",
        "lt-0123456789abcdef0",
        true,
    ),
    (
        "a short-form id, a form the mint never writes",
        "lt-0fedcba9",
        false,
    ),
    (
        "the all-zero placeholder, which carries no identifier at all",
        "lt-00000000000000000",
        false,
    ),
    ("an id that is not hexadecimal", "lt-zzzzzzzzzzzzzzzzz", false),
    (
        "an id carrying another resource kind's prefix",
        "sg-00000000000000001",
        false,
    ),
];

/// RULE: a `LaunchTemplateId` in the long form Amazon EC2 decodes whose identifier its mint
/// never issued is reported as that service reports it — `Malformed launch template Id: <id>`
/// — by every request that configures a group against it, while an id outside that form, and
/// one the mint DID issue, is looked up: the template it names is launched from, or reported
/// as not existing when the account holds none under it.
///
/// The rows differ along the FORM of the id (its printed length, its digits, its prefix and
/// the all-zero placeholder) and along what the mint did with it (never issued, issued and
/// deleted, issued and held), and each is driven through both requests that configure a
/// group — so what the rule pins is the screen, not one value.
#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_reported_as_amazon_ec2_reports_it() {
    let (router, _engine) = emulator();
    let case = "the mint screen";
    let held_id = create_template(&router, case, HELD_TEMPLATE_NAME).await;
    let deleted_id = create_template(&router, case, DELETED_TEMPLATE_NAME).await;
    delete_launch_template(&router, case, &deleted_id).await;
    create_group(&router, case).await;

    // (case, the id, the sentence it earns, or `None` where the create must be served)
    let mut rows: Vec<(String, String, Option<String>)> = UNRESOLVABLE_IDS
        .iter()
        .map(|(case, id, malformed)| {
            (
                (*case).to_string(),
                (*id).to_string(),
                Some(if *malformed {
                    format!("Malformed launch template Id: {id}")
                } else {
                    format!(
                        "The specified launch template, with template ID {id}, does not exist."
                    )
                }),
            )
        })
        .collect();
    rows.push((
        "an id the mint really issued, whose template has since been deleted".to_string(),
        deleted_id.clone(),
        Some(format!(
            "The specified launch template, with template ID {deleted_id}, does not exist."
        )),
    ));
    rows.push((
        "an id the mint really issued, whose template the account still holds".to_string(),
        held_id.clone(),
        None,
    ));

    let zone = zone();
    for (at, (case, id, expected)) in rows.iter().enumerate() {
        let created_name = format!("mint-screen-group-{at}");
        let params = vec![
            p("AutoScalingGroupName", created_name.as_str()),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("DesiredCapacity", "0"),
            p("AvailabilityZones.member.1", zone.as_str()),
            p("LaunchTemplate.LaunchTemplateId", id.as_str()),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        // The same reference through the UPDATE, which resolves it the same way.
        let (update_status, update_body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", GROUP),
                ("LaunchTemplate.LaunchTemplateId", id.as_str()),
            ],
        )
        .await;

        match expected {
            Some(expected) => {
                for (operation, status, body) in [
                    ("CreateAutoScalingGroup", status, &body),
                    ("UpdateAutoScalingGroup", update_status, &update_body),
                ] {
                    assert_eq!(
                        status, 400,
                        "case {case:?}/{operation}: expected HTTP 400, got {status}: {body}"
                    );
                    assert_eq!(
                        tag_value(body, "Code"),
                        VALIDATION_ERROR,
                        "case {case:?}/{operation}: expected {VALIDATION_ERROR}, got {body}"
                    );
                    let answered = xml_unescape(&tag_value(body, "Message"));
                    assert_eq!(
                        &answered, expected,
                        "case {case:?}/{operation}: expected {expected:?}, got {answered:?}"
                    );
                }
                assert!(
                    !group_exists(&router, &created_name).await,
                    "case {case:?}: the refused create must leave no group"
                );
                let reported = reported_template(&router).await;
                assert_eq!(
                    reported.0.as_str(),
                    held_id.as_str(),
                    "case {case:?}: the refused update must leave the group on the template it \
                     resolved, got {reported:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}/CreateAutoScalingGroup: an issued id the account holds must \
                     be resolved, got {status}: {body}"
                );
                assert_eq!(
                    update_status, 200,
                    "case {case:?}/UpdateAutoScalingGroup: an issued id the account holds must \
                     be resolved, got {update_status}: {update_body}"
                );
                assert!(
                    group_exists(&router, &created_name).await,
                    "case {case:?}: the accepted create must leave a group"
                );
            }
        }
    }
}
