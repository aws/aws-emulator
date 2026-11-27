// SPDX-License-Identifier: CC0-1.0

//! What Amazon EC2 Auto Scaling REFUSES about the configuration of a group — driven end to
//! end over the wire this service speaks (`awsQuery`), with Amazon EC2 on the same engine so
//! the zones and launch templates under test are the account's own.
//!
//! THE RULES UNDER TEST, each applied to `CreateAutoScalingGroup` and
//! `UpdateAutoScalingGroup` alike, because the two write the same group:
//!
//! * a group is placed only in Availability Zones its Region HAS, by either spelling (the
//!   zone's name or its id);
//! * a group launches only from a launch template the account HOLDS, wherever the launch
//!   source names it, and only from a VERSION of it the template holds;
//! * an instance maintenance policy states a lower healthy threshold for every upper one,
//!   and the two are at most 100 apart;
//! * one instance type has at most one override in a mixed instances policy;
//! * a request names exactly ONE launch source, which is decided from the MEMBERS it
//!   carries before any of their values is looked up;
//! * a forced delete is refused while the group's `TerminateHookAbandon` retention trigger
//!   is set to retain;
//! * and a delete that races another delete of the same group is ACCEPTED, the group being
//!   gone either way.
//!
//! Every refused request leaves the account exactly as it was: the group is not created, or
//! keeps the configuration it had, which each test reads back through
//! `DescribeAutoScalingGroups`. Every sentence asserted here is spelled out rather than
//! imported, so a wrong constant in the implementation cannot make a test pass.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    borrowed, new_engine, p, query, router_with, seed_launch_templates, tag_value, tag_values,
    xml_unescape, REGION,
};

/// The launch template every group in this suite launches from, and a second one an update
/// moves a group onto.
const TEMPLATE: &str = "lt-configuration";
const OTHER_TEMPLATE: &str = "lt-configuration-next";

/// A launch template name, and a launch template id of the published form, that no template
/// of the account carries: a specification naming either resolves to nothing.
const ABSENT_TEMPLATE_NAME: &str = "lt-configuration-never-created";
const ABSENT_TEMPLATE_ID: &str = "lt-00000000000000999";

/// The instance types the override rows name. Any type is a valid `InstanceType` string
/// (`XmlStringMaxLen255`); which types exist is not what the duplicate rule is about.
const FIRST_TYPE: &str = "t3.micro";
const SECOND_TYPE: &str = "t3.small";

/// One row that names a VALUE under a member: the case, the member, the value, and whether
/// the request is served.
type ValueRow = (&'static str, &'static str, String, bool);

/// The same with a borrowed value, for an update whose values are all literals.
type UpdateValueRow = (&'static str, &'static str, &'static str, bool);

/// One row of the healthy-threshold table: the case, `MinHealthyPercentage`,
/// `MaxHealthyPercentage`, and the refusal the create answers beside the one the update
/// answers.
type ThresholdRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<(&'static str, &'static str)>,
);

/// The same for a refresh's preferences, which answer one sentence per rule.
type RefreshThresholdRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// One row of the override table: the case, the instance types the overrides name in order,
/// and whether the request is served.
type OverrideRow = (&'static str, &'static [&'static str], bool);

/// One row of the launch-source table: the case and the launch-source members the request
/// carries.
type LaunchSourceRow = (&'static str, Vec<(String, String)>);

/// One row of the retention table: the case, the trigger the group carries, and whether the
/// forced delete is served.
type RetentionRow = (&'static str, Option<&'static str>, bool);

/// One emulator with Auto Scaling and the account's own Amazon EC2 store behind it, and the
/// two launch templates this suite's groups launch from already in it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = new_engine();
    seed_launch_templates(&engine, &[TEMPLATE, OTHER_TEMPLATE]);
    (router_with(Arc::clone(&engine)), engine)
}

/// The Availability Zones Region [`REGION`] has, as Amazon EC2's own catalog gives them —
/// never written out here.
fn zones_of_the_region() -> Vec<ec2::AvailabilityZoneRecord> {
    let zones = ec2::availability_zones_in_region(REGION);
    assert!(
        !zones.is_empty(),
        "the Region {REGION} must have Availability Zones for these rows to name"
    );
    zones
}

/// The id Amazon EC2 assigned the launch template `name`, read from the account's own store.
fn held_template_id(engine: &Arc<engine::InMemoryEngine>, name: &str) -> String {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let store = state.lock();
    store
        .launch_template_id_for_name(name)
        .unwrap_or_else(|| panic!("the account must hold a launch template named {name:?}"))
}

/// The `<Message>` of an answered refusal, decoded the way a client reads it.
fn message(body: &str) -> String {
    xml_unescape(&tag_value(body, "Message"))
}

/// Whether the account holds a group under `name`, read back through the service's own
/// report.
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

/// The parameters of a create that is valid apart from what a row adds: a group of the
/// Region's first zone, launching from the template the account holds.
fn base_create(name: &str) -> Vec<(String, String)> {
    vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        p(
            "AvailabilityZones.member.1",
            zones_of_the_region()[0].zone_name.clone(),
        ),
    ]
}

/// Create the group `name` from [`base_create`] plus `extra`, and answer the status and body.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = base_create(name);
    params.extend_from_slice(extra);
    query(router, "CreateAutoScalingGroup", &borrowed(&params)).await
}

/// RULE: a group is placed only in Availability Zones its Region has, and a request naming
/// any other is refused with the zone it named — leaving no group.
#[tokio::test]
async fn a_group_is_placed_only_in_zones_the_region_has() {
    let zones = zones_of_the_region();
    let first = zones[0].clone();
    let last = zones.last().expect("a Region has zones").clone();
    // A letter one past the Region's own zones: the Region has as many as the catalog lists,
    // so the next letter names none of them.
    let past_the_last = format!(
        "{REGION}{}",
        char::from(b'a' + u8::try_from(zones.len()).unwrap_or(0))
    );
    // (case, the member the zone is named under, the value, whether the create is served)
    let rows: Vec<ValueRow> = vec![
        (
            "the Region's first zone, by name",
            "AvailabilityZones.member.1",
            first.zone_name.clone(),
            true,
        ),
        (
            "the Region's last zone, by name",
            "AvailabilityZones.member.1",
            last.zone_name.clone(),
            true,
        ),
        (
            "a zone letter past the ones the Region has",
            "AvailabilityZones.member.1",
            past_the_last.clone(),
            false,
        ),
        (
            "a zone of another Region",
            "AvailabilityZones.member.1",
            "eu-central-1a".to_string(),
            false,
        ),
        (
            "a zone id in the zone-name member",
            "AvailabilityZones.member.1",
            first.zone_id.clone(),
            false,
        ),
        (
            "a misspelled zone name",
            "AvailabilityZones.member.1",
            format!("{REGION}-a"),
            false,
        ),
    ];

    for (index, (case, member, value, served)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-zone-{index}");
        // A group named by zone id states no zone name, so the base create's own zone is
        // replaced rather than added to.
        let mut params: Vec<(String, String)> = base_create(&name)
            .into_iter()
            .filter(|(key, _)| key != "AvailabilityZones.member.1")
            .collect();
        params.push(p(member, value.clone()));
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: a zone the Region has must be accepted, got {status}: {body}"
            );
            assert!(
                group_exists(&router, &name).await,
                "case {case:?}: the accepted create must leave a group"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the zone to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            message(&body),
            format!(
                "You must use a valid fully-formed launch template. Invalid availability zone: \
                 [{value}]"
            ),
            "case {case:?}: expected AWS's sentence naming the zone the request sent"
        );
        assert!(
            !group_exists(&router, &name).await,
            "case {case:?}: the refused create must leave no group"
        );
    }
}

/// RULE: an update is taken against the same zone rule, by either spelling — the zone's name
/// or its id — and a refused update leaves the group in the zones it had.
#[tokio::test]
async fn an_update_cannot_move_a_group_into_a_zone_the_region_does_not_have() {
    let zones = zones_of_the_region();
    let first = zones[0].clone();
    // (case, the member the update names the zone under, the value, whether it is served)
    let rows: Vec<ValueRow> = vec![
        (
            "onto another zone of the Region, by name",
            "AvailabilityZones.member.1",
            zones
                .last()
                .expect("a Region has zones")
                .zone_name
                .clone(),
            true,
        ),
        (
            "onto a zone of no Region, by name",
            "AvailabilityZones.member.1",
            format!("{REGION}-nowhere"),
            false,
        ),
        (
            "onto a zone of the Region, by id",
            "AvailabilityZoneIds.member.1",
            first.zone_id.clone(),
            true,
        ),
        (
            "onto a zone id of another Region",
            "AvailabilityZoneIds.member.1",
            "euc1-az1".to_string(),
            false,
        ),
        (
            "onto a zone name put in the zone-id member",
            "AvailabilityZoneIds.member.1",
            first.zone_name.clone(),
            false,
        ),
    ];

    for (index, (case, member, value, served)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-zone-update-{index}");
        let (status, body) = create(&router, &name, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created, got {status}: {body}"
        );

        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                (member, value.as_str()),
            ],
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: a zone the Region has must be accepted, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            message(&body),
            format!(
                "You must use a valid fully-formed launch template. Invalid availability zone: \
                 [{value}]"
            ),
            "case {case:?}: expected AWS's sentence naming the zone the request sent"
        );
        let (_, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert!(
            described.contains(&first.zone_name),
            "case {case:?}: the refused update must leave the group in the zone it had, got \
             {described}"
        );
    }
}

/// RULE: a group launches only from a launch template the account HOLDS — wherever the launch
/// source names it, and by either half of the specification. A request naming one the account
/// does not hold is refused, naming the value it sent, and leaves no group.
#[tokio::test]
async fn a_group_launches_only_from_a_launch_template_the_account_holds() {
    let (_, engine) = emulator();
    let held_id = held_template_id(&engine, TEMPLATE);
    // (case, the member the template is named under, the value, whether the create is served)
    let rows: Vec<ValueRow> = vec![
        (
            "its own launch template, by a name the account holds",
            "LaunchTemplate.LaunchTemplateName",
            TEMPLATE.to_string(),
            true,
        ),
        (
            "its own launch template, by the id the account holds",
            "LaunchTemplate.LaunchTemplateId",
            held_id.clone(),
            true,
        ),
        (
            "its own launch template, by a name nothing holds",
            "LaunchTemplate.LaunchTemplateName",
            ABSENT_TEMPLATE_NAME.to_string(),
            false,
        ),
        (
            "its own launch template, by an id nothing holds",
            "LaunchTemplate.LaunchTemplateId",
            ABSENT_TEMPLATE_ID.to_string(),
            false,
        ),
        (
            "the launch template of a mixed instances policy, held",
            "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
            TEMPLATE.to_string(),
            true,
        ),
        (
            "the launch template of a mixed instances policy, absent",
            "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
            ABSENT_TEMPLATE_NAME.to_string(),
            false,
        ),
        (
            "the template an override of that policy names, absent",
            "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.\
             LaunchTemplateSpecification.LaunchTemplateId",
            ABSENT_TEMPLATE_ID.to_string(),
            false,
        ),
    ];

    for (index, (case, member, value, served)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-template-{index}");
        // Each row states its OWN launch source, so the base create's launch template is
        // replaced rather than added to — a request naming two launch sources is refused for
        // naming two.
        let mut params: Vec<(String, String)> = base_create(&name)
            .into_iter()
            .filter(|(key, _)| key != "LaunchTemplate.LaunchTemplateName")
            .collect();
        params.push(p(member, value.clone()));
        if member.contains("Overrides") {
            // An override names the instance type it overrides, beside the template it
            // launches that type from, and the policy carries a launch template of its own.
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                 LaunchTemplateName",
                TEMPLATE,
            ));
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                FIRST_TYPE,
            ));
        }
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: a template the account holds must be accepted, got {status}: \
                 {body}"
            );
            assert!(
                group_exists(&router, &name).await,
                "case {case:?}: the accepted create must leave a group"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the launch template to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        let answered = message(&body);
        // The sentence is Amazon EC2's own for the half the request stated, surfaced by Auto
        // Scaling. That service SCREENS an id against its own mint before it looks one up, so
        // an id it never issued — which is what every absent id below is — is reported as the
        // value it could not read, while an absent NAME is a template that does not exist.
        let expected = if member.ends_with("LaunchTemplateId") {
            format!("Malformed launch template Id: {value}")
        } else {
            format!("The specified launch template, with template name {value}, does not exist.")
        };
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the sentence AWS answers for the half the request stated"
        );
        assert!(
            !group_exists(&router, &name).await,
            "case {case:?}: the refused create must leave no group"
        );
    }
}

/// One row of the launch-template-VERSION table: the case, the prefix the specification is
/// carried under, which half of it names the template, the `Version` the row states (`None`
/// for a row that states none) and whether the request is served.
struct VersionCase {
    case: &'static str,
    prefix: &'static str,
    by_id: bool,
    version: Option<&'static str>,
    served: bool,
}

/// Mint ONE more version of the launch template `name`, through Amazon EC2's own writer, and
/// answer the number it was given.
///
/// A template that holds two versions is what lets a row name the version after the first,
/// the one past the last the template ever minted, and the two words that float on whichever
/// version is current.
fn mint_another_version(engine: &Arc<engine::InMemoryEngine>, name: &str) -> i64 {
    let launch_template_id = held_template_id(engine, name);
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let mut store = state.lock();
    store
        .insert_launch_template_version(
            &launch_template_id,
            ec2::LaunchTemplateVersionSpec {
                created_by: engine::emulator_arn("iam", "", "root"),
                create_time: std::time::SystemTime::now(),
                version_description: None,
                data: crate::support::seeded_launch_template_data(),
            },
        )
        .expect("the account must be able to hold a second version of this launch template")
}

/// RULE: a group launches from ONE VERSION of a launch template, so the version its launch
/// source names must be one the template HOLDS — a number the template minted, or one of the
/// two words that float on the version in force. A version the template never minted is
/// refused with Amazon EC2's own sentence for the half that did not resolve, naming the
/// version and the template's id, and leaves no group.
#[tokio::test]
async fn a_group_launches_only_from_a_launch_template_version_the_account_holds() {
    const OWN_TEMPLATE: &str = "LaunchTemplate";
    const POLICY_TEMPLATE: &str = "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification";
    let rows: &[VersionCase] = &[
        VersionCase {
            case: "no version at all: the group launches from the default version",
            prefix: OWN_TEMPLATE,
            by_id: false,
            version: None,
            served: true,
        },
        VersionCase {
            case: "the template's first version",
            prefix: OWN_TEMPLATE,
            by_id: false,
            version: Some("1"),
            served: true,
        },
        VersionCase {
            case: "the version minted after the first",
            prefix: OWN_TEMPLATE,
            by_id: false,
            version: Some("2"),
            served: true,
        },
        VersionCase {
            case: "the word that floats on the default version",
            prefix: OWN_TEMPLATE,
            by_id: false,
            version: Some("$Default"),
            served: true,
        },
        VersionCase {
            case: "the word that floats on the latest version",
            prefix: OWN_TEMPLATE,
            by_id: true,
            version: Some("$Latest"),
            served: true,
        },
        VersionCase {
            case: "one past the last version the template minted",
            prefix: OWN_TEMPLATE,
            by_id: false,
            version: Some("3"),
            served: false,
        },
        VersionCase {
            case: "far past the last version, on a template named by its id",
            prefix: OWN_TEMPLATE,
            by_id: true,
            version: Some("99"),
            served: false,
        },
        VersionCase {
            case: "below the first version a template can carry",
            prefix: OWN_TEMPLATE,
            by_id: false,
            version: Some("0"),
            served: false,
        },
        VersionCase {
            case: "a held version of the launch template of a mixed instances policy",
            prefix: POLICY_TEMPLATE,
            by_id: false,
            version: Some("2"),
            served: true,
        },
        VersionCase {
            case: "an absent version of the launch template of a mixed instances policy",
            prefix: POLICY_TEMPLATE,
            by_id: false,
            version: Some("99"),
            served: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator();
        let held_id = held_template_id(&engine, TEMPLATE);
        let minted = mint_another_version(&engine, TEMPLATE);
        assert_eq!(
            minted, 2,
            "case {:?}: the harness must leave the template holding versions 1 and 2",
            row.case
        );
        let name = format!("asg-template-version-{index}");
        // Each row states its OWN launch source, so the base create's launch template is
        // replaced rather than added to.
        let mut params: Vec<(String, String)> = base_create(&name)
            .into_iter()
            .filter(|(key, _)| key != "LaunchTemplate.LaunchTemplateName")
            .collect();
        let (half, value) = if row.by_id {
            ("LaunchTemplateId", held_id.clone())
        } else {
            ("LaunchTemplateName", TEMPLATE.to_string())
        };
        params.push(p(&format!("{}.{half}", row.prefix), value));
        if let Some(version) = row.version {
            params.push(p(&format!("{}.Version", row.prefix), version));
        }

        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {:?}: a version the template holds must be accepted, got {status}: {body}",
                row.case
            );
            assert!(
                group_exists(&router, &name).await,
                "case {:?}: the accepted create must leave a group",
                row.case
            );
            // A version the request pinned by NUMBER is the version the group reports back,
            // which is what makes the accepted row a round trip rather than a 200.
            if let Some(version) = row
                .version
                .filter(|version| version.parse::<i64>().is_ok() && row.prefix == OWN_TEMPLATE)
            {
                let (_, described) = query(
                    &router,
                    "DescribeAutoScalingGroups",
                    &[("AutoScalingGroupNames.member.1", name.as_str())],
                )
                .await;
                assert_eq!(
                    tag_value(&described, "Version"),
                    version,
                    "case {:?}: the group must report the version it was pinned to, got \
                     {described}",
                    row.case
                );
            }
            continue;
        }
        assert_eq!(
            status, 400,
            "case {:?}: expected the version to be refused, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {:?}: expected ValidationError, got {body}",
            row.case
        );
        let requested = row.version.expect("a refused row states a version");
        assert_eq!(
            message(&body),
            format!(
                "Could not find the specified version {requested} for the launch template with \
                 ID {held_id}."
            ),
            "case {:?}: expected Amazon EC2's own sentence for the version half",
            row.case
        );
        assert!(
            !group_exists(&router, &name).await,
            "case {:?}: the refused create must leave no group",
            row.case
        );
    }
}

/// RULE: a `Version` is one of the two words that float on a version in force or a NUMBER, so
/// a word that is neither names no version at all and is refused with this service's own
/// sentence listing the forms a version may take — a different verdict from a version that
/// reads as one and names none the template holds, which names that version and the template.
///
/// The rows span what the rule generalises over: the case of the two words (which name the
/// version only as written), a word of neither spelling, an empty-looking word and a numeric
/// value with a unit on it; the two served rows beside them are the two words themselves, so a
/// rule that refused too much could not pass either. Each is driven through the create and the
/// update, the two writers of a group's launch source.
#[tokio::test]
async fn a_version_that_names_no_version_at_all_is_refused_for_its_form() {
    // The sentence is spelled out here rather than imported, so a wrong constant in the
    // implementation cannot make this pass. Entirely fixed text: it lists the forms and
    // carries no part of the request.
    const UNREADABLE_VERSION: &str = "Invalid launch template version: either '$Default', \
                                      '$Latest', or a numeric version are allowed.";
    // (case, the Version the request states, whether it names a version)
    let rows: &[(&str, &str, bool)] = &[
        ("the latest-version word in lower case", "$latest", false),
        ("the default-version word in lower case", "$default", false),
        ("a word of neither spelling", "newest", false),
        ("a version with a unit on the number", "1v", false),
        ("the word that floats on the latest version", "$Latest", true),
        ("the word that floats on the default version", "$Default", true),
    ];

    for (index, (case, version, names_a_version)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-version-word-{index}");
        let mut params = base_create(&name);
        params.push(p("LaunchTemplate.Version", *version));
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;

        // The same word through the UPDATE, which resolves the launch source the same way: the
        // group it repoints launches from the template the account holds.
        let standing = format!("{name}-standing");
        let (standing_status, standing_body) =
            query(&router, "CreateAutoScalingGroup", &borrowed(&base_create(&standing))).await;
        assert_eq!(
            standing_status, 200,
            "case {case:?}: the group the update drives must be created, got \
             {standing_status}: {standing_body}"
        );
        let (update_status, update_body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", standing.as_str()),
                ("LaunchTemplate.LaunchTemplateName", TEMPLATE),
                ("LaunchTemplate.Version", version),
            ],
        )
        .await;

        if *names_a_version {
            assert_eq!(
                status, 200,
                "case {case:?}: a word that names a version must be accepted, got {status}: \
                 {body}"
            );
            assert_eq!(
                update_status, 200,
                "case {case:?}: the update must be served too, got {update_status}: \
                 {update_body}"
            );
            assert!(
                group_exists(&router, &name).await,
                "case {case:?}: the accepted create must leave a group"
            );
            continue;
        }
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
                "ValidationError",
                "case {case:?}/{operation}: expected ValidationError, got {body}"
            );
            assert_eq!(
                message(body),
                UNREADABLE_VERSION,
                "case {case:?}/{operation}: expected the sentence for a version that names \
                 none, got {body}"
            );
        }
        assert!(
            !group_exists(&router, &name).await,
            "case {case:?}: the refused create must leave no group"
        );
    }
}

/// RULE: the same version resolution guards an UPDATE, and a refused update leaves the group
/// on the launch-template version it was already running.
#[tokio::test]
async fn an_update_onto_an_absent_launch_template_version_is_refused() {
    let (router, engine) = emulator();
    let held_id = held_template_id(&engine, TEMPLATE);
    mint_another_version(&engine, TEMPLATE);
    let group = "asg-template-version-update";
    let (status, body) = create(
        &router,
        group,
        &[p("LaunchTemplate.Version", "1".to_string())],
    )
    .await;
    assert_eq!(status, 200, "the group must be created, got {status}: {body}");

    // (case, the version the update names, whether it is served, the version the group runs
    // afterwards)
    let rows: &[(&str, &str, bool, &str)] = &[
        ("onto the second version the template holds", "2", true, "2"),
        ("onto a version the template never minted", "7", false, "2"),
        ("onto a version below the first", "0", false, "2"),
        ("back onto the first version", "1", true, "1"),
    ];
    for (case, version, served, running) in rows {
        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", group),
                ("LaunchTemplate.LaunchTemplateName", TEMPLATE),
                ("LaunchTemplate.Version", version),
            ],
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update to be refused, got {status}: {body}"
            );
            assert_eq!(
                message(&body),
                format!(
                    "Could not find the specified version {version} for the launch template \
                     with ID {held_id}."
                ),
                "case {case:?}: expected Amazon EC2's own sentence for the version half"
            );
        }
        let (_, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", group)],
        )
        .await;
        assert_eq!(
            tag_value(&described, "Version"),
            *running,
            "case {case:?}: the group must run the version the last SERVED request left it on, \
             got {described}"
        );
    }
}

/// RULE: the same resolution guards an UPDATE, and a refused update leaves the group on the
/// launch template it was already running.
#[tokio::test]
async fn an_update_onto_an_absent_launch_template_is_refused() {
    let (router, _engine) = emulator();
    let (status, body) = create(&router, "asg-template-update", &[]).await;
    assert_eq!(status, 200, "the group must be created, got {status}: {body}");

    // (case, the member the update names the template under, the value, whether it is served)
    let rows: &[UpdateValueRow] = &[
        (
            "onto a second template the account holds",
            "LaunchTemplate.LaunchTemplateName",
            OTHER_TEMPLATE,
            true,
        ),
        (
            "onto a name nothing holds",
            "LaunchTemplate.LaunchTemplateName",
            ABSENT_TEMPLATE_NAME,
            false,
        ),
        (
            "onto an id nothing holds",
            "LaunchTemplate.LaunchTemplateId",
            ABSENT_TEMPLATE_ID,
            false,
        ),
    ];
    for (case, member, value, served) in rows {
        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[("AutoScalingGroupName", "asg-template-update"), (member, value)],
        )
        .await;
        let (_, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", "asg-template-update")],
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            );
            assert_eq!(
                tag_value(&described, "LaunchTemplateName"),
                *value,
                "case {case:?}: the group must report the template it is now on, got {described}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&described, "LaunchTemplateName"),
            OTHER_TEMPLATE,
            "case {case:?}: the refused update must leave the group on the template it had, \
             got {described}"
        );
    }
}

/// RULE: an instance maintenance policy states a lower healthy threshold for every upper
/// one, and the two are at most 100 apart — on the create and on the update alike, and a
/// refused request changes no stored threshold. A request that CHANGES a stored pair may also
/// clear a threshold, which is the one difference between the two sentences the pair rule is
/// answered with.
#[tokio::test]
async fn the_maintenance_policy_thresholds_must_hold_together() {
    // The sentences AWS states for the two rules, spelled out here.
    let spread = "The difference between MaxHealthyPercentage and MinHealthyPercentage must be \
                  less than or equal to 100.";
    let pair_stated =
        "Both MinHealthyPercentage and MaxHealthyPercentage must be specified or not at all.";
    let pair_changed = "Both MinHealthyPercentage and MaxHealthyPercentage must be specified \
                        (excluding -1) or not at all.";
    // (case, MinHealthyPercentage, MaxHealthyPercentage, the refusal the create answers and
    //  the one the update answers)
    let rows: &[ThresholdRow] = &[
        ("a lower threshold alone", Some("90"), None, None),
        ("both, 10 apart", Some("90"), Some("100"), None),
        ("both, exactly 100 apart", Some("0"), Some("100"), None),
        ("both at the published bounds", Some("100"), Some("200"), None),
        (
            "both, 101 apart",
            Some("49"),
            Some("150"),
            Some((spread, spread)),
        ),
        (
            "both, the widest the members allow",
            Some("0"),
            Some("200"),
            Some((spread, spread)),
        ),
        (
            "an upper threshold alone",
            None,
            Some("150"),
            Some((pair_stated, pair_changed)),
        ),
        (
            "an upper threshold beside a reset lower one",
            Some("-1"),
            Some("150"),
            Some((pair_stated, pair_changed)),
        ),
        ("both reset", Some("-1"), Some("-1"), None),
    ];

    for (index, (case, min, max, refusal)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-maintenance-{index}");
        let mut extra: Vec<(String, String)> = Vec::new();
        if let Some(min) = min {
            extra.push(p("InstanceMaintenancePolicy.MinHealthyPercentage", *min));
        }
        if let Some(max) = max {
            extra.push(p("InstanceMaintenancePolicy.MaxHealthyPercentage", *max));
        }
        let (status, body) = create(&router, &name, &extra).await;
        match refusal {
            Some((expected, _)) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the policy to be refused, got {status}: {body}"
                );
                assert_eq!(
                    tag_value(&body, "Code"),
                    "ValidationError",
                    "case {case:?}: expected ValidationError, got {body}"
                );
                assert_eq!(
                    message(&body),
                    *expected,
                    "case {case:?}: expected AWS's sentence for this rule"
                );
                assert!(
                    !group_exists(&router, &name).await,
                    "case {case:?}: the refused create must leave no group"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the policy to be accepted, got {status}: {body}"
            ),
        }

        // The same rule on the UPDATE: a group already carrying a valid policy is left with
        // it when the update is refused.
        let (router, _engine) = emulator();
        let updated = format!("asg-maintenance-update-{index}");
        let (status, body) = create(
            &router,
            &updated,
            &[
                p("InstanceMaintenancePolicy.MinHealthyPercentage", "90"),
                p("InstanceMaintenancePolicy.MaxHealthyPercentage", "110"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created with a valid policy, got {status}: {body}"
        );
        let mut params = vec![p("AutoScalingGroupName", updated.as_str())];
        params.extend(extra);
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&params)).await;
        let (_, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", updated.as_str())],
        )
        .await;
        match refusal {
            Some((_, expected)) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the update to be refused, got {status}: {body}"
                );
                assert_eq!(
                    message(&body),
                    *expected,
                    "case {case:?}: expected AWS's sentence for this rule on the update"
                );
                assert_eq!(
                    (
                        tag_value(&described, "MinHealthyPercentage"),
                        tag_value(&described, "MaxHealthyPercentage")
                    ),
                    ("90".to_string(), "110".to_string()),
                    "case {case:?}: the refused update must leave the stored policy alone, got \
                     {described}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            ),
        }
    }
}

/// RULE: the same pair of healthy thresholds carried by the PREFERENCES of an instance
/// refresh is taken against the same two rules, and answered the sentence the service words
/// for a refresh's own preferences.
#[tokio::test]
async fn the_threshold_rules_hold_for_an_instance_refreshs_preferences() {
    let spread = "The difference between MaxHealthyPercentage and MinHealthyPercentage must be \
                  less than or equal to 100.";
    let pair = "MinHealthyPercentage is required to use MaxHealthyPercentage. Specify \
                MinHealthyPercentage, or remove MaxHealthyPercentage, and try again.";
    // (case, MinHealthyPercentage, MaxHealthyPercentage, the refusal expected, if any)
    let rows: &[RefreshThresholdRow] = &[
        ("a lower threshold alone", Some("50"), None, None),
        ("both, exactly 100 apart", Some("50"), Some("150"), None),
        ("both, 101 apart", Some("49"), Some("150"), Some(spread)),
        ("an upper threshold alone", None, Some("150"), Some(pair)),
    ];

    for (index, (case, min, max, refusal)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-refresh-thresholds-{index}");
        let (status, body) = create(&router, &name, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created, got {status}: {body}"
        );
        let mut params = vec![p("AutoScalingGroupName", name.as_str())];
        if let Some(min) = min {
            params.push(p("Preferences.MinHealthyPercentage", *min));
        }
        if let Some(max) = max {
            params.push(p("Preferences.MaxHealthyPercentage", *max));
        }
        let (status, body) = query(&router, "StartInstanceRefresh", &borrowed(&params)).await;
        match refusal {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the preferences to be refused, got {status}: {body}"
                );
                assert_eq!(
                    tag_value(&body, "Code"),
                    "ValidationError",
                    "case {case:?}: expected ValidationError, got {body}"
                );
                assert_eq!(
                    message(&body),
                    *expected,
                    "case {case:?}: expected AWS's sentence for this rule"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the preferences to be accepted, got {status}: {body}"
            ),
        }
    }
}

/// RULE: one instance type has at most ONE override in a mixed instances policy, and a
/// request overriding a type twice is refused naming that type, leaving no group.
#[tokio::test]
async fn one_instance_type_has_at_most_one_override() {
    // (case, the instance types the overrides name in order, whether the create is served)
    let rows: &[OverrideRow] = &[
        ("one override", &[FIRST_TYPE], true),
        ("two overrides of different types", &[FIRST_TYPE, SECOND_TYPE], true),
        ("the same type twice", &[FIRST_TYPE, FIRST_TYPE], false),
        (
            "a repeat after another type",
            &[FIRST_TYPE, SECOND_TYPE, FIRST_TYPE],
            false,
        ),
        (
            "a repeat of the second type",
            &[FIRST_TYPE, SECOND_TYPE, SECOND_TYPE],
            false,
        ),
    ];

    for (index, (case, types, served)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-overrides-{index}");
        let mut params: Vec<(String, String)> = base_create(&name)
            .into_iter()
            .filter(|(key, _)| key != "LaunchTemplate.LaunchTemplateName")
            .collect();
        params.push(p(
            "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
            TEMPLATE,
        ));
        for (position, instance_type) in types.iter().enumerate() {
            params.push(p(
                &format!(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.{}.InstanceType",
                    position + 1
                ),
                *instance_type,
            ));
            // A weight on every override, so the duplicate is refused for being a second
            // override of one type rather than for anything about its weight.
            params.push(p(
                &format!(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.{}.WeightedCapacity",
                    position + 1
                ),
                (position + 1).to_string(),
            ));
        }
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: distinct overrides must be accepted, got {status}: {body}"
            );
            assert!(
                group_exists(&router, &name).await,
                "case {case:?}: the accepted create must leave a group"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate override to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        let answered = message(&body);
        let repeated: Vec<&str> = types
            .iter()
            .enumerate()
            .filter(|(position, instance_type)| types[..*position].contains(instance_type))
            .map(|(_, instance_type)| *instance_type)
            .collect();
        assert_eq!(
            answered,
            format!(
                "Cannot add same instance type override more than once. Remove these duplicates \
                 from the request and try again: [{}]",
                repeated.join(", ")
            ),
            "case {case:?}: expected AWS's sentence naming every type the request repeated"
        );
        assert!(
            !group_exists(&router, &name).await,
            "case {case:?}: the refused create must leave no group"
        );

        // The same rule on the UPDATE: the group keeps the launch source it had.
        let (router, _engine) = emulator();
        let updated = format!("asg-overrides-update-{index}");
        let (status, body) = create(&router, &updated, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created, got {status}: {body}"
        );
        let mut update = vec![p("AutoScalingGroupName", updated.as_str())];
        update.extend(
            params
                .into_iter()
                .filter(|(key, _)| key.starts_with("MixedInstancesPolicy")),
        );
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&update)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused too, got {status}: {body}"
        );
        let (_, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", updated.as_str())],
        )
        .await;
        assert_eq!(
            tag_value(&described, "LaunchTemplateName"),
            TEMPLATE,
            "case {case:?}: the refused update must leave the group's launch source alone, got \
             {described}"
        );
    }
}

/// RULE: a request names exactly ONE launch source, and that is decided from the members it
/// carries — so a request naming two is told to remove one, whether or not either value
/// resolves.
#[tokio::test]
async fn a_request_names_exactly_one_launch_source() {
    let one_of = "Valid requests must contain either LaunchTemplate, LaunchConfigurationName, \
                  InstanceId or MixedInstancesPolicy parameter.";
    // (case, the launch-source members the request carries)
    let rows: Vec<LaunchSourceRow> = vec![
        ("no launch source at all", vec![]),
        (
            "a launch template and a launch configuration that does not exist",
            vec![
                p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
                p("LaunchConfigurationName", "lc-never-created"),
            ],
        ),
        (
            "a launch template and a mixed instances policy",
            vec![
                p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
                p(
                    "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.\
                     LaunchTemplateName",
                    TEMPLATE,
                ),
            ],
        ),
        (
            "an instance and a launch template",
            vec![
                p("InstanceId", "i-00000000000000001"),
                p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
            ],
        ),
        (
            "a launch configuration and an instance",
            vec![
                p("LaunchConfigurationName", "lc-never-created"),
                p("InstanceId", "i-00000000000000001"),
            ],
        ),
    ];

    for (index, (case, source)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-source-{index}");
        let mut params: Vec<(String, String)> = base_create(&name)
            .into_iter()
            .filter(|(key, _)| key != "LaunchTemplate.LaunchTemplateName")
            .collect();
        params.extend(source.iter().cloned());
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            message(&body),
            one_of,
            "case {case:?}: expected AWS's one-of sentence, which names no value of the request"
        );
        assert!(
            !group_exists(&router, &name).await,
            "case {case:?}: the refused create must leave no group"
        );
    }
}

/// RULE: a forced delete is refused while the group's `TerminateHookAbandon` retention
/// trigger is set to retain, and the group survives; any other setting of the trigger leaves
/// the delete served.
#[tokio::test]
async fn a_force_delete_is_refused_while_the_retention_trigger_retains() {
    let retains = "You can't force delete this Auto Scaling group because the TerminateHookAbandon \
                   retention trigger is set to retain. Change the retention trigger to terminate \
                   and try again.";
    // (case, the trigger the group carries, whether the forced delete is served)
    let rows: &[RetentionRow] = &[
        ("a group retaining abandoned terminations", Some("retain"), false),
        ("a group terminating them", Some("terminate"), true),
        ("a group with no retention trigger at all", None, true),
    ];

    for (index, (case, trigger, served)) in rows.iter().enumerate() {
        let (router, _engine) = emulator();
        let name = format!("asg-retention-{index}");
        let extra: Vec<(String, String)> = trigger
            .iter()
            .map(|trigger| {
                p(
                    "InstanceLifecyclePolicy.RetentionTriggers.TerminateHookAbandon",
                    *trigger,
                )
            })
            .collect();
        let (status, body) = create(&router, &name, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created, got {status}: {body}"
        );

        let (status, body) = query(
            &router,
            "DeleteAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("ForceDelete", "true"),
            ],
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: the forced delete must be served, got {status}: {body}"
            );
            assert!(
                !group_exists(&router, &name).await,
                "case {case:?}: the served delete must leave no group"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the forced delete to be refused, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            message(&body),
            retains,
            "case {case:?}: expected AWS's retention sentence, which names no value"
        );
        assert!(
            group_exists(&router, &name).await,
            "case {case:?}: the refused delete must leave the group standing"
        );
        // The guard is about FORCING the delete: the group is deletable once nothing of it
        // is being destroyed against the trigger.
        let (status, body) = query(
            &router,
            "DeleteAutoScalingGroup",
            &[("AutoScalingGroupName", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete that forces nothing must still be served, got {status}: \
             {body}"
        );
    }
}

/// RULE: `AutoScalingGroupName` names the group a delete removes, so exactly ONE of a set of
/// racing deletes of one group is served and every other is refused as a name that answers to
/// nothing — the delete is not idempotent in its name — and the group is gone afterwards.
#[tokio::test]
async fn racing_deletes_of_one_group_serve_exactly_one_caller() {
    // (case, whether the racers force the delete)
    for (case, force) in [("forced deletes", true), ("plain deletes", false)] {
        let (router, _engine) = emulator();
        let (status, body) = create(&router, "asg-raced", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must be created, got {status}: {body}"
        );

        let mut params = vec![("AutoScalingGroupName", "asg-raced")];
        if force {
            params.push(("ForceDelete", "true"));
        }
        let delete = || query(&router, "DeleteAutoScalingGroup", &params);
        // Eight callers, all in flight at once, exactly as a parallel teardown sends them.
        let answers = tokio::join!(
            delete(),
            delete(),
            delete(),
            delete(),
            delete(),
            delete(),
            delete(),
            delete()
        );
        let answers = [
            answers.0, answers.1, answers.2, answers.3, answers.4, answers.5, answers.6,
            answers.7,
        ];
        let served = answers
            .iter()
            .filter(|(status, _)| *status == 200)
            .count();
        assert_eq!(
            served, 1,
            "case {case:?}: exactly one racer removes the group, got {served} served answers in \
             {answers:?}"
        );
        for (racer, (status, body)) in answers.iter().enumerate() {
            if *status == 200 {
                continue;
            }
            assert_eq!(
                *status, 400,
                "case {case:?}: racer {racer} asked for a group the first delete removed and \
                 must be refused, got {status}: {body}"
            );
            assert_eq!(
                tag_value(body, "Code"),
                "ValidationError",
                "case {case:?}: racer {racer} must be refused as an unknown group, got {body}"
            );
        }
        assert!(
            !group_exists(&router, "asg-raced").await,
            "case {case:?}: the group must be gone once the racing deletes are answered"
        );

        // A name no group ever answered to is refused in exactly the same terms.
        let (status, body) = query(
            &router,
            "DeleteAutoScalingGroup",
            &[("AutoScalingGroupName", "asg-never-created")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name no group answered to must still be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {case:?}: expected ValidationError for an unknown group, got {body}"
        );
    }
}
