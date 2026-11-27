// SPDX-License-Identifier: CC0-1.0

//! What `DescribeLoadBalancers` NAMES when it resolves no load balancer, over the wire.
//!
//! One rule, by the SELECTOR the request used rather than by the operation: a selection of
//! load balancers by `LoadBalancerArns` is refused ABOUT THE SELECTION — the collection-level
//! sentence `One or more load balancers not found`, fixed text that names no value of the
//! request — while a selection by `Names` quotes the names it could not resolve, under the
//! plural noun and rendered as the list the request carried (`Load balancers '[<name>]' not
//! found`). The ARN form is therefore worded exactly as this service's other list reads and as
//! a tag call naming a load balancer are (`tag_not_found_wording`), and the name form is the
//! one lookup here whose refusal carries a value of the request at all; an operation
//! addressing ONE load balancer through its own required `LoadBalancerArn` quotes that
//! identifier, which `tag_not_found_wording` pins too.
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

/// The sentence a selection by ARN is refused with, whatever it selected: the collection-level
/// wording, spelled out here rather than imported so a wrong constant in the implementation
/// cannot make this suite pass.
const SELECTION_NOT_FOUND: &str = "One or more load balancers not found";

/// The sentence a selection by NAME is refused with: the same plural noun, carrying the names
/// that did not resolve as the list the request sent them as.
fn names_not_found(names: &[&str]) -> String {
    format!("Load balancers '[{}]' not found", names.join(", "))
}

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

/// Assert that the read by `selectors` is refused with exactly `expected`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    selectors: &[(&str, &str)],
    expected: &str,
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
async fn a_selection_by_arn_is_refused_about_the_selection() {
    let router = router();

    let held = create(&router, "arn-not-found-lb").await;
    let first = absent_arn("0123456789abcdef");
    let second = absent_arn("fedcba9876543210");

    // One ARN, two ARNs, and an ARN that resolves beside one that does not: the sentence is
    // the same one, so nothing of the request reaches the caller through it.
    assert_refused(
        &router,
        "one absent ARN",
        &[("LoadBalancerArns.member.1", &first)],
        SELECTION_NOT_FOUND,
    )
    .await;
    assert_refused(
        &router,
        "two absent ARNs",
        &[
            ("LoadBalancerArns.member.1", &first),
            ("LoadBalancerArns.member.2", &second),
        ],
        SELECTION_NOT_FOUND,
    )
    .await;
    assert_refused(
        &router,
        "an ARN that resolves beside one that does not",
        &[
            ("LoadBalancerArns.member.1", &held),
            ("LoadBalancerArns.member.2", &first),
        ],
        SELECTION_NOT_FOUND,
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
    // meets when it reads an ARN whose load balancer is gone — and its own ARN is not echoed
    // back either.
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &held));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected it to be served, got {} body {}",
        deleted.status, deleted.body
    );
    assert_refused(
        &router,
        "the ARN of a deleted load balancer",
        &[("LoadBalancerArns.member.1", &held)],
        SELECTION_NOT_FOUND,
    )
    .await;
}

#[tokio::test]
async fn a_selection_by_name_names_the_names_it_could_not_resolve() {
    let router = router();

    let held = create(&router, "named-not-found-lb").await;

    // One name, and two in the order the request carried them.
    assert_refused(
        &router,
        "one absent name",
        &[("Names.member.1", "no-such-alb")],
        &names_not_found(&["no-such-alb"]),
    )
    .await;
    assert_refused(
        &router,
        "two absent names",
        &[
            ("Names.member.1", "no-such-alb"),
            ("Names.member.2", "nor-this-one"),
        ],
        &names_not_found(&["no-such-alb", "nor-this-one"]),
    )
    .await;
    // A name that resolves beside one that does not: only what did not resolve is named.
    assert_refused(
        &router,
        "a name that resolves beside one that does not",
        &[
            ("Names.member.1", "named-not-found-lb"),
            ("Names.member.2", "no-such-alb"),
        ],
        &names_not_found(&["no-such-alb"]),
    )
    .await;

    // The read by a name that resolves is served.
    let served = describe(&router, &[("Names.member.1", "named-not-found-lb")]).await;
    assert_eq!(
        (served.status, served.text("LoadBalancerArn").as_deref()),
        (200, Some(held.as_str())),
        "case the load balancer that is held: expected it to be reported, got body {}",
        served.body
    );

    // The name of a DELETED load balancer is one the read no longer resolves, and the name is
    // named — the same selector, the same wording, whatever took the load balancer away.
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &held));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected it to be served, got {} body {}",
        deleted.status, deleted.body
    );
    assert_refused(
        &router,
        "the name of a deleted load balancer",
        &[("Names.member.1", "named-not-found-lb")],
        &names_not_found(&["named-not-found-lb"]),
    )
    .await;

    // And the two selectors do NOT answer one wording: the ARN form of the very same absent
    // load balancer is refused about the selection instead.
    assert_refused(
        &router,
        "the ARN of the same deleted load balancer",
        &[("LoadBalancerArns.member.1", &held)],
        SELECTION_NOT_FOUND,
    )
    .await;
}
