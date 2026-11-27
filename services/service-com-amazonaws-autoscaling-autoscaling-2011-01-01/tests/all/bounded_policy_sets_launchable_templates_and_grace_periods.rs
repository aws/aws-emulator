// SPDX-License-Identifier: CC0-1.0

//! Three writes Amazon EC2 Auto Scaling REFUSES, each because the state the request would
//! leave is one the service does not hold.
//!
//! RULE 1: a scaling policy belongs to ONE Auto Scaling group, and the published quota
//! "Scaling policies per Auto Scaling group" bounds that group's set at 50 — so a put that
//! would CREATE the group's fifty-first policy is refused with HTTP 400 and the modelled
//! `LimitExceededFault` (`awsQueryError` code `LimitExceeded`,
//! `smithy_query(query="operation",operation="PutScalingPolicy")`), the group's set stays at
//! the ceiling, and a put that only UPDATES a policy the group already holds adds nothing to
//! that set and is served. The bound is the GROUP's, so another group of the same account
//! still takes 50 of its own, and freeing one slot lets the refused put through.
//!
//! RULE 2: a group launches its instances from the launch-template VERSION its specification
//! names, and Amazon EC2 Auto Scaling reads that version's launch parameters when the group is
//! configured — "An AMI ID is required to launch an instance"
//! (`smithy_query(query="doc",shape="RunInstancesRequest",member="ImageId")`) — so a request
//! pointing a group at a version that states no image is refused with HTTP 400
//! `ValidationError`, wherever the specification is carried and whichever request carries it,
//! and the group stays on the version it already resolved.
//!
//! RULE 3: "If the Auto Scaling group of the specified instance has a `HealthCheckGracePeriod`
//! specified for the group, by default, this call respects the grace period. Set this to
//! `False`, to have the call not respect the grace period associated with the group"
//! (`smithy_query(query="doc",shape="SetInstanceHealthQuery",member="ShouldRespectGracePeriod")`)
//! — respecting the period means the call cannot condemn an instance the period still covers:
//! `SetInstanceHealth` recording `Unhealthy` for such an instance is refused with HTTP 400
//! `ValidationError`, nothing is written and the instance is not replaced, while the same call
//! with the flag off, one for an instance already past the period, and one recording `Healthy`
//! are all served.
//!
//! Each table spans what its rule generalises over — which policy type the put names and
//! whether it creates or updates, where the launch-template specification is carried and which
//! request carries it, and the period the group states beside the flag and the status the call
//! records — rather than one recorded request. Every wait below looks AGAIN until its
//! condition holds or a generous deadline passes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{
    borrowed, ec2_query, emulator, p, query, seed_subnets, tag_value, tag_values, xml_unescape,
};

/// The two error codes this suite asserts: the `awsQueryError` code of the modelled fault a
/// quota refusal carries, and the code this service answers a request it cannot satisfy with.
const LIMIT_EXCEEDED: &str = "LimitExceeded";
const VALIDATION_ERROR: &str = "ValidationError";

/// The published quota on ONE group's scaling policies — "Scaling policies per Auto Scaling
/// group", whose value is 50 — written out here rather than taken from the crate, so the
/// assertion states the bound itself.
const POLICIES_PER_GROUP: usize = 50;

/// The sentence a put past that quota is refused with: fixed text naming the bound and nothing
/// of the request.
const POLICY_LIMIT_MESSAGE: &str = "The maximum limit of 50 policies has been reached.";

/// The sentence a request pointing a group at a launch-template version that states no image
/// is refused with: this service's own preamble for a launch source it cannot use, followed by
/// Amazon EC2's own wording for the parameter the version left out.
const INCOMPLETE_TEMPLATE_MESSAGE: &str = "You must use a valid fully-formed launch template. \
                                           The request must contain the parameter ImageId";

/// The instance type every launch template here carries, and the processor architecture that
/// type runs — the architecture the image a COMPLETE template states has to be built for, since
/// Amazon EC2 launches no instance of a type its AMI does not match.
const INSTANCE_TYPE: &str = "t3.micro";
const INSTANCE_ARCHITECTURE: &str = "x86_64";

/// The image a COMPLETE launch template of this suite states, resolved against Amazon EC2's OWN
/// catalog rather than written out here: the first image Amazon publishes for the architecture
/// the instance type above runs, as `DescribeImages` reports it. An id a case invented would
/// name no image of the account, and an instance can only be launched from an AMI built for the
/// architecture of its type.
async fn published_image_id(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "DescribeImages",
        &[
            p("Filter.1.Name", "owner-alias"),
            p("Filter.1.Value.1", "amazon"),
            p("Filter.2.Name", "architecture"),
            p("Filter.2.Value.1", INSTANCE_ARCHITECTURE),
            p("Filter.3.Name", "state"),
            p("Filter.3.Value.1", "available"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeImages expected 200, got {status}: {body}"
    );
    tag_values(&body, "imageId")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the account must publish an {INSTANCE_ARCHITECTURE} image, got \
                 {body}"
            )
        })
}

/// The `LifecycleState` of a member that is in service, and the two `HealthStatus` values
/// `SetInstanceHealth` documents — written out here rather than taken from the crate.
const IN_SERVICE: &str = "InService";
const HEALTHY: &str = "Healthy";
const UNHEALTHY: &str = "Unhealthy";

/// How long a case waits for a move the clock owes before calling it a failure. A BACKSTOP,
/// not a wait: every helper below looks again until its condition holds.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long one look waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// How long a case that asserts the health check has NOT acted keeps looking: several times
/// the span this emulator gives that check, derived from the service's own span so the case and
/// the implementation cannot drift apart.
fn held_off_window() -> Duration {
    autoscaling::state::UNHEALTHY_INSTANCE_REPLACEMENT * 2
}

// ---------------------------------------------------------------------------
// RULE 1: the policies one group holds
// ---------------------------------------------------------------------------

/// Create one group that launches nothing, so a case about its POLICIES waits for no instance.
async fn create_empty_group(router: &MultiServiceRouter, case: &str, name: &str, template: &str) {
    let params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", template),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "0"),
        p("AvailabilityZones.member.1", first_zone()),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
}

/// The first Availability Zone of the Region, from Amazon EC2's own catalog rather than written
/// out here.
fn first_zone() -> String {
    service_com_amazonaws_ec2_amazonec2::state::availability_zones_in_region(
        crate::support::REGION,
    )
    .first()
    .map(|zone| zone.zone_name.clone())
    .expect("the Region has Availability Zones")
}

/// Put one scaling policy of `policy_type` on `group`, and answer the call's status and body.
async fn put_policy(
    router: &MultiServiceRouter,
    group: &str,
    policy_name: &str,
    policy_type: &str,
    scaling_adjustment: i32,
) -> (u16, String) {
    let mut params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", policy_name),
        p("PolicyType", policy_type),
        p("AdjustmentType", "ChangeInCapacity"),
    ];
    if policy_type == "StepScaling" {
        params.push(p("MetricAggregationType", "Average"));
        params.push(p(
            "StepAdjustments.member.1.ScalingAdjustment",
            scaling_adjustment.to_string(),
        ));
        params.push(p(
            "StepAdjustments.member.1.MetricIntervalLowerBound",
            "0",
        ));
    } else {
        params.push(p("ScalingAdjustment", scaling_adjustment.to_string()));
    }
    query(router, "PutScalingPolicy", &borrowed(&params)).await
}

/// The policy names `DescribePolicies` reports for `group`.
async fn reported_policies(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribePolicies",
        &[("AutoScalingGroupName", group), ("MaxRecords", "100")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribePolicies for {group} expected 200, got {status}: {body}"
    );
    tag_values(&body, "PolicyName")
}

/// The `ScalingAdjustment` the group reports for the policy `policy_name`.
async fn reported_adjustment(router: &MultiServiceRouter, group: &str, policy_name: &str) -> String {
    let (status, body) = query(
        router,
        "DescribePolicies",
        &[
            ("AutoScalingGroupName", group),
            ("PolicyNames.member.1", policy_name),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribePolicies for {policy_name} expected 200, got {status}: {body}"
    );
    tag_value(&body, "ScalingAdjustment")
}

/// Assert the call `(status, body)` was refused with the quota fault.
fn assert_quota_refusal(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a put past the group's policy quota expected 400, got {status}: {body}"
    );
    assert_eq!(
        tag_value(body, "Code"),
        LIMIT_EXCEEDED,
        "case {case:?}: expected the modelled fault's code {LIMIT_EXCEEDED}, got {body}"
    );
    assert_eq!(
        xml_unescape(&tag_value(body, "Message")),
        POLICY_LIMIT_MESSAGE,
        "case {case:?}: expected {POLICY_LIMIT_MESSAGE:?}, got {body}"
    );
}

/// One row of the quota table: the put made against a group already holding its 50 policies.
struct QuotaRow {
    case: &'static str,
    /// The policy the put names — one the group already holds, or a name new to it.
    policy_name: &'static str,
    policy_type: &'static str,
    /// Whether the put must be refused.
    refused: bool,
}

/// RULE 1 (see the module docs): the fifty-first policy of a group is refused, the quota is the
/// GROUP's, and a put that updates a policy already there is served at the ceiling.
#[tokio::test]
async fn the_policy_quota_of_a_group_refuses_the_policy_past_it() {
    let case = "a group filled to its policy quota";
    let (router, _engine) = emulator();
    let group = "quota-group";
    let other_group = "quota-other-group";
    create_empty_group(&router, case, group, "lt-1").await;
    create_empty_group(&router, case, other_group, "lt-1").await;

    // The group is filled to the ceiling through its own API, one put per policy.
    for index in 0..POLICIES_PER_GROUP {
        let name = format!("filler-{index}");
        let (status, body) = put_policy(&router, group, &name, "SimpleScaling", 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutScalingPolicy {name} expected 200, got {status}: {body}"
        );
    }
    let held = reported_policies(&router, group).await;
    assert_eq!(
        held.len(),
        POLICIES_PER_GROUP,
        "case {case:?}: the group must hold its {POLICIES_PER_GROUP} policies, got {held:?}"
    );

    // The rows differ in what the put names at the ceiling: a new policy, a new policy of
    // another type, and one the group already holds.
    let rows = [
        QuotaRow {
            case: "a new simple scaling policy at the ceiling",
            policy_name: "past-the-ceiling",
            policy_type: "SimpleScaling",
            refused: true,
        },
        QuotaRow {
            case: "a new step scaling policy at the ceiling",
            policy_name: "past-the-ceiling-step",
            policy_type: "StepScaling",
            refused: true,
        },
        QuotaRow {
            case: "a put that updates a policy the group already holds",
            policy_name: "filler-0",
            policy_type: "SimpleScaling",
            refused: false,
        },
    ];

    for row in &rows {
        let (status, body) = put_policy(&router, group, row.policy_name, row.policy_type, 7).await;
        if row.refused {
            assert_quota_refusal(row.case, status, &body);
        } else {
            assert_eq!(
                status, 200,
                "case {:?}: a put that creates nothing new must be served at the ceiling, got \
                 {status}: {body}",
                row.case
            );
            let adjustment = reported_adjustment(&router, group, row.policy_name).await;
            assert_eq!(
                adjustment, "7",
                "case {:?}: the served put must have updated the policy, got {adjustment:?}",
                row.case
            );
        }
        let held = reported_policies(&router, group).await;
        assert_eq!(
            held.len(),
            POLICIES_PER_GROUP,
            "case {:?}: the group's set stays at {POLICIES_PER_GROUP}, got {}",
            row.case,
            held.len()
        );
        assert!(
            !row.refused || !held.iter().any(|name| name == row.policy_name),
            "case {:?}: a refused put stores no policy, got {held:?}",
            row.case
        );
    }

    // The quota is the GROUP's: another group of the same account takes a policy of its own,
    // under the very name the full group refused.
    let (status, body) = put_policy(&router, other_group, "past-the-ceiling", "SimpleScaling", 1)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: another group of the account must take its own policy, got {status}: \
         {body}"
    );
    let held_elsewhere = reported_policies(&router, other_group).await;
    assert_eq!(
        held_elsewhere,
        vec!["past-the-ceiling".to_string()],
        "case {case:?}: the other group holds exactly its own policy, got {held_elsewhere:?}"
    );

    // Freeing one slot lets the refused put through.
    let (status, body) = query(
        &router,
        "DeletePolicy",
        &[
            ("AutoScalingGroupName", group),
            ("PolicyName", "filler-0"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeletePolicy expected 200, got {status}: {body}"
    );
    let (status, body) = put_policy(&router, group, "past-the-ceiling", "SimpleScaling", 1).await;
    assert_eq!(
        status, 200,
        "case {case:?}: with a slot free the same put must be served, got {status}: {body}"
    );
    let held = reported_policies(&router, group).await;
    assert_eq!(
        held.len(),
        POLICIES_PER_GROUP,
        "case {case:?}: the group is back at {POLICIES_PER_GROUP} policies, got {held:?}"
    );
    assert!(
        held.iter().any(|name| name == "past-the-ceiling"),
        "case {case:?}: the policy the freed slot took must be reported, got {held:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the launch parameters the version a group points at states
// ---------------------------------------------------------------------------

/// Create one launch template over Amazon EC2's own wire, with the image only when `image` says
/// so, and answer the id that service assigned it.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    image: Option<&str>,
) -> String {
    let mut params = vec![
        p("LaunchTemplateName", name),
        p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
    ];
    if let Some(image_id) = image {
        params.push(p("LaunchTemplateData.ImageId", image_id));
    }
    let (status, body) = ec2_query(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    assert!(
        !id.is_empty(),
        "case {case:?}: CreateLaunchTemplate must report the template's id, got {body}"
    );
    id
}

/// Mint one more version of the launch template `name` over Amazon EC2's own wire, with the
/// image only when `image` says so, and answer the version number that service assigned it.
async fn create_template_version(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    image: Option<&str>,
) -> String {
    let mut params = vec![
        p("LaunchTemplateName", name),
        p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
    ];
    if let Some(image_id) = image {
        params.push(p("LaunchTemplateData.ImageId", image_id));
    }
    let (status, body) =
        ec2_query(router, "CreateLaunchTemplateVersion", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplateVersion for {name} expected 200, got {status}: \
         {body}"
    );
    let version = tag_value(&body, "versionNumber");
    assert!(
        !version.is_empty(),
        "case {case:?}: CreateLaunchTemplateVersion must report the version it minted, got \
         {body}"
    );
    version
}

/// Where the row carries the launch-template specification it points the group at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// The group's own `LaunchTemplate`.
    Own,
    /// The launch template of a `MixedInstancesPolicy`.
    PolicyTemplate,
    /// The template an OVERRIDE of that policy names.
    PolicyOverride,
}

/// Which request carries the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Create,
    Update,
}

/// One row of the launch-parameter table.
struct TemplateRow {
    case: &'static str,
    request: Request,
    carried: Carried,
    /// Whether the specification names the template's SECOND version, which states no image
    /// where its first one does.
    by_version: bool,
    /// Whether the request must be refused.
    refused: bool,
}

/// The template every group of rule 2 starts on, and the one a row points it at.
const COMPLETE_TEMPLATE: &str = "image-complete-template";
const IMAGE_LESS_TEMPLATE: &str = "image-less-template";
const VERSIONED_TEMPLATE: &str = "image-versioned-template";

/// The parameter prefix the specification's members are carried under.
fn template_prefix(carried: Carried) -> &'static str {
    match carried {
        Carried::Own => "LaunchTemplate",
        Carried::PolicyTemplate => "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification",
        Carried::PolicyOverride => {
            "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.LaunchTemplateSpecification"
        }
    }
}

/// RULE 2 (see the module docs): a group is never pointed at a launch-template version that
/// states no image, and a refused request leaves the group where it was.
#[tokio::test]
async fn a_launch_template_version_that_states_no_image_is_refused() {
    let rows = [
        TemplateRow {
            case: "an update onto a template whose only version states no image",
            request: Request::Update,
            carried: Carried::Own,
            by_version: false,
            refused: true,
        },
        TemplateRow {
            case: "a create from a template whose only version states no image",
            request: Request::Create,
            carried: Carried::Own,
            by_version: false,
            refused: true,
        },
        TemplateRow {
            case: "an update whose mixed instances policy names the image-less template",
            request: Request::Update,
            carried: Carried::PolicyTemplate,
            by_version: false,
            refused: true,
        },
        TemplateRow {
            case: "a create whose policy override names the image-less template",
            request: Request::Create,
            carried: Carried::PolicyOverride,
            by_version: false,
            refused: true,
        },
        TemplateRow {
            case: "an update naming the one version of a template that states no image",
            request: Request::Update,
            carried: Carried::Own,
            by_version: true,
            refused: true,
        },
        TemplateRow {
            case: "an update onto a template whose version states the image",
            request: Request::Update,
            carried: Carried::Own,
            by_version: false,
            refused: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (router, _engine) = emulator();
        let image_id = published_image_id(&router, case).await;
        let complete_id =
            create_template(&router, case, COMPLETE_TEMPLATE, Some(&image_id)).await;
        create_template(&router, case, IMAGE_LESS_TEMPLATE, None).await;
        // A template whose FIRST version states the image and whose second does not, so a row
        // that names the second one pins the VERSION the group is pointed at rather than the
        // template.
        create_template(&router, case, VERSIONED_TEMPLATE, Some(&image_id)).await;
        let second_version =
            create_template_version(&router, case, VERSIONED_TEMPLATE, None).await;

        let group = format!("image-rule-group-{index}");
        create_empty_group(&router, case, &group, COMPLETE_TEMPLATE).await;

        // What the row points the group at: the image-less template, the second version of the
        // versioned one, or the complete template the group already launches from.
        let (template_name, version) = match (row.refused, row.by_version) {
            (true, false) => (IMAGE_LESS_TEMPLATE, None),
            (true, true) => (VERSIONED_TEMPLATE, Some(second_version.as_str())),
            (false, _) => (COMPLETE_TEMPLATE, None),
        };
        let prefix = template_prefix(row.carried);
        let created_name = format!("{group}-created");
        let mut params = match row.request {
            Request::Create => vec![
                p("AutoScalingGroupName", created_name.as_str()),
                p("MinSize", "0"),
                p("MaxSize", "1"),
                p("DesiredCapacity", "0"),
                p("AvailabilityZones.member.1", first_zone()),
            ],
            Request::Update => vec![p("AutoScalingGroupName", group.as_str())],
        };
        params.push(p(
            &format!("{prefix}.LaunchTemplateName"),
            template_name,
        ));
        if let Some(version) = version {
            params.push(p(&format!("{prefix}.Version"), version));
        }
        if row.carried == Carried::PolicyOverride {
            // An override names the instance type it overrides, beside the template it launches
            // that type from, and the policy carries a launch template of its own.
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                 LaunchTemplateName",
                COMPLETE_TEMPLATE,
            ));
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                INSTANCE_TYPE,
            ));
        }
        let action = match row.request {
            Request::Create => "CreateAutoScalingGroup",
            Request::Update => "UpdateAutoScalingGroup",
        };
        let (status, body) = query(&router, action, &borrowed(&params)).await;

        if row.refused {
            assert_eq!(
                status, 400,
                "case {case:?}: {action} onto a version with no image expected 400, got \
                 {status}: {body}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                VALIDATION_ERROR,
                "case {case:?}: expected {VALIDATION_ERROR}, got {body}"
            );
            let answered = xml_unescape(&tag_value(&body, "Message"));
            assert_eq!(
                answered, INCOMPLETE_TEMPLATE_MESSAGE,
                "case {case:?}: expected {INCOMPLETE_TEMPLATE_MESSAGE:?}, got {answered:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: {action} onto a complete version expected 200, got {status}: \
                 {body}"
            );
        }

        // Nothing the refused request asked for was stored: the group still launches from the
        // template and version it resolved, and a refused create left no group.
        let (reported_id, reported_version) = reported_template(&router, &group).await;
        assert_eq!(
            reported_id, complete_id,
            "case {case:?}: the group must still point at the template it resolved, got \
             {reported_id:?}"
        );
        assert!(
            reported_version.is_empty() || reported_version != second_version,
            "case {case:?}: the group must not have been rebased onto the image-less version, \
             got {reported_version:?}"
        );
        if row.request == Request::Create {
            let exists = group_exists(&router, &created_name).await;
            assert_eq!(
                exists, !row.refused,
                "case {case:?}: a refused create must leave no group, got exists={exists}"
            );
        }
    }
}

/// Create one launch template over Amazon EC2's own wire, naming the image and the instance
/// type given — the launch parameters a version has to state for an instance to be launched
/// from it.
async fn create_typed_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    image_id: &str,
    instance_type: &str,
) {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id),
            p("LaunchTemplateData.InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
}

/// RULE: an instance is only ever placed as an instance type the service that owns them
/// PUBLISHES, so a group pointed at a launch-template version naming a type that does not
/// exist could hold no capacity and the request is refused — this service's own preamble for a
/// launch source it cannot use, followed by that service's own sentence for the value. A
/// refused request leaves the group on the version it was already running.
///
/// The rows span what the rule generalises over: WHICH request configures the group (the
/// create, the update) and the FORM of the type that does not exist (a published family at a
/// size it does not offer, a family nothing publishes), beside a published type that is served
/// — so a rule refusing too much could not pass either.
#[tokio::test]
async fn a_launch_template_version_naming_an_instance_type_that_does_not_exist_is_refused() {
    // (case, the instance type the template states, whether it names a published type)
    let rows: &[(&str, &str, bool)] = &[
        ("a published family at a size it does not offer", "t3.bogus", false),
        ("a family nothing publishes at all", "zz9.plural", false),
        ("a published instance type", INSTANCE_TYPE, true),
    ];

    for (index, (case, instance_type, published)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let image_id = published_image_id(&router, case).await;
        create_template(&router, case, COMPLETE_TEMPLATE, Some(&image_id)).await;
        let template = format!("instance-type-rule-template-{index}");
        create_typed_template(&router, case, &template, &image_id, instance_type).await;

        let group = format!("instance-type-rule-group-{index}");
        create_empty_group(&router, case, &group, COMPLETE_TEMPLATE).await;
        let created_name = format!("{group}-created");

        // The same version through both writers of a group's launch source.
        let create = vec![
            p("AutoScalingGroupName", created_name.as_str()),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("DesiredCapacity", "0"),
            p("AvailabilityZones.member.1", first_zone()),
            p("LaunchTemplate.LaunchTemplateName", template.as_str()),
        ];
        let (create_status, create_body) =
            query(&router, "CreateAutoScalingGroup", &borrowed(&create)).await;
        let update = vec![
            p("AutoScalingGroupName", group.as_str()),
            p("LaunchTemplate.LaunchTemplateName", template.as_str()),
        ];
        let (update_status, update_body) =
            query(&router, "UpdateAutoScalingGroup", &borrowed(&update)).await;

        if *published {
            assert_eq!(
                create_status, 200,
                "case {case:?}: a published type must be accepted by the create, got \
                 {create_status}: {create_body}"
            );
            assert_eq!(
                update_status, 200,
                "case {case:?}: a published type must be accepted by the update, got \
                 {update_status}: {update_body}"
            );
            assert!(
                group_exists(&router, &created_name).await,
                "case {case:?}: the accepted create must leave a group"
            );
            continue;
        }
        // The sentence is written out here rather than taken from the implementation: this
        // service's own preamble for a launch source it cannot use, then the owning service's
        // sentence for the value, whose one variable part is the type the template stated.
        let expected = format!(
            "You must use a valid fully-formed launch template. Invalid value \
             '{instance_type}' for InstanceType."
        );
        for (action, status, body) in [
            ("CreateAutoScalingGroup", create_status, &create_body),
            ("UpdateAutoScalingGroup", update_status, &update_body),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?}/{action}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                tag_value(body, "Code"),
                VALIDATION_ERROR,
                "case {case:?}/{action}: expected {VALIDATION_ERROR}, got {body}"
            );
            let answered = xml_unescape(&tag_value(body, "Message"));
            assert_eq!(
                answered, expected,
                "case {case:?}/{action}: expected {expected:?}, got {answered:?}"
            );
        }
        assert!(
            !group_exists(&router, &created_name).await,
            "case {case:?}: the refused create must leave no group"
        );
    }
}

/// The `(LaunchTemplateId, Version)` pair the group `name` reports.
async fn reported_template(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    (
        tag_value(&body, "LaunchTemplateId"),
        tag_value(&body, "Version"),
    )
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
        "DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    !tag_values(&body, "AutoScalingGroupName").is_empty()
}

// ---------------------------------------------------------------------------
// RULE 3: the grace period a respected call cannot condemn an instance inside
// ---------------------------------------------------------------------------

/// One row of the grace-period table.
struct GraceRow {
    case: &'static str,
    /// The `HealthCheckGracePeriod` the group states, or `None` for a group stating none.
    grace_period: Option<i32>,
    /// `ShouldRespectGracePeriod` as the request states it, or `None` for a request that states
    /// nothing — which the member documents as respecting the period.
    respect: Option<bool>,
    /// The `HealthStatus` the call records.
    health_status: &'static str,
    /// Whether the call must be refused.
    refused: bool,
}

/// RULE 3 (see the module docs): a respected grace period refuses the unhealthy verdict and
/// nothing else is held back.
#[tokio::test]
async fn an_unhealthy_status_inside_a_respected_grace_period_is_refused() {
    // A period long enough that an instance of these groups is inside it for the whole of a
    // case, and the shortest one the member admits, which covers no instance at all.
    const LONG_PERIOD: i32 = 900;
    const NO_SECONDS: i32 = 0;

    let rows = [
        GraceRow {
            case: "unhealthy with the period respected by default",
            grace_period: Some(LONG_PERIOD),
            respect: None,
            health_status: UNHEALTHY,
            refused: true,
        },
        GraceRow {
            case: "unhealthy with the period respected outright",
            grace_period: Some(LONG_PERIOD),
            respect: Some(true),
            health_status: UNHEALTHY,
            refused: true,
        },
        GraceRow {
            case: "unhealthy with the period not respected",
            grace_period: Some(LONG_PERIOD),
            respect: Some(false),
            health_status: UNHEALTHY,
            refused: false,
        },
        GraceRow {
            case: "healthy inside the same respected period",
            grace_period: Some(LONG_PERIOD),
            respect: Some(true),
            health_status: HEALTHY,
            refused: false,
        },
        GraceRow {
            case: "unhealthy under a period of no seconds, which covers nothing",
            grace_period: Some(NO_SECONDS),
            respect: Some(true),
            health_status: UNHEALTHY,
            refused: false,
        },
        GraceRow {
            case: "unhealthy on a group that states no period at all",
            grace_period: None,
            respect: Some(true),
            health_status: UNHEALTHY,
            refused: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (router, engine) = emulator();
        let group = format!("grace-group-{index}");
        let instance_id = group_holding_one(&router, &engine, case, &group, row.grace_period).await;

        let mut params = vec![
            p("InstanceId", instance_id.as_str()),
            p("HealthStatus", row.health_status),
        ];
        if let Some(respect) = row.respect {
            params.push(p("ShouldRespectGracePeriod", respect.to_string()));
        }
        let (status, body) = query(&router, "SetInstanceHealth", &borrowed(&params)).await;

        if row.refused {
            let grace = row
                .grace_period
                .expect("a refused row states the period it is refused under");
            assert_eq!(
                status, 400,
                "case {case:?}: SetInstanceHealth inside the period expected 400, got {status}: \
                 {body}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                VALIDATION_ERROR,
                "case {case:?}: expected {VALIDATION_ERROR}, got {body}"
            );
            // The sentence weighs the instance's age against the group's configured period:
            // the age is what the service measured against its own clock, so the assertion
            // pins the fixed text and the period, and reads the age back out of it.
            let answered = xml_unescape(&tag_value(&body, "Message"));
            let tail = format!(" seconds) is less than the configured grace period of {grace} seconds");
            let age = answered
                .strip_prefix("The age of the instance (")
                .and_then(|rest| rest.strip_suffix(&tail))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the refusal to weigh the instance's age \
                         against the period {grace}, got {answered:?}"
                    )
                });
            let age: u64 = age.parse().unwrap_or_else(|_| {
                panic!("case {case:?}: the age must be a count of seconds, got {answered:?}")
            });
            assert!(
                age < u64::try_from(grace).expect("a period that fits in a u64"),
                "case {case:?}: the refusal must state an age inside the period, got {age} \
                 against {grace}"
            );

            // Nothing was written: the member goes on running, healthy, over a window several
            // times the span the check takes once it is owed.
            let until = Instant::now() + held_off_window();
            loop {
                let (state, health) = member_of(&router, case, &group, &instance_id).await;
                assert_eq!(
                    state, IN_SERVICE,
                    "case {case:?}: a refused request moves no member, got {state:?}/{health:?}"
                );
                assert_eq!(
                    health, HEALTHY,
                    "case {case:?}: a refused request records no status, got \
                     {state:?}/{health:?}"
                );
                if Instant::now() >= until {
                    break;
                }
                tokio::time::sleep(LOOK_AGAIN).await;
            }
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: SetInstanceHealth expected 200, got {status}: {body}"
            );
            if row.health_status == UNHEALTHY {
                // A served unhealthy verdict is one the group ACTS on: the member it condemned
                // leaves and the group holds a replacement in service.
                let replacement = replaced_member(&router, case, &group, &instance_id).await;
                assert_ne!(
                    replacement, instance_id,
                    "case {case:?}: the condemned member must be replaced"
                );
            } else {
                let (state, health) = member_of(&router, case, &group, &instance_id).await;
                assert_eq!(
                    (state.as_str(), health.as_str()),
                    (IN_SERVICE, HEALTHY),
                    "case {case:?}: a healthy status leaves the member in service"
                );
            }
        }
    }
}

/// The refusal stands only while the period COVERS the instance: the very same call, repeated,
/// is served once the period has elapsed for it, and the verdict is then acted on.
///
/// The period here is short because the case is about it ELAPSING; the deadline the loop looks
/// under is generous, and nothing waits for a fixed span.
#[tokio::test]
async fn the_same_unhealthy_call_is_served_once_the_period_has_elapsed() {
    const SHORT_PERIOD: i32 = 2;
    let case = "a period that elapses under the caller";
    let (router, engine) = emulator();
    let group = "grace-elapsing-group";
    let instance_id = group_holding_one(&router, &engine, case, group, Some(SHORT_PERIOD)).await;

    let params = vec![
        p("InstanceId", instance_id.as_str()),
        p("HealthStatus", UNHEALTHY),
        p("ShouldRespectGracePeriod", "true"),
    ];
    let deadline = Instant::now() + DEADLINE;
    let mut refusals = 0usize;
    loop {
        let (status, body) = query(&router, "SetInstanceHealth", &borrowed(&params)).await;
        if status == 200 {
            break;
        }
        refusals += 1;
        assert_eq!(
            status, 400,
            "case {case:?}: a call inside the period is refused with HTTP 400, got {status}: \
             {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            VALIDATION_ERROR,
            "case {case:?}: expected {VALIDATION_ERROR}, got {body}"
        );
        // Every refused call wrote nothing, so the member is still the group's own, in service.
        let (state, health) = member_of(&router, case, group, &instance_id).await;
        assert_eq!(
            (state.as_str(), health.as_str()),
            (IN_SERVICE, HEALTHY),
            "case {case:?}: a refused call leaves the member where it was"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the period of {SHORT_PERIOD} seconds must elapse and the call must \
             then be served; {refusals} refusals so far"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert!(
        refusals > 0,
        "case {case:?}: the first call must have been refused inside the period"
    );
    let replacement = replaced_member(&router, case, group, &instance_id).await;
    assert_ne!(
        replacement, instance_id,
        "case {case:?}: the served verdict must have the member replaced"
    );
}

/// Create one group holding ONE instance, with the `HealthCheckGracePeriod` `grace_period`
/// names, and answer that instance's id once the group reports it in service.
async fn group_holding_one(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    grace_period: Option<i32>,
) -> String {
    let subnets = seed_subnets(engine, 1);
    let template = format!("lt-{name}");
    create_template(router, case, &template, Some(&published_image_id(router, case).await)).await;
    let grace = grace_period.map(|value| value.to_string());
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", template.as_str()),
        p("MinSize", "1"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "1"),
        p("VPCZoneIdentifier", subnets[0].id.as_str()),
        p("HealthCheckType", "EC2"),
    ];
    if let Some(grace) = grace.as_deref() {
        params.push(p("HealthCheckGracePeriod", grace));
    }
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        let members = reported_members(router, case, name).await;
        if let [(instance_id, state, _)] = members.as_slice() {
            if state == IN_SERVICE {
                return instance_id.clone();
            }
        }
        if Instant::now() >= deadline {
            // What the group RECORDED about the capacity it was asked for, so a group that
            // never reaches it says why rather than only that it did not.
            let (_, activities) = query(
                router,
                "DescribeScalingActivities",
                &[("AutoScalingGroupName", name)],
            )
            .await;
            panic!(
                "case {case:?}: the group {name} must hold one member in service, got \
                 {members:?}; its activities: {activities}"
            );
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every member the group `name` reports, as `(instance id, lifecycle state, health status)`.
async fn reported_members(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Vec<(String, String, String)> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    let instances = tag_value(&body, "Instances");
    instances
        .split("</member>")
        .filter(|chunk| chunk.contains("<InstanceId"))
        .map(|chunk| {
            (
                tag_value(chunk, "InstanceId"),
                tag_value(chunk, "LifecycleState"),
                tag_value(chunk, "HealthStatus"),
            )
        })
        .collect()
}

/// The `(lifecycle state, health status)` the group reports for `instance_id`.
async fn member_of(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    instance_id: &str,
) -> (String, String) {
    reported_members(router, case, name)
        .await
        .into_iter()
        .find(|(id, _, _)| id == instance_id)
        .map(|(_, state, health)| (state, health))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the group {name} must still report {instance_id}")
        })
}

/// Look again until the group has taken `condemned` out and holds another member in service,
/// and answer that member's id.
async fn replaced_member(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    condemned: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let members = reported_members(router, case, name).await;
        let serving = members
            .iter()
            .find(|(id, state, _)| id != condemned && state == IN_SERVICE)
            .map(|(id, _, _)| id.clone());
        let gone = !members.iter().any(|(id, _, _)| id == condemned);
        if let (true, Some(serving)) = (gone, serving) {
            return serving;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the group {name} must replace the condemned {condemned}, got \
             {members:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
