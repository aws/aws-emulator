// SPDX-License-Identifier: CC0-1.0

//! What `DescribeTargetGroups` NAMES when it resolves none of the identifiers it was given,
//! over the wire.
//!
//! One rule: a read that SELECTS target groups is refused ABOUT THE SELECTION rather than
//! about an identifier in it — the collection-level sentence `One or more target groups not
//! found`, fixed text naming no value of the request — whichever of its selectors carried the
//! identifiers, since the refusal is a verdict on the selection and not a report about one
//! resource. It is the wording this service's sibling list reads answer, while an operation
//! addressing ONE target group through its own required `TargetGroupArn` quotes that
//! identifier, which `target_group_attributes_and_modification` pins.
//!
//! The cases span what the rule generalises over: which selector carried the identifier, how
//! many did not resolve, whether some of them DID resolve, and whether the target group never
//! existed or was deleted — the last being the case a client meets when the stack that
//! deployed it is torn down and it reads the identifier that stack published.
//!
//! The wire harness (`router`, `call`, `action`, `p`) comes from `wire` and the message reader
//! from `target_group_attributes_and_modification`; nothing here restates them.

use smithy_server_runtime::MultiServiceRouter;

use crate::target_group_attributes_and_modification::unescaped;
use crate::wire::{action, call, p, router, Reply};

/// The awsQuery code this read is refused with, and the status it is answered under.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";
const REFUSED_STATUS: u16 = 400;

/// The sentence a selection this read could not resolve is refused with, spelled out here
/// rather than imported so a wrong constant in the implementation cannot make this suite pass.
const SELECTION_NOT_FOUND: &str = "One or more target groups not found";

/// Create a target group called `name` and answer its ARN.
async fn create_target_group(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-12345678"));
    let created = call(router, &form).await;
    assert_eq!(
        created.status, 200,
        "case setup {name}: expected the create to be served, got {} body {}",
        created.status, created.body
    );
    created
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case setup {name}: no ARN in {}", created.body))
}

/// Read by the `(member, value)` selectors `selectors` names.
async fn describe(router: &MultiServiceRouter, selectors: &[(&str, &str)]) -> Reply {
    let mut form = action("DescribeTargetGroups");
    for (member, value) in selectors {
        form.push(p(member, value));
    }
    call(router, &form).await
}

/// Assert that the read by `selectors` is refused with the collection-level sentence.
async fn assert_refused(router: &MultiServiceRouter, case: &str, selectors: &[(&str, &str)]) {
    let reply = describe(router, selectors).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (REFUSED_STATUS, TARGET_GROUP_NOT_FOUND),
        "case {case:?}: expected {REFUSED_STATUS} {TARGET_GROUP_NOT_FOUND}, got {} body {}",
        reply.status,
        reply.body
    );
    let stated = reply
        .text("Message")
        .map(|text| unescaped(&text))
        .unwrap_or_else(|| panic!("case {case:?}: no message in {}", reply.body));
    assert_eq!(
        stated, SELECTION_NOT_FOUND,
        "case {case:?}: expected {SELECTION_NOT_FOUND:?}, got {stated:?}"
    );
}

/// A well-formed target-group ARN of this service and this Region that names nothing the store
/// holds — so the read gets as far as the store and answers for the target group it did not
/// find rather than for the shape of the value.
fn absent_arn(id: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{}:{}:targetgroup/ghost/{id}",
        crate::provisioning_loop::REGION,
        crate::provisioning_loop::account()
    )
}

#[tokio::test]
async fn a_target_group_read_is_refused_about_the_selection_it_could_not_resolve() {
    let router = router();

    let held = create_target_group(&router, "named-not-found-tg").await;
    let first = absent_arn("0123456789abcdef");
    let second = absent_arn("fedcba9876543210");

    // One identifier, on each of the two selectors.
    assert_refused(&router, "one absent name", &[("Names.member.1", "no-such-tg")]).await;
    assert_refused(
        &router,
        "one absent ARN",
        &[("TargetGroupArns.member.1", &first)],
    )
    .await;
    // Two of them, on each selector.
    assert_refused(
        &router,
        "two absent names",
        &[
            ("Names.member.1", "no-such-tg"),
            ("Names.member.2", "nor-this-one"),
        ],
    )
    .await;
    assert_refused(
        &router,
        "two absent ARNs",
        &[
            ("TargetGroupArns.member.1", &first),
            ("TargetGroupArns.member.2", &second),
        ],
    )
    .await;
    // The target group that IS held does not make the read serve: a selection is refused when
    // any entry of it goes unresolved, and still names none of them.
    assert_refused(
        &router,
        "an ARN that resolves beside one that does not",
        &[
            ("TargetGroupArns.member.1", &held),
            ("TargetGroupArns.member.2", &first),
        ],
    )
    .await;
    assert_refused(
        &router,
        "a name that resolves beside one that does not",
        &[
            ("Names.member.1", "named-not-found-tg"),
            ("Names.member.2", "no-such-tg"),
        ],
    )
    .await;

    // The read that resolves is served, so the refusals above are about the identifiers and
    // not about the selector.
    let served = describe(&router, &[("TargetGroupArns.member.1", &held)]).await;
    assert_eq!(
        (served.status, served.text("TargetGroupArn").as_deref()),
        (200, Some(held.as_str())),
        "case the target group that is held: expected it to be reported, got body {}",
        served.body
    );

    // A target group that was DELETED is one the read no longer resolves — the case a client
    // meets when it reads an identifier whose resource is gone — under either selector.
    let mut form = action("DeleteTargetGroup");
    form.push(p("TargetGroupArn", &held));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected it to be served, got {} body {}",
        deleted.status, deleted.body
    );
    assert_refused(
        &router,
        "the ARN of a deleted target group",
        &[("TargetGroupArns.member.1", &held)],
    )
    .await;
    assert_refused(
        &router,
        "the name of a deleted target group",
        &[("Names.member.1", "named-not-found-tg")],
    )
    .await;
}
