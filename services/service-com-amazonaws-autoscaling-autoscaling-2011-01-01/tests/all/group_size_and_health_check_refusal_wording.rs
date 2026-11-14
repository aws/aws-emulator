// SPDX-License-Identifier: CC0-1.0

//! The sentences Amazon EC2 Auto Scaling refuses a group's SIZES and its HEALTH CHECK
//! TYPE with.
//!
//! RULE: an error code is not the whole answer — a client (a CDK or CloudFormation
//! wrapper, a retry policy) tells one `ValidationError` from another by the sentence that
//! comes with it, so each refusal is the fixed text the service words for that error class
//! with the request's own values interpolated, and nothing else:
//!
//! * the three sizes describe ONE group, so a desired capacity outside the bounds the
//!   request leaves the group with is refused `Desired capacity:<DesiredCapacity> must be
//!   between the specified min size:<MinSize> and max size:<MaxSize>` — the three values
//!   being the only variable parts;
//! * `HealthCheckType` is "one or more comma-separated health check types"
//!   (`smithy_query(query="doc",shape="AutoScalingGroup",member="HealthCheckType")`), a
//!   free string the service validates itself, so the refusal is the one machine-readable
//!   statement of the set it accepts: the fixed sentence `Valid health check types are:
//!   [EC2, ELB, EBS, VPC_LATTICE].`, carrying nothing from the request.
//!
//! The rows span what each rule generalises over: which side of the range the capacity
//! misses and by how much, whether the bounds come from the create's own members or from
//! the group the update is applied to, and which health check type is named — one word
//! outside the set, a word of the set in the wrong case, a comma-separated list with one
//! unknown entry — beside the comma-separated forms the service accepts.

use crate::support::{borrowed, p, query, tag_value, xml_unescape};

/// A launch source every row carries, so no row is refused for naming none. The bounds and
/// the health check type are read before any launch source is resolved, so the template
/// need not exist.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-wording");

/// The zone every row places its group in.
const ZONE: (&str, &str) = ("AvailabilityZones.member.1", "us-west-2a");

/// The fixed text of the bounds refusal, with the request's three sizes interpolated.
fn expected_bounds_sentence(desired: i32, min: i32, max: i32) -> String {
    format!("Desired capacity:{desired} must be between the specified min size:{min} and max size:{max}")
}

/// The fixed sentence the service states the health check types it accepts with. The set is
/// the model's own ("the valid values are EC2, EBS, ELB, and VPC_LATTICE",
/// `smithy_query(query="doc",shape="CreateAutoScalingGroupType",member="HealthCheckType")`),
/// in the order this sentence lists them.
const EXPECTED_HEALTH_CHECK_SENTENCE: &str = "Valid health check types are: [EC2, ELB, EBS, VPC_LATTICE].";

/// The `(code, message)` of a refused awsQuery answer.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// One row of the bounds table: the sizes the create carries.
struct BoundsRow {
    case: &'static str,
    min: i32,
    max: i32,
    desired: i32,
}

#[tokio::test]
async fn a_desired_capacity_outside_the_bounds_is_refused_with_the_bounds_sentence() {
    let rows = [
        BoundsRow {
            case: "one above a maximum of one",
            min: 0,
            max: 1,
            desired: 2,
        },
        BoundsRow {
            case: "below the minimum",
            min: 2,
            max: 5,
            desired: 1,
        },
        BoundsRow {
            case: "one above bounds that are equal",
            min: 3,
            max: 3,
            desired: 4,
        },
        BoundsRow {
            case: "far above a maximum of zero",
            min: 0,
            max: 0,
            desired: 9,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let engine = crate::support::new_engine();
        let router = crate::support::router_with(engine);
        let name = format!("bounds-{index}");
        let params = [
            p("AutoScalingGroupName", name.clone()),
            p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
            p(ZONE.0, ZONE.1),
            p("MinSize", row.min.to_string()),
            p("MaxSize", row.max.to_string()),
            p("DesiredCapacity", row.desired.to_string()),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        let case = row.case;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                "ValidationError".to_string(),
                expected_bounds_sentence(row.desired, row.min, row.max)
            ),
            "case {case:?}: expected the bounds refusal, got {body}"
        );

        // A refused create stores nothing.
        let (status, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups expected 200, got {status}: {described}"
        );
        assert!(
            !described.contains(name.as_str()),
            "case {case:?}: a refused create leaves no group behind, got {described}"
        );
    }
}

/// One row of the update table: the bounds the group was created with, what the update
/// carries, and the three sizes the refusal must name — the ones the update LEAVES the
/// group with, which is where a member the request omits keeps the group's own value.
struct UpdateRow {
    case: &'static str,
    created_min: i32,
    created_max: i32,
    /// `MinSize`/`MaxSize` the update carries, when it carries them.
    update_min: Option<i32>,
    update_max: Option<i32>,
    desired: i32,
    expected_min: i32,
    expected_max: i32,
}

#[tokio::test]
async fn the_update_path_states_the_bounds_it_leaves_the_group_with() {
    let rows = [
        UpdateRow {
            case: "a capacity above the group's own maximum, which the update keeps",
            created_min: 0,
            created_max: 2,
            update_min: None,
            update_max: None,
            desired: 3,
            expected_min: 0,
            expected_max: 2,
        },
        UpdateRow {
            case: "a capacity below a minimum the update itself raises",
            created_min: 0,
            created_max: 4,
            update_min: Some(3),
            update_max: None,
            desired: 1,
            expected_min: 3,
            expected_max: 4,
        },
        UpdateRow {
            case: "a capacity above a maximum the update itself lowers",
            created_min: 0,
            created_max: 6,
            update_min: None,
            update_max: Some(2),
            desired: 5,
            expected_min: 0,
            expected_max: 2,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let engine = crate::support::new_engine();
        let router = crate::support::router_with(engine);
        let name = format!("update-bounds-{index}");
        let case = row.case;
        let created = [
            p("AutoScalingGroupName", name.clone()),
            p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
            p(ZONE.0, ZONE.1),
            p("MinSize", row.created_min.to_string()),
            p("MaxSize", row.created_max.to_string()),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&created)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateAutoScalingGroup expected 200, got {status}: {body}"
        );

        let mut update = vec![
            p("AutoScalingGroupName", name.clone()),
            p("DesiredCapacity", row.desired.to_string()),
        ];
        if let Some(min) = row.update_min {
            update.push(p("MinSize", min.to_string()));
        }
        if let Some(max) = row.update_max {
            update.push(p("MaxSize", max.to_string()));
        }
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &borrowed(&update)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                "ValidationError".to_string(),
                expected_bounds_sentence(row.desired, row.expected_min, row.expected_max)
            ),
            "case {case:?}: expected the bounds refusal, got {body}"
        );

        // The refused update changed nothing.
        let (_, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            (
                tag_value(&described, "MinSize"),
                tag_value(&described, "MaxSize")
            ),
            (
                row.created_min.to_string(),
                row.created_max.to_string()
            ),
            "case {case:?}: a refused update leaves the group's bounds alone, got {described}"
        );
    }
}

/// One row of the health check table: the value named, and whether the service accepts it.
struct HealthCheckRow {
    case: &'static str,
    value: &'static str,
    accepted: bool,
}

#[tokio::test]
async fn an_unsupported_health_check_type_is_refused_with_the_fixed_sentence() {
    let rows = [
        HealthCheckRow {
            case: "a word outside the set",
            value: "HTTP",
            accepted: false,
        },
        HealthCheckRow {
            case: "a word of the set in the wrong case",
            value: "ec2",
            accepted: false,
        },
        HealthCheckRow {
            case: "a comma-separated list with one unknown entry",
            value: "EC2,HTTP",
            accepted: false,
        },
        HealthCheckRow {
            case: "an entry of the set the issue never cited",
            value: "VPC_LATTICE",
            accepted: true,
        },
        HealthCheckRow {
            case: "the comma-separated form the model documents",
            value: "EC2,ELB",
            accepted: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let engine = crate::support::new_engine();
        let router = crate::support::router_with(engine);
        let name = format!("health-{index}");
        let case = row.case;
        let params = [
            p("AutoScalingGroupName", name.clone()),
            p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
            p(ZONE.0, ZONE.1),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("HealthCheckType", row.value),
        ];
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({value}): expected the create to be accepted, got {status}: {body}",
                value = row.value
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?} ({value}): expected the create to be refused, got {status}: {body}",
            value = row.value
        );
        assert_eq!(
            refusal(&body),
            (
                "ValidationError".to_string(),
                EXPECTED_HEALTH_CHECK_SENTENCE.to_string()
            ),
            "case {case:?}: expected the fixed health check sentence, got {body}"
        );
        assert!(
            !xml_unescape(&tag_value(&body, "Message")).contains(row.value),
            "case {case:?}: the sentence carries nothing from the request, got {body}"
        );

        // The same sentence on the update path, for a group that already exists.
        let base = format!("{name}-base");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &borrowed(&[
                p("AutoScalingGroupName", base.clone()),
                p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
                p(ZONE.0, ZONE.1),
                p("MinSize", "0"),
                p("MaxSize", "1"),
            ]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the base group must be created, got {status}: {body}"
        );
        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", base.as_str()),
                ("HealthCheckType", row.value),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                "ValidationError".to_string(),
                EXPECTED_HEALTH_CHECK_SENTENCE.to_string()
            ),
            "case {case:?}: expected the fixed health check sentence on the update, got {body}"
        );
    }
}
