// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `NotFound` a tag operation answers for a resource the store does
//! not hold.
//!
//! The rule under test is not one call: Elastic Load Balancing words this refusal per
//! resource KIND, so each kind keeps its own template and all three tag operations
//! (`AddTags`, `RemoveTags` and `DescribeTags`, which all take `ResourceArns`) answer the
//! same words for the same kind — the load balancer, listener, rule and trust store kinds
//! naming no value, the target group kind quoting the entry that did not resolve. The
//! catalog of those templates is the sibling suite's
//! ([`crate::tag_not_found_wording_by_kind::tag_not_found_message`]), so the two suites
//! cannot drift on what a kind answers. So the table below spans what the rule generalizes
//! over: all three tag operations, every taggable kind, several ARN forms per kind
//! (including a load balancer that was created and deleted), and selections of one, several
//! and a live resource beside an absent one — the recorded call (one deleted load balancer
//! named by `AddTags`) is one row of it.
//!
//! The boundary is asserted too: an operation that addresses exactly ONE resource through
//! its own required identifier still quotes that identifier, whatever its kind.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form body
//! in, an `<ErrorResponse>` envelope out — and the helpers are the ones the sibling tag
//! suites already carry (`provisioning_loop`, `resource_tags`, `tag_writes`,
//! `tag_removal`), so the calls under test are built exactly as those suites build them.

use crate::provisioning_loop::{action, call, create_http_target_group, emulator, p, Reply};
use crate::resource_tags::{absent_arn, describe_tags};
use crate::tag_not_found_wording_by_kind::{names_the_identifier, tag_not_found_message};
use crate::tag_removal::remove_tags;
use crate::tag_writes::{add_tags, message, owned, stored};
use crate::target_group_attributes_and_modification::unescaped;
use smithy_server_runtime::MultiServiceRouter;

/// The three operations whose resource selector is the `ResourceArns` LIST
/// (`smithy_query(query="operation",operation="AddTags,RemoveTags,DescribeTags")`: each
/// takes `ResourceArns` of type list, and each models the five per-kind `NotFound`
/// errors).
const TAG_OPERATIONS: [&str; 3] = ["AddTags", "RemoveTags", "DescribeTags"];

/// One row of the table: the case, the awsQuery error code of the `NotFound` shape the
/// kind is refused with, and ARNs of that kind that no resource in the store carries.
type AbsentRow = (&'static str, &'static str, Vec<String>);

/// One tag operation over `arns`. The tags and keys are the ones the resolution pass never
/// reaches on a refusal, so every operation refuses on the same selection.
async fn tag_call(router: &MultiServiceRouter, operation: &str, arns: &[String]) -> Reply {
    match operation {
        "AddTags" => add_tags(router, arns, &owned(&[("after", "delete")])).await,
        "RemoveTags" => remove_tags(router, arns, &["after".to_string()]).await,
        "DescribeTags" => describe_tags(router, arns).await,
        other => panic!("case {other:?}: this suite drives only the tag operations"),
    }
}

// ---------------------------------------------------------------------------
// Every tag operation, every kind, every selection: the collection wording.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tag_operation_refuses_an_absent_resource_with_the_wording_of_its_kind() {
    let (router, _engine) = emulator();

    // A live resource, so a selection can mix one that resolves with one that does not,
    // and so the suite can witness that a refused write stored nothing.
    let live = create_http_target_group(&router, "tagword-live-tg").await;
    let live_tags = owned(&[("env", "prod")]);
    assert_eq!(
        add_tags(&router, std::slice::from_ref(&live), &live_tags)
            .await
            .status,
        200,
        "case tagging the live target group: expected 200"
    );

    // A load balancer that was created and then DELETED: the recorded row, and the one no
    // "it never existed" ARN can stand in for.
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "tagword-deleted-lb"));
    form.push(p("Subnets.member.1", "subnet-0123456789abcdef0"));
    form.push(p("Subnets.member.2", "subnet-0123456789abcdef1"));
    let created = call(&router, &form).await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateLoadBalancer answered {} body {}",
        created.status, created.body
    );
    let deleted = created
        .text("LoadBalancerArn")
        .expect("case setup: CreateLoadBalancer must report a LoadBalancerArn");
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &deleted));
    let gone = call(&router, &form).await;
    assert_eq!(
        gone.status, 200,
        "case setup: DeleteLoadBalancer answered {} body {}",
        gone.status, gone.body
    );

    // The ARN forms AWS publishes for this service's resources, grouped by the kind the
    // resource segment of the ARN declares.
    let rows: Vec<AbsentRow> = vec![
        (
            "load balancers",
            "LoadBalancerNotFound",
            vec![
                deleted.clone(),
                absent_arn("loadbalancer/app/absent/0123456789abcdef"),
                absent_arn("loadbalancer/net/absent/fedcba9876543210"),
                absent_arn("loadbalancer/gwy/absent/00112233445566aa"),
            ],
        ),
        (
            "target groups",
            "TargetGroupNotFound",
            vec![
                absent_arn("targetgroup/absent/0123456789abcdef"),
                absent_arn("targetgroup/nor-this-one/fedcba9876543210"),
            ],
        ),
        (
            "listeners",
            "ListenerNotFound",
            vec![
                absent_arn("listener/app/absent/0123456789abcdef/fedcba9876543210"),
                absent_arn("listener/net/nor-this-one/0123456789abcdef/00112233445566aa"),
            ],
        ),
        (
            "rules",
            "RuleNotFound",
            vec![
                absent_arn(
                    "listener-rule/app/absent/0123456789abcdef/fedcba9876543210/00112233445566aa",
                ),
                absent_arn(
                    "listener-rule/net/nor-this/fedcba9876543210/0123456789abcdef/aa6655443322110",
                ),
            ],
        ),
        (
            "trust stores",
            "TrustStoreNotFound",
            vec![
                absent_arn("truststore/absent/0123456789abcdef"),
                absent_arn("truststore/nor-this-one/fedcba9876543210"),
            ],
        ),
    ];

    for (kind, code, arns) in &rows {
        // The selections differ along what the rule generalizes over: which ARN of the
        // kind is named, how many are named at once, and whether one that resolves sits
        // beside them. The third element is the entry that does not resolve — the first
        // such entry in request order, which is the entry the refusal is about.
        let mut selections: Vec<(String, Vec<String>, String)> = arns
            .iter()
            .map(|arn| {
                (
                    format!("one {kind} ARN ({arn})"),
                    vec![arn.clone()],
                    arn.clone(),
                )
            })
            .collect();
        selections.push((
            format!("every absent {kind} ARN at once"),
            arns.clone(),
            arns[0].clone(),
        ));
        selections.push((
            format!("an absent {kind} ARN beside a live target group"),
            vec![live.clone(), arns[0].clone()],
            arns[0].clone(),
        ));

        for operation in TAG_OPERATIONS {
            for (selection, named, unresolved) in &selections {
                let case = format!("{operation} over {selection}");
                let expected = tag_not_found_message(code, unresolved);
                let refused = tag_call(&router, operation, named).await;
                assert_eq!(
                    (refused.status, refused.error_code().as_str()),
                    (400, *code),
                    "case {case:?}: expected 400 {code}, got {} body {}",
                    refused.status,
                    refused.body
                );
                // The framework's XML codec escapes the quotes a message carries, so the
                // VALUE is what is compared — through the crate's one reader for that
                // encoding.
                let said = unescaped(&message(&refused));
                assert_eq!(
                    said, expected,
                    "case {case:?}: expected the message {expected:?}, got {said:?}"
                );
                for arn in named {
                    let quoted = said.contains(arn.as_str());
                    let owed = names_the_identifier(code) && arn == unresolved;
                    assert_eq!(
                        quoted, owed,
                        "case {case:?}: expected the message to quote {arn} only when the \
                         kind's template names the unresolved identifier ({owed}), got {said:?}"
                    );
                }
            }
        }
    }

    // Every refusal above left the live resource exactly as it was: the wording changed,
    // not what a tag call does.
    let still = stored(&router, &live).await;
    assert_eq!(
        still,
        {
            let mut sorted = live_tags.clone();
            sorted.sort();
            sorted
        },
        "case the live target group survives every refusal: got {still:?}"
    );
}

// ---------------------------------------------------------------------------
// The boundary: one resource addressed through its own identifier is named.
// ---------------------------------------------------------------------------

/// One row: the case, the operation, the form fields that carry its single required
/// identifier, the awsQuery error code and the message the refusal must carry.
type AddressedRow = (&'static str, &'static str, Vec<(String, String)>, &'static str, String);

#[tokio::test]
async fn an_operation_addressing_one_resource_still_names_its_identifier() {
    let (router, _engine) = emulator();

    let absent_lb = absent_arn("loadbalancer/app/addressed/0123456789abcdef");
    let absent_tg = absent_arn("targetgroup/addressed/fedcba9876543210");
    let absent_listener = absent_arn("listener/app/addressed/0123456789abcdef/fedcba9876543210");

    let cases: Vec<AddressedRow> = vec![
        (
            "the attribute read of one load balancer",
            "DescribeLoadBalancerAttributes",
            vec![p("LoadBalancerArn", &absent_lb)],
            "LoadBalancerNotFound",
            format!("Load balancer '{absent_lb}' not found"),
        ),
        (
            "the attribute read of one target group",
            "DescribeTargetGroupAttributes",
            vec![p("TargetGroupArn", &absent_tg)],
            "TargetGroupNotFound",
            format!("Target groups '{absent_tg}' not found"),
        ),
        (
            "the attribute read of one listener",
            "DescribeListenerAttributes",
            vec![p("ListenerArn", &absent_listener)],
            "ListenerNotFound",
            format!("Listener '{absent_listener}' not found"),
        ),
    ];

    for (case, operation, fields, code, expected) in &cases {
        let mut form = action(operation);
        form.extend(fields.iter().cloned());
        let refused = call(&router, &form).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, *code),
            "case {case:?}: expected 400 {code}, got {} body {}",
            refused.status,
            refused.body
        );
        let said = unescaped(&message(&refused));
        assert_eq!(
            &said, expected,
            "case {case:?}: expected the message {expected:?}, got {said:?}"
        );
    }

    // The same selection shape on a read that is NOT a tag call: a tag operation addressing a
    // load balancer names no value of the request, and `DescribeLoadBalancers` over the ARN
    // selector answers that same collection-level sentence — the read quotes an identifier
    // back only through its `Names` selector, which is asserted here beside it so neither
    // wording can be changed into the other unnoticed.
    let mut form = action("DescribeLoadBalancers");
    form.push(p("LoadBalancerArns.member.1", &absent_lb));
    let refused = call(&router, &form).await;
    let said = unescaped(&message(&refused));
    let tagged = tag_not_found_message("LoadBalancerNotFound", &absent_lb);
    assert_eq!(
        (refused.status, refused.error_code().as_str(), said.as_str()),
        (400, "LoadBalancerNotFound", tagged.as_str()),
        "case a selection of load balancers by ARN: got {} body {}",
        refused.status,
        refused.body
    );

    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", "tagword-no-such-lb"));
    let refused = call(&router, &form).await;
    let said = unescaped(&message(&refused));
    let expected = "Load balancers '[tagword-no-such-lb]' not found";
    assert_eq!(
        (refused.status, refused.error_code().as_str(), said.as_str()),
        (400, "LoadBalancerNotFound", expected),
        "case a selection of load balancers by name: got {} body {}",
        refused.status,
        refused.body
    );
    assert_ne!(
        said, tagged,
        "case a selection of load balancers by name: the read names the name where a tag \
         operation names none, got {said:?} for both"
    );
}
