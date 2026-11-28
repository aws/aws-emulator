// SPDX-License-Identifier: CC0-1.0

//! The refusal `DetachTrafficSources` answers when the Auto Scaling group it names does not
//! exist, over the awsQuery wire.
//!
//! The rule under test: when a request mutating the traffic sources or load balancers a group
//! is attached to names a group this account does not hold, the service refuses it HTTP 400
//! `ValidationError` with ONE constant sentence whose trailing detail field is left unfilled —
//! so it travels as the literal token `null` — and no part of that sentence is taken from the
//! request or from stored state.
//!
//! The tables below therefore span what the rule generalises over: the group name's length
//! (both ends of the member's `@length 1..255`), its character set, case, separators and
//! spelling — including a name spelled exactly like the message's own detail token, a name
//! spelled like the wire error code, and a name that extends a group that DOES exist — whether
//! the name never existed or was created and then deleted, how many traffic sources the request
//! carries (one up to the documented per-call bound), which kind each is stated under (`elb`,
//! `elbv2`, `vpc-lattice`, or no `Type` at all), and which operation of the attachment family
//! carries the request. Every row asserts the WHOLE message, because a name spelled like part
//! of the sentence would satisfy a substring check against a handler that still interpolated
//! it.
//!
//! The scope of the rule is pinned too: the group that DOES exist still has its attachments
//! read, changed and read back, and the OTHER refusal of the same operation — a detach naming
//! associations the group does not hold — keeps naming those identifiers, so the constant
//! sentence is not spreading over refusals that do report a value.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, create_target_group, emulator, list_members, p, query, seed_subnets, tag_value,
    vpc_of, xml_unescape, SeededSubnet, REGION,
};

/// The launch source every group here is created from — a launch TEMPLATE, because no account
/// can create a launch configuration (see `launch_configurations_are_unavailable.rs`).
const LAUNCH_TEMPLATE: &str = "lt-1";

/// The whole message Amazon EC2 Auto Scaling answers a group-scoped attachment mutation that
/// names no stored group with: fixed text, the detail after the dash left unfilled.
const ABSENT_GROUP_MESSAGE: &str = "AutoScalingGroup name not found - null";

/// The wire error code of a client refusal on this awsQuery service.
const VALIDATION_ERROR: &str = "ValidationError";

/// The most traffic sources one call may name, from the model's own documentation for
/// `DetachTrafficSourcesType$TrafficSources`: "You can specify up to 10 traffic sources."
const MAX_TRAFFIC_SOURCES_PER_CALL: usize = 10;

/// The three `TrafficSourceIdentifier$Type` values an association can be stated under, from the
/// model's own catalog for the member
/// (`smithy_query(query="doc",shape="TrafficSourceIdentifier",member="Type")`): `elb` for the
/// name of a Classic Load Balancer, `elbv2` for an Elastic Load Balancing target group ARN and
/// `vpc-lattice` for a VPC Lattice target group ARN.
const TYPE_ELB: &str = "elb";
const TYPE_ELBV2: &str = "elbv2";
const TYPE_VPC_LATTICE: &str = "vpc-lattice";

/// The state an attachment of a group owing no registration is reported in: "all Auto Scaling
/// instances are registered with the load balancer or target group"
/// (`smithy_query(query="doc",shape="TrafficSourceState",member="State")`), which a group
/// holding no instance satisfies with none.
const STATE_ADDED: &str = "Added";

/// The port the target group of this suite listens on.
const TARGET_GROUP_PORT: i32 = 80;

/// The `TargetTypeEnum` value of a target group whose targets are Amazon EC2 instances
/// (`TargetGroup$TargetType`: "`instance` - Targets are specified by instance ID").
const TARGET_TYPE_INSTANCE: &str = "instance";

/// The name of the Classic Load Balancer this suite's `elb` rows state, in the form the member
/// documents ("Classic Load Balancer name: `my-classic-load-balancer`").
const CLASSIC_LOAD_BALANCER: &str = "my-classic-load-balancer";

/// The group that DOES exist in every test here, so a row refused for an absent name cannot be
/// passing because the account holds nothing at all.
const LIVING_GROUP: &str = "asg-detach-absent-live";

/// One traffic source as a client reads it off `DescribeTrafficSources`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedSource {
    identifier: String,
    source_type: String,
    state: String,
}

/// The ARN of a VPC Lattice target group in this account and Region — the form a `vpc-lattice`
/// source names ("VPC Lattice ARN:
/// `arn:aws:vpc-lattice:us-west-2:123456789012:targetgroup/tg-1234567890123456`",
/// `TrafficSourceIdentifier$Identifier`).
fn lattice_target_group(id: &str) -> String {
    engine::emulator_arn("vpc-lattice", REGION, &format!("targetgroup/{id}"))
}

/// Names a client can submit for a group that does not exist. They differ in length, character
/// set, case, separators and spelling; `AutoScalingGroupName` is `XmlStringMaxLen255` (len
/// 1..255, pattern
/// `^[\u{0020}-\u{D7FF}\u{E000}-\u{FFFD}\u{D800}\u{DC00}-\u{DBFF}\u{DFFF}\r\n\t]*$`,
/// `smithy_query(query="operation", operation="DetachTrafficSources")`), so every row here is a
/// name the boundary validator accepts and only the handler can refuse.
fn absent_group_names() -> Vec<(&'static str, String)> {
    let mut names: Vec<(&'static str, String)> = vec![
        ("the shortest name the model allows", "a".to_string()),
        (
            "a name of the shape the recorded calls used",
            "sim2real-behav-as-grp-0123456789ab".to_string(),
        ),
        ("an upper-case name", "ASG-DETACH-ABSENT".to_string()),
        (
            "a name carrying every separator the model allows",
            "asg.detach_absent-name 1".to_string(),
        ),
        ("a name spelled like the message's own detail token", "null".to_string()),
        ("a name spelled like the wire error code", VALIDATION_ERROR.to_string()),
        (
            "a name carrying XML metacharacters",
            "<asg & 'name' \"quoted\">".to_string(),
        ),
        ("a non-ASCII name", "グループ-ünïcode".to_string()),
        ("a name that is only digits", "1234567890123456".to_string()),
        (
            "a name extending the name of a group that does exist",
            format!("{LIVING_GROUP}-extra"),
        ),
    ];
    // Both ends of the member's own length constraint: the shortest is the first row above,
    // the longest is the last name the boundary admits.
    names.push(("the longest name the model allows", "z".repeat(255)));
    names
}

/// One traffic source as a request states it: the identifier, and the `Type` the row states for
/// it where it states one at all (the member is optional).
type StatedSource = (String, Option<&'static str>);

/// The traffic-source lists a client can carry, spanning the count (one up to the documented
/// per-call bound), the kind each is stated under, whether the kind is stated at all, whether
/// the same identifier repeats, and whether the identifiers resolve to records the account
/// really holds.
fn traffic_source_sets(elbv2_arn: &str, lattice_arn: &str) -> Vec<(&'static str, Vec<StatedSource>)> {
    let mut sets: Vec<(&'static str, Vec<StatedSource>)> = vec![
        (
            "one target group the account holds, stated as elbv2",
            vec![(elbv2_arn.to_string(), Some(TYPE_ELBV2))],
        ),
        (
            "one Classic Load Balancer name, stated as elb",
            vec![(CLASSIC_LOAD_BALANCER.to_string(), Some(TYPE_ELB))],
        ),
        (
            "one VPC Lattice target group, stated as vpc-lattice",
            vec![(lattice_arn.to_string(), Some(TYPE_VPC_LATTICE))],
        ),
        (
            "one identifier stating no Type at all",
            vec![(elbv2_arn.to_string(), None)],
        ),
        (
            "one identifier no account record answers to",
            vec![("not-a-traffic-source".to_string(), Some(TYPE_ELB))],
        ),
        (
            "an identifier spelled like the message's own detail token",
            vec![("null".to_string(), Some(TYPE_ELB))],
        ),
        (
            "one identifier named twice",
            vec![
                (elbv2_arn.to_string(), Some(TYPE_ELBV2)),
                (elbv2_arn.to_string(), Some(TYPE_ELBV2)),
            ],
        ),
        (
            "all three kinds at once",
            vec![
                (elbv2_arn.to_string(), Some(TYPE_ELBV2)),
                (CLASSIC_LOAD_BALANCER.to_string(), Some(TYPE_ELB)),
                (lattice_arn.to_string(), Some(TYPE_VPC_LATTICE)),
            ],
        ),
    ];
    // The documented per-call bound: the most traffic sources one call may name.
    sets.push((
        "as many traffic sources as one call may name",
        (0..MAX_TRAFFIC_SOURCES_PER_CALL)
            .map(|index| (format!("classic-load-balancer-{index}"), Some(TYPE_ELB)))
            .collect(),
    ));
    sets
}

/// The request parameters naming `sources` the way an awsQuery client writes a `TrafficSources`
/// list.
fn traffic_source_params(name: &str, sources: &[StatedSource]) -> Vec<(String, String)> {
    let mut params = vec![p("AutoScalingGroupName", name)];
    for (index, (identifier, stated)) in sources.iter().enumerate() {
        let member = index + 1;
        params.push(p(
            format!("TrafficSources.member.{member}.Identifier").as_str(),
            identifier.clone(),
        ));
        if let Some(source_type) = stated {
            params.push(p(
                format!("TrafficSources.member.{member}.Type").as_str(),
                *source_type,
            ));
        }
    }
    params
}

/// Post one awsQuery action and answer its status, wire error code and message.
async fn refusal_of(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String, String) {
    let (status, body) = query(router, action, &borrowed(params)).await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Post one `DetachTrafficSources` naming `sources`.
async fn detach_traffic_sources(
    router: &MultiServiceRouter,
    name: &str,
    sources: &[StatedSource],
) -> (u16, String, String) {
    refusal_of(
        router,
        "DetachTrafficSources",
        &traffic_source_params(name, sources),
    )
    .await
}

/// Create one group with no capacity, so it owes no registration and nothing is launched.
async fn create_group(router: &MultiServiceRouter, name: &str, subnets: &[SeededSubnet]) {
    let ids: Vec<&str> = subnets.iter().map(|subnet| subnet.id.as_str()).collect();
    let params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
        p("MinSize", "0"),
        p("MaxSize", "4"),
        p("DesiredCapacity", "0"),
        p("VPCZoneIdentifier", ids.join(",")),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "creating {name:?} must succeed, got {status} with {body:?}"
    );
}

/// Attach `sources` to `name` and require the call to be served: what a test sets up rather
/// than what it asserts.
async fn attach_traffic_sources(router: &MultiServiceRouter, name: &str, sources: &[StatedSource]) {
    let (status, code, message) = refusal_of(
        router,
        "AttachTrafficSources",
        &traffic_source_params(name, sources),
    )
    .await;
    assert_eq!(
        status, 200,
        "attaching {sources:?} to {name:?} must succeed, got {status} with {code:?} {message:?}"
    );
}

/// The traffic sources the group reports through `DescribeTrafficSources`, in the order
/// reported.
async fn reported_sources(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Vec<ReportedSource> {
    let (status, body) = query(
        router,
        "DescribeTrafficSources",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the traffic sources of {name:?} expected 200, got {status} with \
         {body:?}"
    );
    list_members(&body, "TrafficSources")
        .into_iter()
        .map(|member| ReportedSource {
            identifier: tag_value(member, "Identifier"),
            source_type: tag_value(member, "Type"),
            state: tag_value(member, "State"),
        })
        .collect()
}

/// What the group must report for the sources it still holds: each one once, under the type it
/// is held as, in attachment order, in the state an attachment owing no registration is in.
fn expected_sources(held: &[(&str, &str)]) -> Vec<ReportedSource> {
    held.iter()
        .map(|(identifier, source_type)| ReportedSource {
            identifier: (*identifier).to_string(),
            source_type: (*source_type).to_string(),
            state: STATE_ADDED.to_string(),
        })
        .collect()
}

/// Seed one emulator holding `LIVING_GROUP`, and answer the router, the ARN of a target group
/// the account really holds, and a VPC Lattice target group ARN.
async fn seeded_emulator() -> (MultiServiceRouter, String, String) {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    let elbv2_arn = create_target_group(
        &router,
        "tg-detach-absent-group",
        TARGET_GROUP_PORT,
        TARGET_TYPE_INSTANCE,
        &vpc_id,
    )
    .await;
    create_group(&router, LIVING_GROUP, &subnets).await;
    (
        router,
        elbv2_arn,
        lattice_target_group("tg-0123456789abcdef0"),
    )
}

/// Every absent name, crossed with every traffic-source list: HTTP 400, `ValidationError`, and
/// the one constant message — no part of it taken from the request.
#[tokio::test]
async fn a_traffic_source_detach_of_an_absent_group_answers_one_constant_message() {
    let (router, elbv2_arn, lattice_arn) = seeded_emulator().await;

    for (name_case, name) in absent_group_names() {
        for (sources_case, sources) in traffic_source_sets(&elbv2_arn, &lattice_arn) {
            let case = format!("{name_case} with {sources_case}");
            let (status, code, message) = detach_traffic_sources(&router, &name, &sources).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {code:?} {message:?}"
            );
            assert_eq!(
                code, VALIDATION_ERROR,
                "case {case:?}: expected the {VALIDATION_ERROR} code, got {code:?} {message:?}"
            );
            // Equality with the constant is what says no part of the request reached the
            // message: every row submits a different name, and every row reports this same
            // sentence.
            assert_eq!(
                message, ABSENT_GROUP_MESSAGE,
                "case {case:?}: expected the message {ABSENT_GROUP_MESSAGE:?}, got {message:?}"
            );
        }
    }
}

/// The rule reaches the whole attachment family, not one operation: every sibling that changes
/// which traffic sources or load balancers a group is attached to answers the same constant
/// sentence for a name no group answers to.
#[tokio::test]
async fn the_attachment_family_answers_the_same_constant_message() {
    let (router, elbv2_arn, lattice_arn) = seeded_emulator().await;

    for (name_case, name) in absent_group_names() {
        let mut requests: Vec<(&str, Vec<(String, String)>)> = vec![
            (
                "AttachTrafficSources",
                traffic_source_params(&name, &[(lattice_arn.clone(), Some(TYPE_VPC_LATTICE))]),
            ),
            (
                "AttachLoadBalancers",
                vec![
                    p("AutoScalingGroupName", name.as_str()),
                    p("LoadBalancerNames.member.1", CLASSIC_LOAD_BALANCER),
                ],
            ),
            (
                "DetachLoadBalancers",
                vec![
                    p("AutoScalingGroupName", name.as_str()),
                    p("LoadBalancerNames.member.1", CLASSIC_LOAD_BALANCER),
                ],
            ),
        ];
        for action in ["AttachLoadBalancerTargetGroups", "DetachLoadBalancerTargetGroups"] {
            requests.push((
                action,
                vec![
                    p("AutoScalingGroupName", name.as_str()),
                    p("TargetGroupARNs.member.1", elbv2_arn.as_str()),
                ],
            ));
        }

        for (action, params) in requests {
            let case = format!("{action} with {name_case}");
            let (status, code, message) = refusal_of(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {code:?} {message:?}"
            );
            assert_eq!(
                code, VALIDATION_ERROR,
                "case {case:?}: expected the {VALIDATION_ERROR} code, got {code:?} {message:?}"
            );
            assert_eq!(
                message, ABSENT_GROUP_MESSAGE,
                "case {case:?}: expected the message {ABSENT_GROUP_MESSAGE:?}, got {message:?}"
            );
        }
    }
}

/// The scope of the rule, from both sides: a group that EXISTS still detaches what it holds and
/// reports the rest, the OTHER refusal of the same operation keeps naming the identifiers it is
/// about, and once that group is deleted its own name answers the constant absent-group
/// sentence — the sequence a teardown probe walks.
#[tokio::test]
async fn a_living_group_detaches_and_its_deleted_name_is_refused() {
    let (router, elbv2_arn, lattice_arn) = seeded_emulator().await;

    let held: Vec<StatedSource> = vec![
        (elbv2_arn.clone(), Some(TYPE_ELBV2)),
        (lattice_arn.clone(), Some(TYPE_VPC_LATTICE)),
    ];
    attach_traffic_sources(&router, LIVING_GROUP, &held).await;

    // The other refusal of this very operation: an identifier the group does not hold is
    // reported BY NAME, so the constant sentence has not spread over a refusal that does carry
    // a value from the request.
    let unheld = "classic-load-balancer-not-attached";
    let (status, code, message) = detach_traffic_sources(
        &router,
        LIVING_GROUP,
        &[(unheld.to_string(), Some(TYPE_ELB))],
    )
    .await;
    assert_eq!(
        status, 400,
        "detaching an unheld traffic source expected HTTP 400, got {status} with {code:?} \
         {message:?}"
    );
    assert_eq!(
        code, VALIDATION_ERROR,
        "detaching an unheld traffic source expected the {VALIDATION_ERROR} code, got {code:?}"
    );
    let expected =
        format!("Trying to remove Target Groups that are not part of the group: {unheld}");
    assert_eq!(
        message, expected,
        "expected the message {expected:?}, got {message:?}"
    );

    // Nothing was removed by that refusal: the group still reports both attachments.
    let expected = expected_sources(&[
        (elbv2_arn.as_str(), TYPE_ELBV2),
        (lattice_arn.as_str(), TYPE_VPC_LATTICE),
    ]);
    let actual = reported_sources(&router, "after the refused detach", LIVING_GROUP).await;
    assert_eq!(
        actual, expected,
        "expected the group to keep both traffic sources, got {actual:?}"
    );

    // The happy path the refusal must not have swallowed: one held source detaches, and the
    // implemented reader of that very collection reports only the one left.
    let (status, code, message) =
        detach_traffic_sources(&router, LIVING_GROUP, &held[..1]).await;
    assert_eq!(
        status, 200,
        "detaching a held traffic source must succeed, got {status} with {code:?} {message:?}"
    );
    let expected = expected_sources(&[(lattice_arn.as_str(), TYPE_VPC_LATTICE)]);
    let actual = reported_sources(&router, "after the served detach", LIVING_GROUP).await;
    assert_eq!(
        actual, expected,
        "expected only the traffic source the call did not name to be left, got {actual:?}"
    );

    // Delete the group, and its own name joins the absent ones.
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", LIVING_GROUP)],
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting {LIVING_GROUP:?} must succeed, got {status} with {body:?}"
    );

    let (status, code, message) = detach_traffic_sources(&router, LIVING_GROUP, &held).await;
    assert_eq!(
        status, 400,
        "a detach naming the deleted group expected HTTP 400, got {status} with {code:?} \
         {message:?}"
    );
    assert_eq!(
        code, VALIDATION_ERROR,
        "a detach naming the deleted group expected the {VALIDATION_ERROR} code, got {code:?}"
    );
    assert_eq!(
        message, ABSENT_GROUP_MESSAGE,
        "expected the message {ABSENT_GROUP_MESSAGE:?}, got {message:?}"
    );
}
