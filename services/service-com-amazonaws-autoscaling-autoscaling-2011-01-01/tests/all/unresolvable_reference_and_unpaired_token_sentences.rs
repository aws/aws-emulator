// SPDX-License-Identifier: CC0-1.0

//! The two SENTENCES this service answers a request it refuses on a reference it cannot read
//! and on a resumption token it cannot page with.
//!
//! RULE under test, in two halves, each pinned over a SET of inputs rather than over one
//! recorded request:
//!
//! * a `LaunchTemplateId` the launch-template service's mint never ISSUED is no launch
//!   template id at all, so a request that configures a group against one is refused with that
//!   service's own sentence for a value it cannot read (`Malformed launch template Id: <id>`)
//!   before anything about the template — its existence, its version — is resolved, while an
//!   id that mint DID issue is resolved: the template it names is launched from, or reported as
//!   not existing once a client has deleted it. The refusal stores nothing.
//! * a paginated read that pages a group's attachments against the page size the request states
//!   needs the two members together, so a `NextToken` sent with no `MaxRecords` is refused with
//!   a sentence about that PAIR, carrying nothing of the request in it — not the token, not the
//!   group, not the read — and answered before the token is examined; the same request without
//!   the token is served.
//!
//! Both halves are refusals under HTTP 400 and the service-common `ValidationError`, which is
//! this service's one code for a client error, so the message is the whole of what tells the
//! caller which rule it broke: an id it must stop referencing versus a template it must
//! re-create, and a page size it must state beside its token versus a token it must drop.
//!
//! Auto Scaling and Amazon EC2 are mounted on ONE engine and driven over their own wire
//! protocols, so every launch template and subnet here is written where a client writes it.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, create_launch_template, delete_launch_template, emulator, named_list, p, query,
    seed_subnets, tag_value, tag_values, xml_unescape, SeededSubnet,
};

/// The code this service answers every client error under.
const VALIDATION_ERROR: &str = "ValidationError";

/// The code the reads that treat an unusable token as a modelled fault answer instead.
const INVALID_NEXT_TOKEN: &str = "InvalidNextToken";

/// The `(code, message)` of a refusal, with the message unescaped as a client reads it.
fn refusal(body: &str) -> (String, String) {
    (
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// Create one group from `template`, placed in the seeded subnet, holding no capacity.
async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    subnet: &SeededSubnet,
    template_id: &str,
) {
    let params = vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "0"),
        p("AvailabilityZones.member.1", subnet.zone.as_str()),
        p("VPCZoneIdentifier", subnet.id.as_str()),
        p("LaunchTemplate.LaunchTemplateId", template_id),
        p("LaunchTemplate.Version", "$Latest"),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the group {name:?} must be created, got {status}: {body}"
    );
}

/// The names `DescribeAutoScalingGroups` reports for one group name.
async fn groups_named(router: &MultiServiceRouter, name: &str) -> Vec<String> {
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
    tag_values(&body, "AutoScalingGroupName")
}

/// Launch template id FORMS the mint cannot have produced, from that service's own screen of
/// the member: an id of that service is an obfuscated identifier it decodes before it resolves
/// anything, printed as `lt-` and 17 lowercase hexadecimal digits under a leading `0`, so a
/// value above the one its counter has reached — and the all-zero placeholder a counter
/// starting at one can never write — is an identifier it never issued. Each reads like a
/// printed id and names no launch template.
const UNISSUED_IDS: &[(&str, &str)] = &[
    (
        "a printed id far above every value the mint can reach",
        "lt-000000000000fffff",
    ),
    (
        "an id whose digits do not fit the width the mint counts in",
        "lt-0ffffffffffffffff",
    ),
    (
        "an id of the documentation shape no mint ever answered with",
        "lt-0123456789abcdef0",
    ),
    (
        "the all-zero placeholder, which a mint counting from one cannot write",
        "lt-00000000000000000",
    ),
];

/// The `Version` values a launch source may state beside the id, including none at all: the
/// id is screened BEFORE the version is resolved, so every row earns the same sentence
/// ("the default value is `$Default`",
/// `smithy_query(query="shape",shape="LaunchTemplateSpecification")`).
const VERSIONS: &[Option<&str>] = &[None, Some("$Latest"), Some("$Default"), Some("1")];

/// RULE: a `LaunchTemplateId` the launch-template service's mint never issued is refused as a
/// value that service cannot READ, whatever version the launch source states beside it and
/// whatever became of the templates the account does hold — while an id that mint issued is
/// resolved, so the two verdicts a caller must act on differently stay apart.
#[tokio::test]
async fn an_unissued_launch_template_id_is_refused_as_unreadable_before_any_version_is_resolved() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let subnet = &subnets[0];
    let (live_id, _version) = create_launch_template(&router, "reference-screen-live").await;
    let (deleted_id, _deleted_version) =
        create_launch_template(&router, "reference-screen-deleted").await;
    delete_launch_template(&router, "the deleted template", &deleted_id).await;

    // (case, the id stated, the sentence it earns, or `None` where the create must be served)
    let mut rows: Vec<(String, String, Option<String>)> = UNISSUED_IDS
        .iter()
        .map(|(case, id)| {
            (
                (*case).to_string(),
                (*id).to_string(),
                Some(format!("Malformed launch template Id: {id}")),
            )
        })
        .collect();
    rows.push((
        "an id the mint issued, whose template a client has deleted".to_string(),
        deleted_id.clone(),
        Some(format!(
            "The specified launch template, with template ID {deleted_id}, does not exist."
        )),
    ));
    rows.push((
        "an id the mint issued, whose template the account still holds".to_string(),
        live_id.clone(),
        None,
    ));

    for (row, (case, id, expected)) in rows.iter().enumerate() {
        for (at, version) in VERSIONS.iter().enumerate() {
            let case = format!("{case} with Version {version:?}");
            let name = format!("reference-screen-group-{row}-{at}");
            let mut params = vec![
                p("AutoScalingGroupName", name.as_str()),
                p("MinSize", "0"),
                p("MaxSize", "1"),
                p("DesiredCapacity", "0"),
                p("AvailabilityZones.member.1", subnet.zone.as_str()),
                p("VPCZoneIdentifier", subnet.id.as_str()),
                p("LaunchTemplate.LaunchTemplateId", id.as_str()),
            ];
            if let Some(version) = version {
                params.push(p("LaunchTemplate.Version", *version));
            }
            let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;

            match expected {
                Some(expected) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status}: {body}"
                    );
                    let answered = refusal(&body);
                    assert_eq!(
                        answered,
                        (VALIDATION_ERROR.to_string(), expected.clone()),
                        "case {case:?}: expected {VALIDATION_ERROR} {expected:?}, got \
                         {answered:?}"
                    );
                    let reported = groups_named(&router, &name).await;
                    assert!(
                        reported.is_empty(),
                        "case {case:?}: the refused create must leave no group, got {reported:?}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an id the account holds must be resolved, got {status}: \
                         {body}"
                    );
                    // Read the created group back: it launches from the template the request
                    // named, by the id the mint issued.
                    let (read_status, read_body) = query(
                        &router,
                        "DescribeAutoScalingGroups",
                        &[("AutoScalingGroupNames.member.1", name.as_str())],
                    )
                    .await;
                    assert_eq!(
                        read_status, 200,
                        "case {case:?}: DescribeAutoScalingGroups expected 200, got \
                         {read_status}: {read_body}"
                    );
                    let read_id = tag_value(&read_body, "LaunchTemplateId");
                    assert_eq!(
                        read_id, *live_id,
                        "case {case:?}: the created group must launch from {live_id:?}, got \
                         {read_id:?}"
                    );
                }
            }
        }
    }
}

/// The reads that page a group's attachments against the page size the request states, so a
/// token sent without one is refused for the PAIR — each beside the list element it reports
/// its page of attachments as, which is what the same request WITHOUT a token answers.
const PAIRED_READS: &[(&str, &str)] = &[
    ("DescribeLoadBalancers", "LoadBalancers"),
    ("DescribeTrafficSources", "TrafficSources"),
];

/// The tokens the pairing rule is read over: the token's own value cannot change the verdict,
/// because the pair is read before the token is. They span a value of no form this service
/// issues, a form it does issue (its marker and an offset), the first page's offset (never
/// issued as a token) and a value carrying characters a form body has to encode.
const TOKENS: &[&str] = &[
    "sim2real-not-a-real-next-token-xyz123",
    "not-a-token",
    "next-1",
    "next-0",
    "a token with spaces & an ampersand",
];

/// RULE: a resumption token sent to a read that pages on `MaxRecords` without that page size
/// is refused with a sentence about the two members and nothing of the request — the same
/// sentence for every token, every group and both reads — while the same request without the
/// token is served and a read that pages on no such pair answers for the token's own value.
#[tokio::test]
async fn a_token_without_a_page_size_is_refused_with_a_sentence_carrying_none_of_the_request() {
    const WITHOUT_PAGE_SIZE: &str = "Cannot specify NextToken without MaxRecords";

    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let subnet = &subnets[0];
    let (template_id, _version) = create_launch_template(&router, "token-pairing-template").await;

    // Two groups, whose names differ in length and in content, so a sentence quoting the group
    // could not pass as the fixed one.
    let groups = [
        "token-pairing-a",
        "token-pairing-group-with-a-considerably-longer-name",
    ];
    for group in groups {
        create_group(&router, "the groups under test", group, subnet, &template_id).await;
    }

    for group in groups {
        for (read, list) in PAIRED_READS {
            for token in TOKENS {
                let case = format!("{read} on {group:?} with the token {token:?} and no MaxRecords");
                let params = vec![p("AutoScalingGroupName", group), p("NextToken", *token)];
                let (status, body) = query(&router, read, &borrowed(&params)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status}: {body}"
                );
                let answered = refusal(&body);
                assert_eq!(
                    answered,
                    (VALIDATION_ERROR.to_string(), WITHOUT_PAGE_SIZE.to_string()),
                    "case {case:?}: expected {VALIDATION_ERROR} {WITHOUT_PAGE_SIZE:?}, got \
                     {answered:?}"
                );
                // The sentence has no variable part: nothing the request carried is in it.
                for carried in [*token, group, *read] {
                    assert!(
                        !answered.1.contains(carried),
                        "case {case:?}: the sentence must carry none of the request, but \
                         {carried:?} is in {:?}",
                        answered.1
                    );
                }

                // The same token BESIDE a page size is read for its own value instead, so the
                // pairing refusal is the pair's and not the token's.
                let paged = vec![
                    p("AutoScalingGroupName", group),
                    p("NextToken", *token),
                    p("MaxRecords", "1"),
                ];
                let (status, body) = query(&router, read, &borrowed(&paged)).await;
                let case = format!("{case} (with MaxRecords stated)");
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status}: {body}"
                );
                let answered = refusal(&body);
                assert_eq!(
                    answered,
                    (
                        VALIDATION_ERROR.to_string(),
                        format!("NextToken: {token}")
                    ),
                    "case {case:?}: expected the token's own value verdict, got {answered:?}"
                );
            }

            // Without the token the read is served, and NAMES the group's attachments as a
            // list — empty on a group attached to nothing — rather than refusing.
            let (status, body) = query(&router, read, &[("AutoScalingGroupName", group)]).await;
            let case = format!("{read} on {group:?} with no NextToken");
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status}: {body}"
            );
            let reported = named_list(&case, &body, list);
            assert!(
                reported.is_empty(),
                "case {case:?}: a group attached to nothing must report an empty {list}, got \
                 {reported:?}"
            );
        }
    }

    // A read that pages on no such pair answers the token's own value under its own modelled
    // fault, so the two verdicts a caller must act on differently stay apart. It is scoped to
    // the account rather than to a group, so the tokens are driven through it once.
    for token in TOKENS {
        let (status, body) = query(
            &router,
            "DescribeAutoScalingInstances",
            &[("NextToken", token)],
        )
        .await;
        let case = format!("DescribeAutoScalingInstances with the token {token:?}");
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        let answered = refusal(&body);
        assert_eq!(
            answered,
            (
                INVALID_NEXT_TOKEN.to_string(),
                format!("The token '{token}' is invalid")
            ),
            "case {case:?}: expected the token's own verdict, got {answered:?}"
        );
    }
}
