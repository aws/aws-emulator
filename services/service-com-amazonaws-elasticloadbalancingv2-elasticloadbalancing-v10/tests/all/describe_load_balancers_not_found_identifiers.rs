// SPDX-License-Identifier: CC0-1.0

//! What `DescribeLoadBalancers` NAMES when it resolves no load balancer, over the wire.
//!
//! One rule, shared with the sibling read of target groups (`DescribeTargetGroups`, read in
//! `list_read_not_found_identifiers.rs`): a read that selects by a LIST of identifiers refuses
//! naming the identifiers it could not resolve, under the plural noun and rendered as the list
//! the request carried — `Load balancers '[<identifier>]' not found` — whichever of its two
//! selectors carried them, since `LoadBalancerArns` and `Names` select the same resource by
//! two spellings of its identity. A client acting on the refusal has to know WHICH of the
//! identifiers it asked about is missing, which a sentence naming none of them does not tell
//! it; an operation addressing ONE load balancer through its own required `LoadBalancerArn`
//! names that identifier with no list around it, which `tag_not_found_wording` pins.
//!
//! The rows span what the rule generalises over: which selector carried the identifier, how
//! many identifiers did not resolve, whether some of them DID resolve, and whether the load
//! balancer never existed or was deleted — the last being the case a client meets when the
//! stack that deployed it is torn down and it reads the ARN that stack published.
//!
//! The wire harness (`router`, `call`, `action`, `p`) comes from `wire` and the message reader
//! from `target_group_attributes_and_modification`; nothing here restates them.

use smithy_server_runtime::MultiServiceRouter;

use crate::target_group_attributes_and_modification::unescaped;
use crate::wire::{action, call, p, router, Reply};

/// The awsQuery code a read that resolves no load balancer is refused with, and the status it
/// is answered under.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";
const REFUSED_STATUS: u16 = 400;

/// Create a load balancer called `name` and answer its ARN.
async fn create(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let created = call(router, &form).await;
    assert_eq!(
        created.status, 200,
        "case setup {name}: expected the create to be served, got {} body {}",
        created.status, created.body
    );
    created
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case setup {name}: no ARN in {}", created.body))
}

/// Read by the `(member, value)` selectors `selectors` names.
async fn describe(router: &MultiServiceRouter, selectors: &[(&str, &str)]) -> Reply {
    let mut form = action("DescribeLoadBalancers");
    for (member, value) in selectors {
        form.push(p(member, value));
    }
    call(router, &form).await
}

/// Assert that the read by `selectors` is refused naming exactly `identifiers`.
async fn assert_names(
    router: &MultiServiceRouter,
    case: &str,
    selectors: &[(&str, &str)],
    identifiers: &[&str],
) {
    let reply = describe(router, selectors).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (REFUSED_STATUS, LOAD_BALANCER_NOT_FOUND),
        "case {case:?}: expected {REFUSED_STATUS} {LOAD_BALANCER_NOT_FOUND}, got {} body {}",
        reply.status,
        reply.body
    );
    let stated = reply
        .text("Message")
        .map(|text| unescaped(&text))
        .unwrap_or_else(|| panic!("case {case:?}: no message in {}", reply.body));
    let expected = format!("Load balancers '[{}]' not found", identifiers.join(", "));
    assert_eq!(
        stated, expected,
        "case {case:?}: expected {expected:?}, got {stated:?}"
    );
}

/// A well-formed ARN of this service, this Region and this resource form that names a load
/// balancer nothing created — so the read gets as far as the store and answers for the load
/// balancer it did not find rather than for the shape of the value.
fn absent_arn(id: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{}:{}:loadbalancer/app/ghost/{id}",
        crate::provisioning_loop::REGION,
        crate::provisioning_loop::account()
    )
}

#[tokio::test]
async fn a_read_that_resolves_nothing_names_the_identifiers_it_could_not_resolve() {
    let router = router();

    let held = create(&router, "named-not-found-lb").await;
    let first = absent_arn("0123456789abcdef");
    let second = absent_arn("fedcba9876543210");

    // One identifier, on each of the two selectors.
    assert_names(
        &router,
        "one absent name",
        &[("Names.member.1", "no-such-alb")],
        &["no-such-alb"],
    )
    .await;
    assert_names(
        &router,
        "one absent ARN",
        &[("LoadBalancerArns.member.1", &first)],
        &[&first],
    )
    .await;
    // Two of them, in the order the request carried them.
    assert_names(
        &router,
        "two absent names",
        &[
            ("Names.member.1", "no-such-alb"),
            ("Names.member.2", "nor-this-one"),
        ],
        &["no-such-alb", "nor-this-one"],
    )
    .await;
    assert_names(
        &router,
        "two absent ARNs",
        &[
            ("LoadBalancerArns.member.1", &first),
            ("LoadBalancerArns.member.2", &second),
        ],
        &[&first, &second],
    )
    .await;
    // And the load balancer that IS held does not make the read serve: the refusal names
    // only what did not resolve.
    assert_names(
        &router,
        "an ARN that resolves beside one that does not",
        &[
            ("LoadBalancerArns.member.1", &held),
            ("LoadBalancerArns.member.2", &first),
        ],
        &[&first],
    )
    .await;

    // The read that resolves is served, so the refusals above are about the identifiers and
    // not about the selector.
    let served = describe(&router, &[("LoadBalancerArns.member.1", &held)]).await;
    assert_eq!(
        (served.status, served.text("LoadBalancerArn").as_deref()),
        (200, Some(held.as_str())),
        "case the load balancer that is held: expected it to be reported, got body {}",
        served.body
    );

    // A load balancer that was DELETED is one the read no longer resolves — the case a client
    // meets when it reads an ARN whose load balancer is gone.
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &held));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected it to be served, got {} body {}",
        deleted.status, deleted.body
    );
    assert_names(
        &router,
        "the ARN of a deleted load balancer",
        &[("LoadBalancerArns.member.1", &held)],
        &[&held],
    )
    .await;
}
