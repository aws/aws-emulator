// SPDX-License-Identifier: CC0-1.0

//! Amazon EC2 Auto Scaling reports the tags its groups carry through `DescribeTags`.
//!
//! The RULE under test: `DescribeTags` answers HTTP 200 with the tags the account's Auto
//! Scaling groups hold, each projected as a `TagDescription` (the group as `ResourceId`,
//! `auto-scaling-group` as `ResourceType`, the key, the value and whether it propagates at
//! launch), narrowed by the filter types the model documents for this operation —
//! `auto-scaling-group`, `key`, `value` and `propagate-at-launch` — whose values are joined
//! with OR and whose filters are joined with AND, and paged by `MaxRecords`/`NextToken`. A
//! request that matches nothing answers an EMPTY list ("If there's no match, no special
//! message is returned"), never a refusal, and an unusable criterion — an unknown filter
//! type, a filter with no values, a `MaxRecords` outside its documented range, a
//! `NextToken` this service never issued — is refused with HTTP 400.
//!
//! The rows span what the rule generalises over: which filter type narrows the read, how
//! many values one filter carries, how many filters are combined, how many groups and tags
//! are in play, and whether anything matches at all. The tags are written through
//! `CreateAutoScalingGroup` and read back here, so every assertion follows state the
//! service stored rather than a value this suite invented.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;

use crate::support::{borrowed, ec2_query, p, query, seed_subnets, tag_value, tag_values};

/// The `ResourceType` every tag of an Auto Scaling group is reported under, from the
/// model's own documentation for `TagDescription$ResourceType`: "The only supported value
/// is auto-scaling-group". Spelled out here rather than imported, so a wrong constant in
/// the implementation cannot make a test pass.
const GROUP_RESOURCE_TYPE: &str = "auto-scaling-group";

/// The two groups every row reads over, and the tags each was created with: three tags
/// across two groups, differing in key, in value and in whether they propagate at launch.
const ALPHA: &str = "tags-alpha";
const BETA: &str = "tags-beta";

/// One tag of a group: its key, its value and its `PropagateAtLaunch`.
struct SeededTag {
    group: &'static str,
    key: &'static str,
    value: &'static str,
    propagate: bool,
}

const SEEDED: &[SeededTag] = &[
    SeededTag {
        group: ALPHA,
        key: "env",
        value: "prod",
        propagate: true,
    },
    SeededTag {
        group: ALPHA,
        key: "team",
        value: "platform",
        propagate: false,
    },
    SeededTag {
        group: BETA,
        key: "env",
        value: "staging",
        propagate: true,
    },
];

/// One emulator: Auto Scaling and Amazon EC2 on one engine, because a group is created
/// from an EC2 launch template placed in EC2 subnets.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One tag as a row of this suite's expectations: group, key, value and propagate flag.
fn rendered(group: &str, key: &str, value: &str, propagate: bool) -> String {
    format!("{group}|{key}|{value}|{propagate}")
}

/// Every `TagDescription` in an answered body, in the order the service reported them, and
/// in the form [`rendered`] writes one.
///
/// Each member carries all five projected values, so the per-member lists line up: a
/// member missing one would shift the others and fail the assertion loudly rather than
/// quietly.
fn described_tags(case: &str, body: &str) -> Vec<String> {
    let resource_ids = tag_values(body, "ResourceId");
    let resource_types = tag_values(body, "ResourceType");
    let keys = tag_values(body, "Key");
    let values = tag_values(body, "Value");
    let propagate = tag_values(body, "PropagateAtLaunch");
    assert!(
        resource_ids.len() == keys.len()
            && keys.len() == values.len()
            && values.len() == propagate.len()
            && propagate.len() == resource_types.len(),
        "case {case:?}: every reported tag carries ResourceId, ResourceType, Key, Value \
         and PropagateAtLaunch, got {body}"
    );
    for resource_type in &resource_types {
        assert_eq!(
            resource_type, GROUP_RESOURCE_TYPE,
            "case {case:?}: expected ResourceType {GROUP_RESOURCE_TYPE}, got {body}"
        );
    }
    resource_ids
        .iter()
        .zip(keys.iter())
        .zip(values.iter())
        .zip(propagate.iter())
        .map(|(((resource_id, key), value), propagate)| {
            format!("{resource_id}|{key}|{value}|{propagate}")
        })
        .collect()
}

/// Create the two groups and their tags through `CreateAutoScalingGroup`, over the wire,
/// and answer the subnet they are placed in — for a case that needs one more group beside
/// them.
async fn given_tagged_groups(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> String {
    let subnets = seed_subnets(engine, 1);
    let subnet = subnets
        .first()
        .expect("one seeded subnet")
        .id
        .clone();
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "tags-template"),
            p("LaunchTemplateData.ImageId", "ami-0abcdef1234567890"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate expected 200, got {status}: {body}"
    );

    for group in [ALPHA, BETA] {
        let mut params = vec![
            p("AutoScalingGroupName", group),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("LaunchTemplate.LaunchTemplateName", "tags-template"),
            p("VPCZoneIdentifier", subnet.clone()),
        ];
        for (position, tag) in SEEDED
            .iter()
            .filter(|tag| tag.group == group)
            .enumerate()
        {
            let member = position + 1;
            params.push(p(&format!("Tags.member.{member}.Key"), tag.key));
            params.push(p(&format!("Tags.member.{member}.Value"), tag.value));
            params.push(p(
                &format!("Tags.member.{member}.PropagateAtLaunch"),
                if tag.propagate { "true" } else { "false" },
            ));
        }
        let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "CreateAutoScalingGroup for {group} expected 200, got {status}: {body}"
        );
    }
    subnet
}

/// One row of the filter table: the request's filter parameters, and the tags it selects.
struct Row {
    case: &'static str,
    /// `Filters.member.N.*` parameters, as a client sends them.
    filters: &'static [(&'static str, &'static str)],
    /// The tags the row expects, in the order the service reports them: by the group the
    /// tag is on, then by key.
    expected: &'static [(&'static str, &'static str, &'static str, bool)],
}

const ROWS: &[Row] = &[
    Row {
        case: "no filters at all: every tag of every group",
        filters: &[],
        expected: &[
            (ALPHA, "env", "prod", true),
            (ALPHA, "team", "platform", false),
            (BETA, "env", "staging", true),
        ],
    },
    Row {
        case: "the auto-scaling-group filter, one value",
        filters: &[
            ("Filters.member.1.Name", "auto-scaling-group"),
            ("Filters.member.1.Values.member.1", ALPHA),
        ],
        expected: &[
            (ALPHA, "env", "prod", true),
            (ALPHA, "team", "platform", false),
        ],
    },
    Row {
        case: "the auto-scaling-group filter, both groups as values",
        filters: &[
            ("Filters.member.1.Name", "auto-scaling-group"),
            ("Filters.member.1.Values.member.1", ALPHA),
            ("Filters.member.1.Values.member.2", BETA),
        ],
        expected: &[
            (ALPHA, "env", "prod", true),
            (ALPHA, "team", "platform", false),
            (BETA, "env", "staging", true),
        ],
    },
    Row {
        case: "the key filter, selecting one key held by two groups",
        filters: &[
            ("Filters.member.1.Name", "key"),
            ("Filters.member.1.Values.member.1", "env"),
        ],
        expected: &[(ALPHA, "env", "prod", true), (BETA, "env", "staging", true)],
    },
    Row {
        case: "the key filter, two values joined with OR",
        filters: &[
            ("Filters.member.1.Name", "key"),
            ("Filters.member.1.Values.member.1", "env"),
            ("Filters.member.1.Values.member.2", "team"),
        ],
        expected: &[
            (ALPHA, "env", "prod", true),
            (ALPHA, "team", "platform", false),
            (BETA, "env", "staging", true),
        ],
    },
    Row {
        case: "the value filter",
        filters: &[
            ("Filters.member.1.Name", "value"),
            ("Filters.member.1.Values.member.1", "staging"),
        ],
        expected: &[(BETA, "env", "staging", true)],
    },
    Row {
        case: "the propagate-at-launch filter, true",
        filters: &[
            ("Filters.member.1.Name", "propagate-at-launch"),
            ("Filters.member.1.Values.member.1", "true"),
        ],
        expected: &[(ALPHA, "env", "prod", true), (BETA, "env", "staging", true)],
    },
    Row {
        case: "the propagate-at-launch filter, false",
        filters: &[
            ("Filters.member.1.Name", "propagate-at-launch"),
            ("Filters.member.1.Values.member.1", "false"),
        ],
        expected: &[(ALPHA, "team", "platform", false)],
    },
    Row {
        case: "two filters joined with AND",
        filters: &[
            ("Filters.member.1.Name", "key"),
            ("Filters.member.1.Values.member.1", "env"),
            ("Filters.member.2.Name", "value"),
            ("Filters.member.2.Values.member.1", "staging"),
        ],
        expected: &[(BETA, "env", "staging", true)],
    },
    Row {
        case: "two filters whose AND selects nothing",
        filters: &[
            ("Filters.member.1.Name", "auto-scaling-group"),
            ("Filters.member.1.Values.member.1", BETA),
            ("Filters.member.2.Name", "key"),
            ("Filters.member.2.Values.member.1", "team"),
        ],
        expected: &[],
    },
    Row {
        case: "a key no tag carries",
        filters: &[
            ("Filters.member.1.Name", "key"),
            ("Filters.member.1.Values.member.1", "unheld"),
        ],
        expected: &[],
    },
    Row {
        case: "a group the account does not hold",
        filters: &[
            ("Filters.member.1.Name", "auto-scaling-group"),
            ("Filters.member.1.Values.member.1", "absent-group"),
        ],
        expected: &[],
    },
];

#[tokio::test]
async fn describe_tags_reports_the_stored_tags_the_filters_select() {
    let (router, engine) = emulator();
    given_tagged_groups(&router, &engine).await;

    for row in ROWS {
        let case = row.case;
        let (status, body) = query(&router, "DescribeTags", row.filters).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTags expected 200, got {status}: {body}"
        );
        let expected: Vec<String> = row
            .expected
            .iter()
            .map(|(group, key, value, propagate)| rendered(group, key, value, *propagate))
            .collect();
        assert_eq!(
            described_tags(case, &body),
            expected,
            "case {case:?}: expected the tags {expected:?}, got {body}"
        );
        assert!(
            tag_values(&body, "NextToken").is_empty(),
            "case {case:?}: a page that holds every match issues no NextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn the_propagate_at_launch_filter_accepts_only_a_boolean() {
    // The filter "accepts a Boolean value", so only the two spellings of one select
    // anything: a value that is neither selects NO tag. A tag created without
    // `PropagateAtLaunch` stored the decision the service takes for it — the member's own
    // default, which is that the tag propagates — so it is selected by `true` and by
    // neither of the non-Booleans.
    let case = "a tag created with no PropagateAtLaunch";
    let (router, engine) = emulator();
    let subnet = given_tagged_groups(&router, &engine).await;
    let flagless_group = "tags-flagless";
    let flagless_key = "unflagged";
    let params = vec![
        p("AutoScalingGroupName", flagless_group),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("LaunchTemplate.LaunchTemplateName", "tags-template"),
        p("VPCZoneIdentifier", subnet),
        p("Tags.member.1.Key", flagless_key),
        p("Tags.member.1.Value", "unset"),
    ];
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup expected 200, got {status}: {body}"
    );

    // The tag IS in the store: a filter on its key reports it, so the exclusions below are
    // the filter's doing and not an absent tag.
    let (status, body) = query(
        &router,
        "DescribeTags",
        &[
            ("Filters.member.1.Name", "key"),
            ("Filters.member.1.Values.member.1", flagless_key),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTags expected 200, got {status}: {body}"
    );
    assert!(
        body.contains(flagless_key),
        "case {case:?}: the stored tag must be reported by a filter on its key, got {body}"
    );

    // (the filter value, whether it selects the tag that was created without the flag)
    let rows: &[(&str, bool)] = &[
        // The decision stored for a tag whose create stated none: it propagates.
        ("true", true),
        ("false", false),
        // A non-Boolean: it selects nothing at all, the flagless tag included.
        ("yes", false),
        ("1", false),
    ];
    for (value, selects) in rows {
        let (status, body) = query(
            &router,
            "DescribeTags",
            &[
                ("Filters.member.1.Name", "propagate-at-launch"),
                ("Filters.member.1.Values.member.1", value),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} / {value:?}: DescribeTags expected 200, got {status}: {body}"
        );
        assert_eq!(
            body.contains(flagless_key),
            *selects,
            "case {case:?} / {value:?}: the filter selects the tag created without the \
             flag: {selects}, got {body}"
        );
        if value.parse::<bool>().is_err() {
            assert!(
                tag_values(&body, "Key").is_empty(),
                "case {case:?} / {value:?}: a value that is not a Boolean selects no tag, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn describe_tags_pages_over_the_tags_it_selects() {
    let (router, engine) = emulator();
    given_tagged_groups(&router, &engine).await;

    // The whole selection, in the order the service reports it, so the pages below are
    // checked against the list they page over rather than against a written-out order.
    let (status, body) = query(&router, "DescribeTags", &[]).await;
    assert_eq!(status, 200, "DescribeTags expected 200, got {status}: {body}");
    let whole = described_tags("the whole selection", &body);
    assert_eq!(
        whole.len(),
        SEEDED.len(),
        "the seeded tags are the whole selection, got {whole:?}"
    );

    // One page per tag, each followed by the token that resumes the walk.
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..SEEDED.len() {
        let case = format!("page {step} of one tag");
        let mut params = vec![p("MaxRecords", "1")];
        if let Some(token) = token.as_ref() {
            params.push(p("NextToken", token.clone()));
        }
        let (status, body) = query(&router, "DescribeTags", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTags expected 200, got {status}: {body}"
        );
        let page = described_tags(&case, &body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: MaxRecords 1 answers one tag, got {page:?}"
        );
        collected.extend(page);
        let next = tag_value(&body, "NextToken");
        token = (!next.is_empty()).then_some(next);
        assert_eq!(
            token.is_some(),
            step + 1 < SEEDED.len(),
            "case {case:?}: a token follows every page but the last, got {body}"
        );
    }
    assert_eq!(
        collected, whole,
        "the pages walk the same selection the unpaged read answers, got {collected:?}"
    );
}

/// One refused request: the parameters, the error code and the text the message carries.
struct Refusal {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    code: &'static str,
    contains: &'static str,
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a filter whose Name is not one of the documented types",
        params: &[
            ("Filters.member.1.Name", "tag-key"),
            ("Filters.member.1.Values.member.1", "env"),
        ],
        code: "ValidationError",
        contains: "auto-scaling-group, key, value, propagate-at-launch",
    },
    Refusal {
        case: "a filter carrying no Name at all",
        params: &[("Filters.member.1.Values.member.1", "env")],
        code: "ValidationError",
        contains: "The Name attribute is required",
    },
    Refusal {
        case: "a filter carrying no values",
        params: &[("Filters.member.1.Name", "key")],
        code: "ValidationError",
        contains: "The Values attribute and one or more filter values are required",
    },
    Refusal {
        case: "a MaxRecords below the documented minimum",
        params: &[("MaxRecords", "0")],
        code: "ValidationError",
        contains: "greater than or equal to 1",
    },
    Refusal {
        case: "a MaxRecords above the documented maximum",
        params: &[("MaxRecords", "101")],
        code: "ValidationError",
        contains: "less than or equal to 100",
    },
    Refusal {
        case: "a NextToken this service never issued",
        params: &[("NextToken", "not-a-token")],
        code: "InvalidNextToken",
        contains: "not-a-token",
    },
    Refusal {
        case: "a NextToken that points past the end of the selection",
        params: &[("NextToken", "next-99")],
        code: "InvalidNextToken",
        contains: "next-99",
    },
];

#[tokio::test]
async fn describe_tags_refuses_what_it_cannot_satisfy() {
    let (router, engine) = emulator();
    given_tagged_groups(&router, &engine).await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = query(&router, "DescribeTags", refusal.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            refusal.code,
            "case {case:?}: expected the code {}, got {body}",
            refusal.code
        );
        let message = crate::support::xml_unescape(&tag_value(&body, "Message"));
        assert!(
            message.contains(refusal.contains),
            "case {case:?}: expected a message carrying {:?}, got {message:?}",
            refusal.contains
        );
    }
}
