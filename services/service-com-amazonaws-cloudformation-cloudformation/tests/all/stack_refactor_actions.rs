// SPDX-License-Identifier: CC0-1.0

//! The ACTIONS one stack refactor concluded, end to end over the awsQuery wire:
//! `CreateStackRefactor` plans a refactor and `ListStackRefactorActions` reports "the stack
//! refactor actions that will be taken after calling the ExecuteStackRefactor action".
//!
//! Four rules are under test here, each with its own table.
//!
//! 1. **A plan's actions are reported in the order CloudFormation carries them out.** Every
//!    stack the refactor has to create comes before every move, because a stack has to exist
//!    before a resource can be moved into it; the moves follow in the order the refactor names
//!    them. A move the request named is the customer's (`MANUAL`), and one the plan worked out
//!    from the final states the request submitted is CloudFormation's own (`AUTO`) — and a
//!    mapping the request named leaves nothing to work out, so it is reported ONCE. Each move
//!    names the stack at either end by that stack's own id, and names the physical instance it
//!    acts on, which is the one the stack that holds the resource reports for it.
//!
//! 2. **A refactor with no plan reports no actions.** A creation still in flight has worked
//!    nothing out yet and a creation that FAILED leaves nothing to execute, so each answers an
//!    empty list rather than actions nothing will carry out.
//!
//! 3. **The actions are paged as the request asks.** `MaxResults` bounds one page and the
//!    `NextToken` resumes the walk just past it, so walking a refactor's actions a page at a
//!    time reports each action exactly once; a token another refactor's walk minted, and a
//!    value this service never issued, are each refused.
//!
//! 4. **A refactor is addressable by its own id, in its own Region.** An id nothing created,
//!    and the id of a refactor another Region holds, are each answered with the modelled
//!    `StackRefactorNotFoundException` (HTTP 404).
//!
//! Each table spans the class its rule generalises over rather than one recorded case: a
//! refactor with one move and with two, with a move into a stack that exists and into one the
//! refactor creates, with a mapping the request named and with one nobody named, over a
//! resource identified by its ARN and one identified by its name, and a refactor with nothing
//! to do at all.

use service_com_amazonaws_cloudformation_cloudformation::state::stack_refactor_actions::{
    action, detection, entity,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{deploy, error_code};
use crate::harness::{
    already_due, call, call_in, member, router, router_with_engine, still_running, store,
    wait_for_the_transition, xml_blocks, xml_text, REGION,
};
use crate::stack_refactors::{
    create_stack_refactor, Definition, Mapping, NO_RESOURCES, ONE_TOPIC, TOPIC_AND_BUCKET,
};

/// A second Region, for the case that pins a refactor to the Region it was created in.
const OTHER_REGION: &str = "us-west-2";

/// The final state that gives the moved topic a DIFFERENT logical id in the stack that is to
/// hold it, which is what makes the move one nobody named: the resource leaves one stack's
/// final state and arrives in another's under a new name.
const RENAMED_TOPIC: &str = r#"{"Resources":{"RenamedTopic":{"Type":"AWS::SNS::Topic"}}}"#;

/// The property a resource of each type this suite moves is identified BY, which is the key a
/// `ResourceIdentifier` carries: "the key is an identifier property (for example,
/// `BucketName` for `AWS::S3::Bucket` resources)".
///
/// Source: each type's documented return values — `Ref` returns the topic ARN for
/// `AWS::SNS::Topic`, so a topic is identified by that ARN, while a bucket is identified by
/// the name property its declaration carries.
const TOPIC_IDENTIFIER: &str = "TopicArn";
const BUCKET_IDENTIFIER: &str = "BucketName";

/// One end of an expected move: the stack the row deployed or named, and the logical resource
/// id in it.
#[derive(Debug, Clone, Copy)]
struct ExpectedEnd {
    stack: &'static str,
    logical_id: &'static str,
}

/// One end of a move, as a row spells it.
const fn at(stack: &'static str, logical_id: &'static str) -> ExpectedEnd {
    ExpectedEnd { stack, logical_id }
}

/// The move an expected action carries out: "the current source of the resource and the
/// destination of where it will be moved to".
#[derive(Debug, Clone, Copy)]
struct ExpectedMove {
    source: ExpectedEnd,
    destination: ExpectedEnd,
}

/// What a row expects of ONE reported action.
#[derive(Debug, Clone, Copy)]
struct ExpectedAction {
    action: &'static str,
    entity: &'static str,
    /// `MANUAL` for a move the request named, `AUTO` for one the plan worked out, and `None`
    /// for an action that carries no mapping at all.
    detection: Option<&'static str>,
    /// The move the action carries, where it carries one.
    mapping: Option<ExpectedMove>,
    /// The identifier property the action's `ResourceIdentifier` must key the instance by,
    /// and `None` for an action on a stack that does not exist yet.
    identifier_property: Option<&'static str>,
    /// The stack a `CREATE`/`STACK` action is about.
    created_stack: Option<&'static str>,
}

impl ExpectedAction {
    /// The action that creates one stack the refactor names and this Region holds none of.
    const fn creating(stack_name: &'static str) -> Self {
        Self {
            action: action::CREATE,
            entity: entity::STACK,
            detection: None,
            mapping: None,
            identifier_property: None,
            created_stack: Some(stack_name),
        }
    }

    /// The action that moves one resource, as the refactor's plan concluded it.
    const fn moving(
        source: ExpectedEnd,
        destination: ExpectedEnd,
        detection: &'static str,
        identifier_property: &'static str,
    ) -> Self {
        Self {
            action: action::MOVE,
            entity: entity::RESOURCE,
            detection: Some(detection),
            mapping: Some(ExpectedMove {
                source,
                destination,
            }),
            identifier_property: Some(identifier_property),
            created_stack: None,
        }
    }
}

/// One row of the ordering table.
struct ActionsCase {
    case: &'static str,
    /// The stacks deployed before the refactor, one `(name, template)` each.
    stacks: &'static [(&'static str, &'static str)],
    description: Option<&'static str>,
    enable_stack_creation: Option<bool>,
    definitions: Vec<Definition>,
    mappings: Vec<Mapping>,
    /// The actions the read must report, in the order it must report them.
    expected: Vec<ExpectedAction>,
}

/// The rows of the ordering table.
fn actions_cases() -> Vec<ActionsCase> {
    vec![
        ActionsCase {
            case: "a move the request named, between two stacks that both exist",
            stacks: &[
                ("actions-source", ONE_TOPIC),
                ("actions-target", NO_RESOURCES),
            ],
            description: Some("move the topic across"),
            enable_stack_creation: Some(false),
            definitions: vec![
                Definition::inline("actions-source", NO_RESOURCES),
                Definition::inline("actions-target", ONE_TOPIC),
            ],
            mappings: vec![Mapping {
                source: ("actions-source", "Topic"),
                destination: ("actions-target", "Topic"),
            }],
            // The same move the final states imply is the one the request named, so it is
            // reported once and as the customer's.
            expected: vec![ExpectedAction::moving(
                at("actions-source", "Topic"),
                at("actions-target", "Topic"),
                detection::MANUAL,
                TOPIC_IDENTIFIER,
            )],
        },
        ActionsCase {
            case: "a move into a stack the refactor has to create",
            stacks: &[("actions-mover", ONE_TOPIC)],
            description: None,
            enable_stack_creation: Some(true),
            definitions: vec![
                Definition::inline("actions-mover", NO_RESOURCES),
                Definition::inline("actions-brand-new", ONE_TOPIC),
            ],
            mappings: vec![Mapping {
                source: ("actions-mover", "Topic"),
                destination: ("actions-brand-new", "Topic"),
            }],
            expected: vec![
                ExpectedAction::creating("actions-brand-new"),
                ExpectedAction::moving(
                    at("actions-mover", "Topic"),
                    at("actions-brand-new", "Topic"),
                    detection::MANUAL,
                    TOPIC_IDENTIFIER,
                ),
            ],
        },
        ActionsCase {
            case: "two moves, in the order the request named them",
            stacks: &[
                ("actions-pair", TOPIC_AND_BUCKET),
                ("actions-split", NO_RESOURCES),
            ],
            description: Some("split the pair"),
            enable_stack_creation: Some(false),
            definitions: vec![
                Definition::inline("actions-pair", NO_RESOURCES),
                Definition::inline("actions-split", TOPIC_AND_BUCKET),
            ],
            mappings: vec![
                Mapping {
                    source: ("actions-pair", "Alpha"),
                    destination: ("actions-split", "Alpha"),
                },
                Mapping {
                    source: ("actions-pair", "Bucket"),
                    destination: ("actions-split", "Bucket"),
                },
            ],
            expected: vec![
                ExpectedAction::moving(
                    at("actions-pair", "Alpha"),
                    at("actions-split", "Alpha"),
                    detection::MANUAL,
                    TOPIC_IDENTIFIER,
                ),
                ExpectedAction::moving(
                    at("actions-pair", "Bucket"),
                    at("actions-split", "Bucket"),
                    detection::MANUAL,
                    BUCKET_IDENTIFIER,
                ),
            ],
        },
        ActionsCase {
            case: "a move nobody named, worked out from the submitted final states",
            stacks: &[
                ("actions-renamer", ONE_TOPIC),
                ("actions-renamed", NO_RESOURCES),
            ],
            description: Some("rename the topic into another stack"),
            enable_stack_creation: Some(false),
            definitions: vec![
                Definition::inline("actions-renamer", NO_RESOURCES),
                Definition::inline("actions-renamed", RENAMED_TOPIC),
            ],
            mappings: Vec::new(),
            expected: vec![ExpectedAction::moving(
                at("actions-renamer", "Topic"),
                at("actions-renamed", "RenamedTopic"),
                detection::AUTO,
                TOPIC_IDENTIFIER,
            )],
        },
        ActionsCase {
            case: "a refactor that changes no stack's template and moves nothing",
            stacks: &[("actions-untouched", ONE_TOPIC)],
            description: None,
            enable_stack_creation: Some(false),
            definitions: vec![Definition::named("actions-untouched")],
            mappings: Vec::new(),
            // Nothing to create and nothing to move: the plan carries out nothing, which is
            // an empty list rather than an invented action.
            expected: Vec::new(),
        },
        ActionsCase {
            case: "a refactor whose creation cannot be carried out",
            stacks: &[("actions-failing", ONE_TOPIC)],
            description: None,
            enable_stack_creation: Some(false),
            definitions: vec![Definition::inline("actions-failing", NO_RESOURCES)],
            // A source the stack does not hold: the creation concludes CREATE_FAILED, so
            // there is no plan to execute and no action to report.
            mappings: vec![Mapping {
                source: ("actions-failing", "NotADeclaredResource"),
                destination: ("actions-failing", "Topic"),
            }],
            expected: Vec::new(),
        },
    ]
}

/// One reported `StackRefactorAction`, read member by member so each value stays with the
/// member that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireAction {
    action: Option<String>,
    entity: Option<String>,
    physical_resource_id: Option<String>,
    resource_identifier: Option<String>,
    description: Option<String>,
    detection: Option<String>,
    detection_reason: Option<String>,
    source: Option<(String, String)>,
    destination: Option<(String, String)>,
}

/// One end of a reported mapping, read inside its own block so the other end's members cannot
/// be swept up with it.
fn wire_location(mapping: &str, tag: &str) -> Option<(String, String)> {
    let block = xml_blocks(mapping, tag).into_iter().next()?;
    Some((
        xml_text(&block, "StackName").unwrap_or_default(),
        xml_text(&block, "LogicalResourceId").unwrap_or_default(),
    ))
}

/// The actions one `ListStackRefactorActions` response reports, in wire order.
fn wire_actions(body: &str) -> Vec<WireAction> {
    let Some(list) = xml_blocks(body, "StackRefactorActions").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&list, "member")
        .into_iter()
        .map(|reported| {
            let mapping = xml_blocks(&reported, "ResourceMapping").into_iter().next();
            WireAction {
                action: xml_text(&reported, "Action"),
                entity: xml_text(&reported, "Entity"),
                physical_resource_id: xml_text(&reported, "PhysicalResourceId"),
                resource_identifier: xml_text(&reported, "ResourceIdentifier"),
                description: xml_text(&reported, "Description"),
                detection: xml_text(&reported, "Detection"),
                detection_reason: xml_text(&reported, "DetectionReason"),
                source: mapping
                    .as_deref()
                    .and_then(|mapping| wire_location(mapping, "Source")),
                destination: mapping
                    .as_deref()
                    .and_then(|mapping| wire_location(mapping, "Destination")),
            }
        })
        .collect()
}

/// `ListStackRefactorActions` for one refactor, signed for `region`.
async fn list_actions_in(
    router: &MultiServiceRouter,
    region: &str,
    stack_refactor_id: &str,
    token: Option<&str>,
    max_results: Option<usize>,
) -> (u16, String) {
    let mut members = vec![member("StackRefactorId", stack_refactor_id)];
    if let Some(token) = token {
        members.push(member("NextToken", token));
    }
    if let Some(max_results) = max_results {
        members.push(member("MaxResults", &max_results.to_string()));
    }
    call_in(router, region, "ListStackRefactorActions", &members).await
}

/// [`list_actions_in`] in the Region every case but the addressing table uses.
async fn list_actions(
    router: &MultiServiceRouter,
    stack_refactor_id: &str,
) -> (u16, Vec<WireAction>, Option<String>) {
    let (status_code, body) = list_actions_in(router, REGION, stack_refactor_id, None, None).await;
    (status_code, wire_actions(&body), xml_text(&body, "NextToken"))
}

/// The physical resource id the stack that HOLDS a resource reports for it, which is what a
/// move has to name: the action's claim is checked against the stack's own read rather than
/// against the action itself.
async fn held_physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResource({stack}, {logical_id}) answered {status_code} with {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("expected {stack}.{logical_id} to report a physical resource id, got {body}")
    })
}

/// Deploy a row's stacks, create its refactor, and wait past the instant its creation's
/// conclusion falls due: the actions exist once the creation has concluded. Answers the
/// `StackRefactorId` and the id each deployed stack was given, which is how an action names
/// the stack it acts on.
async fn plan_refactor(
    router: &MultiServiceRouter,
    row: &ActionsCase,
) -> (String, Vec<(&'static str, String)>) {
    let mut deployed: Vec<(&'static str, String)> = Vec::new();
    for (name, template) in row.stacks {
        deployed.push((name, deploy(router, name, template).await));
    }
    let (status_code, body) = create_stack_refactor(
        router,
        row.description,
        row.enable_stack_creation,
        &row.definitions,
        &row.mappings,
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {:?}: expected CreateStackRefactor to succeed, got {status_code} {body}",
        row.case
    );
    let stack_refactor_id = xml_text(&body, "StackRefactorId")
        .unwrap_or_else(|| panic!("case {:?}: expected a StackRefactorId, got {body}", row.case));
    wait_for_the_transition().await;
    (stack_refactor_id, deployed)
}

/// How an action names one end of a move: by the stack's own id where the row deployed that
/// stack, and by the name the request gave where the refactor is still to create it.
fn stack_identity(deployed: &[(&'static str, String)], stack_name: &str) -> String {
    deployed
        .iter()
        .find(|(name, _)| *name == stack_name)
        .map_or_else(|| stack_name.to_string(), |(_, id)| id.clone())
}

/// The actions a refactor's creation concluded, reported in the order CloudFormation carries
/// them out and each one naming what it acts on.
#[tokio::test]
async fn the_actions_a_plan_concluded_are_reported_in_carry_out_order() {
    for row in actions_cases() {
        let case = row.case;
        let router = router();
        let (stack_refactor_id, deployed) = plan_refactor(&router, &row).await;

        let (status_code, reported, next_token) = list_actions(&router, &stack_refactor_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected ListStackRefactorActions to succeed, got {status_code}"
        );
        assert_eq!(
            reported.len(),
            row.expected.len(),
            "case {case:?}: expected {} action(s), got {reported:?}",
            row.expected.len()
        );
        assert_eq!(
            next_token, None,
            "case {case:?}: one page carries every action here, got {next_token:?}"
        );

        for (index, (expected, answered)) in row.expected.iter().zip(&reported).enumerate() {
            assert_eq!(
                (answered.action.as_deref(), answered.entity.as_deref()),
                (Some(expected.action), Some(expected.entity)),
                "case {case:?}: expected action {index} to be {:?} on a {:?}, got {answered:?}",
                expected.action,
                expected.entity
            );
            assert_eq!(
                answered.detection.as_deref(),
                expected.detection,
                "case {case:?}: expected action {index} to carry the detection {:?}, got \
                 {answered:?}",
                expected.detection
            );
            // "The description of the detection type": an action that says WHO decided the
            // mapping says it in words too, and one carrying no detection carries no reason.
            assert_eq!(
                answered.detection_reason.is_some(),
                expected.detection.is_some(),
                "case {case:?}: expected a detection reason exactly where there is a \
                 detection, got {answered:?}"
            );
            // "A description to help you identify the refactor": the refactor's own.
            assert_eq!(
                answered.description.as_deref(),
                row.description,
                "case {case:?}: expected action {index} to carry the refactor's description, \
                 got {answered:?}"
            );

            match expected.mapping {
                Some(ExpectedMove {
                    source,
                    destination,
                }) => {
                    // Each end names the stack by its own id where the stack exists, and by
                    // the name the request gave where the refactor is still to create it.
                    assert_eq!(
                        answered.source,
                        Some((
                            stack_identity(&deployed, source.stack),
                            source.logical_id.to_string()
                        )),
                        "case {case:?}: expected action {index} to move {source:?}, got \
                         {answered:?}"
                    );
                    assert_eq!(
                        answered.destination,
                        Some((
                            stack_identity(&deployed, destination.stack),
                            destination.logical_id.to_string()
                        )),
                        "case {case:?}: expected action {index} to move into {destination:?}, \
                         got {answered:?}"
                    );
                    // The instance the move acts on is the one the stack that holds the
                    // resource reports for it.
                    let held =
                        held_physical_id(&router, source.stack, source.logical_id).await;
                    assert_eq!(
                        answered.physical_resource_id.as_deref(),
                        Some(held.as_str()),
                        "case {case:?}: expected action {index} to name the instance the \
                         source stack holds ({held:?}), got {answered:?}"
                    );
                    let property = expected
                        .identifier_property
                        .expect("a move names the property its instance is identified by");
                    assert_eq!(
                        answered.resource_identifier.as_deref(),
                        Some(
                            ::serde_json::json!({ property: held.as_str() })
                                .to_string()
                                .as_str()
                        ),
                        "case {case:?}: expected action {index} to identify the resource by \
                         {property:?}, got {answered:?}"
                    );
                }
                None => {
                    assert!(
                        answered.source.is_none() && answered.destination.is_none(),
                        "case {case:?}: expected action {index} to carry no mapping, got \
                         {answered:?}"
                    );
                }
            }

            if let Some(created) = expected.created_stack {
                assert_eq!(
                    answered.physical_resource_id, None,
                    "case {case:?}: a stack that is still to be created has no instance to \
                     name, got {answered:?}"
                );
                assert_eq!(
                    answered.resource_identifier.as_deref(),
                    Some(
                        ::serde_json::json!({ "StackName": created })
                            .to_string()
                            .as_str()
                    ),
                    "case {case:?}: expected action {index} to identify the stack it creates, \
                     got {answered:?}"
                );
            }
        }

        // A read reports; it does not plan anything. A second read answers exactly what the
        // first did.
        let (_, again, _) = list_actions(&router, &stack_refactor_id).await;
        assert_eq!(
            again, reported,
            "case {case:?}: expected a second read to report the same actions, got {again:?}"
        );
    }
}

/// A refactor whose creation has not concluded has worked out no plan yet, so it reports no
/// actions — and reports them once the conclusion the clock owed it is due.
#[tokio::test]
async fn a_creation_still_in_flight_reports_no_actions() {
    let (router, engine) = router_with_engine();
    deploy(&router, "actions-in-flight", ONE_TOPIC).await;
    deploy(&router, "actions-in-flight-target", NO_RESOURCES).await;
    let (status_code, body) = create_stack_refactor(
        &router,
        Some("a refactor read while it is still being worked out"),
        Some(false),
        &[
            Definition::inline("actions-in-flight", NO_RESOURCES),
            Definition::inline("actions-in-flight-target", ONE_TOPIC),
        ],
        &[Mapping {
            source: ("actions-in-flight", "Topic"),
            destination: ("actions-in-flight-target", "Topic"),
        }],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "expected CreateStackRefactor to succeed, got {status_code} {body}"
    );
    let stack_refactor_id = xml_text(&body, "StackRefactorId")
        .unwrap_or_else(|| panic!("expected a StackRefactorId, got {body}"));

    // The conclusion is due at an INSTANT, so the case moves that instant rather than waiting
    // for it: first beyond reach, then already past.
    for (case, due_at, expected_actions) in [
        ("a creation whose conclusion is not due yet", still_running(), 0),
        ("a creation whose conclusion the clock has made due", already_due(), 1),
    ] {
        {
            let state = store(&engine);
            let mut held = state.lock();
            let refactor = held
                .stack_refactors
                .iter_mut()
                .find(|refactor| refactor.stack_refactor_id == stack_refactor_id)
                .unwrap_or_else(|| panic!("case {case:?}: the created refactor is in the store"));
            refactor
                .pending
                .as_mut()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a creation in flight is owed a conclusion")
                })
                .due_at = due_at;
        }
        let (status_code, reported, next_token) = list_actions(&router, &stack_refactor_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the read to succeed, got {status_code}"
        );
        assert_eq!(
            reported.len(),
            expected_actions,
            "case {case:?}: expected {expected_actions} action(s), got {reported:?}"
        );
        assert_eq!(
            next_token, None,
            "case {case:?}: there is no page past these actions, got {next_token:?}"
        );
    }
}

/// The actions are walked a page at a time: `MaxResults` bounds one page, the `NextToken`
/// resumes just past it, and every action is reported exactly once however large the pages
/// are. A token another refactor's walk minted and a value this service never issued are each
/// refused.
#[tokio::test]
async fn the_actions_are_paged_as_the_request_asks() {
    let router = router();
    let paged = ActionsCase {
        case: "a refactor that creates a stack and moves two resources into it",
        stacks: &[("actions-paged", TOPIC_AND_BUCKET)],
        description: Some("a paged plan"),
        enable_stack_creation: Some(true),
        definitions: vec![
            Definition::inline("actions-paged", NO_RESOURCES),
            Definition::inline("actions-paged-new", TOPIC_AND_BUCKET),
        ],
        mappings: vec![
            Mapping {
                source: ("actions-paged", "Alpha"),
                destination: ("actions-paged-new", "Alpha"),
            },
            Mapping {
                source: ("actions-paged", "Bucket"),
                destination: ("actions-paged-new", "Bucket"),
            },
        ],
        expected: Vec::new(),
    };
    let (stack_refactor_id, _) = plan_refactor(&router, &paged).await;

    let (_, whole, _) = list_actions(&router, &stack_refactor_id).await;
    assert_eq!(
        whole.len(),
        3,
        "expected the created stack and both moves, got {whole:?}"
    );

    // Pages of one, of two, and of more than there are: the walk reports the same actions in
    // the same order whatever page size is asked for.
    for page_size in [1usize, 2, 3, 100] {
        let mut walked: Vec<WireAction> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status_code, body) = list_actions_in(
                &router,
                REGION,
                &stack_refactor_id,
                token.as_deref(),
                Some(page_size),
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {page_size}: expected the page to be served, got {status_code} {body}"
            );
            let page = wire_actions(&body);
            assert!(
                page.len() <= page_size,
                "case {page_size}: expected at most {page_size} action(s) in a page, got \
                 {page:?}"
            );
            walked.extend(page);
            token = xml_text(&body, "NextToken");
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= whole.len(),
                "case {page_size}: expected the walk to end, got {pages} pages"
            );
        }
        assert_eq!(
            walked, whole,
            "case {page_size}: expected the walk to report every action exactly once, got \
             {walked:?}"
        );
    }

    // A token of this collection, minted under ANOTHER refactor's walk: the cursor says which
    // walk it belongs to, so it does not resume this one's.
    let other = ActionsCase {
        case: "another refactor's walk",
        stacks: &[("actions-other", TOPIC_AND_BUCKET)],
        description: None,
        enable_stack_creation: Some(true),
        definitions: vec![
            Definition::inline("actions-other", NO_RESOURCES),
            Definition::inline("actions-other-new", TOPIC_AND_BUCKET),
        ],
        mappings: vec![
            Mapping {
                source: ("actions-other", "Alpha"),
                destination: ("actions-other-new", "Alpha"),
            },
            Mapping {
                source: ("actions-other", "Bucket"),
                destination: ("actions-other-new", "Bucket"),
            },
        ],
        expected: Vec::new(),
    };
    let (other_id, _) = plan_refactor(&router, &other).await;
    let (_, body) = list_actions_in(&router, REGION, &other_id, None, Some(1)).await;
    let elsewhere = xml_text(&body, "NextToken")
        .unwrap_or_else(|| panic!("expected a token past the other refactor's page, got {body}"));

    for (case, presented, names) in [
        ("a token another refactor's walk minted", elsewhere, "Hash mismatch."),
        (
            "a value this service never issued",
            "not-a-next-token".to_string(),
            "is not a valid NextToken",
        ),
    ] {
        let (status_code, body) =
            list_actions_in(&router, REGION, &stack_refactor_id, Some(&presented), None).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the token to be refused, got {status_code} {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ValidationError",
            "case {case:?}: expected CloudFormation's own refusal code, got {body}"
        );
        let message = xml_text(&body, "Message")
            .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"));
        assert!(
            message.contains(names),
            "case {case:?}: expected the refusal to say {names:?}, got {message:?}"
        );
    }
}

/// A refactor this account and Region holds no record of is answered with the modelled
/// not-found error, worded with the id the request asked for.
#[tokio::test]
async fn a_refactor_this_region_holds_no_record_of_is_not_found() {
    let router = router();
    deploy(&router, "actions-addressing", ONE_TOPIC).await;
    let (status_code, body) = create_stack_refactor(
        &router,
        None,
        Some(false),
        &[Definition::named("actions-addressing")],
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "expected CreateStackRefactor to succeed, got {status_code} {body}"
    );
    let minted = xml_text(&body, "StackRefactorId")
        .unwrap_or_else(|| panic!("expected a StackRefactorId, got {body}"));

    let unaddressable: Vec<(&str, String, &str)> = vec![
        (
            "an id nothing created",
            "7f3b1a2c-0000-4000-8000-0123456789ab".to_string(),
            REGION,
        ),
        (
            "a value that is not an id at all",
            "not-a-refactor".to_string(),
            REGION,
        ),
        (
            "a real id read in another Region",
            minted.clone(),
            OTHER_REGION,
        ),
    ];

    for (case, id, region) in &unaddressable {
        let (status_code, body) = list_actions_in(&router, region, id, None, None).await;
        assert_eq!(
            status_code, 404,
            "case {case:?}: expected HTTP 404, got {status_code} {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "StackRefactorNotFoundException",
            "case {case:?}: expected the modelled not-found error, got {body}"
        );
        let message = xml_text(&body, "Message")
            .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"));
        // CloudFormation words this refusal with the id the request asked for in square
        // brackets, and nothing else of the request or of the store.
        assert_eq!(
            message,
            format!("Stack refactor [{id}] does not exist"),
            "case {case:?}: expected the refusal to name the id asked for, got {message:?}"
        );
    }
}
