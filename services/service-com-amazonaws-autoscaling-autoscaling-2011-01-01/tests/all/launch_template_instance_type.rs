// SPDX-License-Identifier: CC0-1.0

//! WHERE the instance type a group's instances are launched as has to be stated, and what a
//! request that states it nowhere is refused with.
//!
//! RULE: an instance of an Auto Scaling group is launched as one instance TYPE, and a group has
//! two places to state it — the launch-template version it launches from (`InstanceType` of
//! that version's launch parameters) and the overrides of a mixed instances policy, which are
//! "the instance types (overrides) that are used to launch EC2 instances"
//! (`MixedInstancesPolicy$LaunchTemplate`). So a request that configures a group which is NOT a
//! mixed instances group against a launch-template version stating no instance type is refused
//! `InvalidQueryParameter`, whatever else the request carries, and nothing is stored; a request
//! whose policy overrides state the types the group spreads its capacity over is served even
//! when the version it launches from states none.
//!
//! That refusal is a different VERDICT from the one a name already taken gets: the code is what
//! a client branches on, so the launch source is read before the group's name is weighed —
//! `AlreadyExists` says "the group is there, adopt it or rename and retry", and a rename would
//! otherwise leave the caller with a group whose launch template can launch nothing.
//!
//! The rows span what the rule generalises over: WHERE the launch source names the template
//! (the group's own `LaunchTemplate`, the launch template of a `MixedInstancesPolicy`, the
//! template an override of that policy names), WHICH VERSION of a template the reference
//! resolves to (the default, a pinned number, the floating latest — the same template stating
//! the type in one version and not in another), HOW a mixed instances group states its types
//! (an override naming the type, an override stating `InstanceRequirements` instead, no
//! override at all), and WHICH operation configures the group (the create, the update).
//!
//! Auto Scaling and Amazon EC2 are mounted on ONE engine and driven over their own wire
//! protocols, the way the emulator serves them, so every launch template these requests resolve
//! against is one a client created, with the launch parameters that client stated.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, ec2_query, emulator, p, query, tag_value, tag_values, xml_unescape, SEEDED_IMAGE_ID,
};

/// The error code Amazon EC2 Auto Scaling answers a request whose arguments are each
/// well-formed but whose launch template cannot serve the group they would configure, and the
/// code a group name already taken is answered with (the modelled `AlreadyExistsFault`'s
/// `awsQueryError` code).
const INVALID_QUERY_PARAMETER: &str = "InvalidQueryParameter";
const ALREADY_EXISTS: &str = "AlreadyExists";

/// The sentence AWS answers a group whose launch-template version names no instance type while
/// the group is not a mixed instances group — written out here rather than taken from the
/// implementation, which would assert nothing. Entirely fixed text: it names neither the group
/// nor the template.
const MISSING_INSTANCE_TYPE: &str = "An instance type does not exist in the specified launch \
                                     template. An instance type must be defined in the launch \
                                     template while the Auto Scaling group is not configured \
                                     as a mixed instances group. Use a different launch \
                                     template that contains the InstanceType parameter, and \
                                     try again.";

/// The instance type the templates and overrides here state. Any type is a valid `InstanceType`
/// string (`LaunchTemplateOverrides$InstanceType`, `XmlStringMaxLen255`); which types exist is
/// not what this suite is about.
const INSTANCE_TYPE: &str = "t3.micro";

/// The launch template that states both launch parameters, the one that states only the image,
/// and the one whose FIRST version states only the image while its second adds the type.
const TYPED_TEMPLATE: &str = "lt-instance-type-typed";
const TYPELESS_TEMPLATE: &str = "lt-instance-type-typeless";
const VERSIONED_TEMPLATE: &str = "lt-instance-type-versioned";

/// The zone every group here places its instances in.
const ZONE: &str = "us-west-2a";

/// The launch templates the account holds, as Amazon EC2 reports them: the ids that service
/// minted, and the version of [`VERSIONED_TEMPLATE`] that states the instance type.
struct Templates {
    typeless_id: String,
    typed_id: String,
    versioned_typeless_version: String,
    versioned_typed_version: String,
}

/// Create one launch template over Amazon EC2's own wire and answer the id that service
/// assigned it and its latest version number.
async fn create_template(
    router: &MultiServiceRouter,
    name: &str,
    instance_type: Option<&str>,
) -> (String, String) {
    let mut params = vec![
        p("LaunchTemplateName", name),
        p("LaunchTemplateData.ImageId", SEEDED_IMAGE_ID),
    ];
    if let Some(instance_type) = instance_type {
        params.push(p("LaunchTemplateData.InstanceType", instance_type));
    }
    let (status, body) = ec2_query(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name:?} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    let version = tag_value(&body, "latestVersionNumber");
    assert!(
        !id.is_empty() && !version.is_empty(),
        "CreateLaunchTemplate for {name:?} must report the template's id and latest version, \
         got {body}"
    );
    (id, version)
}

/// Add one version to a launch template over Amazon EC2's own wire and answer its number.
async fn create_template_version(
    router: &MultiServiceRouter,
    name: &str,
    instance_type: &str,
) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", SEEDED_IMAGE_ID),
            p("LaunchTemplateData.InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplateVersion for {name:?} expected 200, got {status}: {body}"
    );
    let version = tag_value(&body, "versionNumber");
    assert!(
        !version.is_empty(),
        "CreateLaunchTemplateVersion must report the version's number, got {body}"
    );
    version
}

/// One emulator holding the three launch templates every row resolves against.
async fn seeded() -> (MultiServiceRouter, Templates) {
    let (router, _engine) = emulator();
    let (typeless_id, _) = create_template(&router, TYPELESS_TEMPLATE, None).await;
    let (typed_id, _) = create_template(&router, TYPED_TEMPLATE, Some(INSTANCE_TYPE)).await;
    let (_, versioned_typeless_version) = create_template(&router, VERSIONED_TEMPLATE, None).await;
    let versioned_typed_version =
        create_template_version(&router, VERSIONED_TEMPLATE, INSTANCE_TYPE).await;
    (
        router,
        Templates {
            typeless_id,
            typed_id,
            versioned_typeless_version,
            versioned_typed_version,
        },
    )
}

/// The launch source one row configures its group with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The group's own `LaunchTemplate`, by the NAME of the template stating no type.
    TypelessByName,
    /// Its own `LaunchTemplate`, by the ID of that template.
    TypelessById,
    /// Its own `LaunchTemplate`, pinned to the VERSION of a template that states no type.
    TypelessVersion,
    /// Its own `LaunchTemplate`, pinned to the version of that same template that states one.
    TypedVersion,
    /// Its own `LaunchTemplate`, floating on the latest version of that template, which states
    /// one: "`$Latest`" is the version a reference floating on the template's newest resolves
    /// to.
    LatestVersion,
    /// Its own `LaunchTemplate`, by the name of the template stating both parameters.
    Typed,
    /// The launch template of a `MixedInstancesPolicy`, stating no type, with NO override: a
    /// policy carrying none states no instance type either.
    PolicyTypelessNoOverride,
    /// That policy with one override naming the instance type outright.
    PolicyTypelessOverrideType,
    /// That policy with one override stating `InstanceRequirements` instead of a type.
    PolicyTypelessOverrideRequirements,
    /// A policy whose own launch template states the type and whose override names the
    /// type-less template to launch its own type from.
    PolicyTypedOverrideTypelessTemplate,
}

/// One row: the launch source, and whether the rule leaves the group configurable.
struct Row {
    case: &'static str,
    source: Source,
    /// `true` when the request states the instance type somewhere the group carries it.
    served: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "the group's own launch template, by name, stating no instance type",
        source: Source::TypelessByName,
        served: false,
    },
    Row {
        case: "the same template by the id Amazon EC2 minted for it",
        source: Source::TypelessById,
        served: false,
    },
    Row {
        case: "a version that states no instance type, of a template whose next one does",
        source: Source::TypelessVersion,
        served: false,
    },
    Row {
        case: "the version of that same template that states one",
        source: Source::TypedVersion,
        served: true,
    },
    Row {
        case: "that template floating on its latest version, which states one",
        source: Source::LatestVersion,
        served: true,
    },
    Row {
        case: "a template stating both launch parameters",
        source: Source::Typed,
        served: true,
    },
    Row {
        case: "a mixed instances policy with no override, on the type-less template",
        source: Source::PolicyTypelessNoOverride,
        served: false,
    },
    Row {
        case: "a mixed instances policy whose override names the instance type",
        source: Source::PolicyTypelessOverrideType,
        served: true,
    },
    Row {
        case: "a mixed instances policy whose override states instance requirements",
        source: Source::PolicyTypelessOverrideRequirements,
        served: true,
    },
    Row {
        case: "a policy whose override launches its type from the type-less template",
        source: Source::PolicyTypedOverrideTypelessTemplate,
        served: true,
    },
];

/// The launch-source parameters the row states.
fn launch_source(source: Source, templates: &Templates) -> Vec<(String, String)> {
    const POLICY_TEMPLATE: &str =
        "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName";
    const OVERRIDE: &str = "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1";
    match source {
        Source::TypelessByName => vec![p("LaunchTemplate.LaunchTemplateName", TYPELESS_TEMPLATE)],
        Source::TypelessById => vec![p(
            "LaunchTemplate.LaunchTemplateId",
            templates.typeless_id.as_str(),
        )],
        Source::TypelessVersion => vec![
            p("LaunchTemplate.LaunchTemplateName", VERSIONED_TEMPLATE),
            p(
                "LaunchTemplate.Version",
                templates.versioned_typeless_version.as_str(),
            ),
        ],
        Source::TypedVersion => vec![
            p("LaunchTemplate.LaunchTemplateName", VERSIONED_TEMPLATE),
            p(
                "LaunchTemplate.Version",
                templates.versioned_typed_version.as_str(),
            ),
        ],
        Source::LatestVersion => vec![
            p("LaunchTemplate.LaunchTemplateName", VERSIONED_TEMPLATE),
            p("LaunchTemplate.Version", "$Latest"),
        ],
        Source::Typed => vec![p(
            "LaunchTemplate.LaunchTemplateId",
            templates.typed_id.as_str(),
        )],
        Source::PolicyTypelessNoOverride => vec![p(POLICY_TEMPLATE, TYPELESS_TEMPLATE)],
        Source::PolicyTypelessOverrideType => vec![
            p(POLICY_TEMPLATE, TYPELESS_TEMPLATE),
            p(&format!("{OVERRIDE}.InstanceType"), INSTANCE_TYPE),
        ],
        Source::PolicyTypelessOverrideRequirements => vec![
            p(POLICY_TEMPLATE, TYPELESS_TEMPLATE),
            p(
                &format!("{OVERRIDE}.InstanceRequirements.VCpuCount.Min"),
                "2",
            ),
            p(
                &format!("{OVERRIDE}.InstanceRequirements.MemoryMiB.Min"),
                "1024",
            ),
        ],
        Source::PolicyTypedOverrideTypelessTemplate => vec![
            p(POLICY_TEMPLATE, TYPED_TEMPLATE),
            p(&format!("{OVERRIDE}.InstanceType"), INSTANCE_TYPE),
            p(
                &format!("{OVERRIDE}.LaunchTemplateSpecification.LaunchTemplateName"),
                TYPELESS_TEMPLATE,
            ),
        ],
    }
}

/// The maximum size the group holding a taken name is created with, and the different one every
/// second create states — so a refusal that still applied the request would show up as a
/// changed bound.
const STANDING_MAX_SIZE: &str = "1";
const SECOND_MAX_SIZE: &str = "5";

/// The create parameters one row makes, under `name`, with the maximum size given.
fn create_params(
    name: &str,
    source: Source,
    templates: &Templates,
    max_size: &str,
) -> Vec<(String, String)> {
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", max_size),
        p("AvailabilityZones.member.1", ZONE),
    ];
    params.extend(launch_source(source, templates));
    params
}

/// The `(code, message)` of a refusal, with the message unescaped as it was worded.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// Whether `DescribeAutoScalingGroups` reports a group under `name`.
async fn group_exists(router: &MultiServiceRouter, name: &str, case: &str) -> bool {
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
    !tag_values(&body, "AutoScalingGroupName").is_empty()
}

#[tokio::test]
async fn a_group_is_configured_only_where_the_instance_type_is_stated() {
    for row in ROWS {
        let case = row.case;
        let (router, templates) = seeded().await;
        let name = "asg-instance-type";
        let params = create_params(name, row.source, &templates, STANDING_MAX_SIZE);

        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: a launch source that states the instance type must be \
                 accepted, got {status}: {body}"
            );
            assert!(
                group_exists(&router, name, case).await,
                "case {case:?}: the accepted create must leave a group of that name behind"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a launch source that states no instance type must be refused, got \
             {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            (answered.0.as_str(), answered.1.as_str()),
            (INVALID_QUERY_PARAMETER, MISSING_INSTANCE_TYPE),
            "case {case:?}: expected the instance-type refusal, got {answered:?} with {body}"
        );
        assert!(
            !group_exists(&router, name, case).await,
            "case {case:?}: the refused create must leave no group of that name behind"
        );

        // AWS refused, so nothing of that name exists: the identical request is refused on the
        // SAME grounds rather than for a name it never took.
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        let retried = refusal(&body);
        assert_eq!(
            (status, retried.0.as_str(), retried.1.as_str()),
            (400, INVALID_QUERY_PARAMETER, MISSING_INSTANCE_TYPE),
            "case {case:?}: the retry must be refused the same way, got {status} {retried:?} \
             with {body}"
        );
    }
}

/// RULE: the launch source is read BEFORE the group's name is weighed against the names the
/// account has taken, so a request whose template states no instance type is refused for the
/// template even under a name that is already taken — and the group holding that name is left
/// exactly as it was.
#[tokio::test]
async fn the_instance_type_is_read_before_the_group_name_is_weighed() {
    // The rows that are refused, re-stated under a name a group already holds, beside the
    // contrast: the same name with a launch source that does state the type is what reaches the
    // name-uniqueness verdict.
    let rows: Vec<(&str, Source, &str, &str)> = ROWS
        .iter()
        .filter(|row| !row.served)
        .map(|row| {
            (
                row.case,
                row.source,
                INVALID_QUERY_PARAMETER,
                MISSING_INSTANCE_TYPE,
            )
        })
        .chain(std::iter::once((
            "a launch source that states the type: the name decides the answer",
            Source::Typed,
            ALREADY_EXISTS,
            "",
        )))
        .collect();

    for (case, source, expected_code, expected_message) in rows {
        let (router, templates) = seeded().await;
        let name = "asg-instance-type-taken";
        let standing = create_params(name, Source::Typed, &templates, STANDING_MAX_SIZE);
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&standing)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group that holds the name must be created, got {status}: {body}"
        );

        let params = create_params(name, source, &templates, SECOND_MAX_SIZE);
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second create under a taken name must be refused, got {status}: \
             {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            answered.0, expected_code,
            "case {case:?}: expected the {expected_code} code, got {answered:?} with {body}"
        );
        if !expected_message.is_empty() {
            assert_eq!(
                answered.1, expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {answered:?}"
            );
        }

        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups must answer 200, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "MaxSize"),
            STANDING_MAX_SIZE,
            "case {case:?}: the refused create must leave the standing group's bounds \
             untouched, got {body}"
        );
    }
}

/// RULE, of the other writer of a group's launch source: an update that REPLACES the launch
/// source is held to the same rule, against the configuration it leaves the group with — so
/// replacing the template of a plain group with one stating no type is refused and changes
/// nothing, while replacing the template of a mixed instances group whose overrides state the
/// types is served.
#[tokio::test]
async fn an_update_replaces_the_launch_source_only_where_the_instance_type_is_stated() {
    // Each row: the launch source the group is created with, the one the update states, and
    // whether the update is served.
    let rows: [(&str, Source, Source, bool); 3] = [
        (
            "a plain group pointed at a template stating no type",
            Source::Typed,
            Source::TypelessByName,
            false,
        ),
        (
            "a plain group pointed at another template that states one",
            Source::Typed,
            Source::TypedVersion,
            true,
        ),
        (
            "a mixed instances group whose overrides state the types another way",
            Source::PolicyTypelessOverrideType,
            Source::PolicyTypelessOverrideRequirements,
            true,
        ),
    ];

    for (case, created, replacement, served) in rows {
        let (router, templates) = seeded().await;
        let name = "asg-instance-type-update";
        let params = create_params(name, created, &templates, STANDING_MAX_SIZE);
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group the update drives must be created, got {status}: {body}"
        );

        let mut update = vec![p("AutoScalingGroupName", name)];
        update.extend(launch_source(replacement, &templates));
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&update)).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: an update whose configuration states the instance type must be \
                 served, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: an update leaving the group with no instance type must be refused, \
             got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            (answered.0.as_str(), answered.1.as_str()),
            (INVALID_QUERY_PARAMETER, MISSING_INSTANCE_TYPE),
            "case {case:?}: expected the instance-type refusal, got {answered:?} with {body}"
        );

        // The refused update changed nothing: the group still launches from the template it was
        // created with, read back through the operation a client reads it with.
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups must answer 200, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "LaunchTemplateName"),
            TYPED_TEMPLATE,
            "case {case:?}: the refused update must leave the group's launch template \
             untouched, got {body}"
        );
    }
}
