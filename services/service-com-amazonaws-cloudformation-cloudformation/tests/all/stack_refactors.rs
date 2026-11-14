// SPDX-License-Identifier: CC0-1.0

//! Stack REFACTORS, end to end over the awsQuery wire: `CreateStackRefactor` records a
//! refactor across the stacks it names, and `DescribeStackRefactor` reads THAT record back.
//!
//! Three rules are under test here, each with its own table.
//!
//! 1. **A refactor is persisted and read back.** `CreateStackRefactor` answers a
//!    `StackRefactorId`, and `DescribeStackRefactor` reports the record it addresses: the
//!    description the call supplied, the id of every stack the refactor covers, and the two
//!    statuses the feature carries. Creating one is asynchronous, so the record is answered
//!    `CREATE_IN_PROGRESS` with nothing executable (`UNAVAILABLE`) and reaches its conclusion
//!    once the clock has made it due — `CREATE_COMPLETE` with the plan `AVAILABLE` for a
//!    refactor that can be carried out, `CREATE_FAILED` with a reason naming what failed and
//!    nothing executable for one that cannot. Reading the record never changes what it
//!    reports.
//!
//! 2. **A refactor is addressable by its own id, in its own Region.** Each call mints its own
//!    id; an id nothing created, and the id of a refactor another Region holds, are each
//!    answered with the modelled `StackRefactorNotFoundException` (HTTP 404).
//!
//! 3. **The refusals around creating one.** A request that names no stack to refactor, a
//!    definition that names no stack, a definition that spells its template both inline and
//!    by reference, and a request past what one record may carry are each refused 400
//!    `ValidationError` with nothing recorded.
//!
//! Each table spans the class its rule generalises over rather than one recorded case: a
//! refactor over one stack and over two, with moves and with none, with a description and
//! without; a template submitted inline and the same one submitted by reference; a stack that
//! exists and one that does not, with and without `EnableStackCreation`; a move whose source
//! resource exists, one whose source resource does not, and one whose source stack does not;
//! and, for the bound on one record, a request exactly at it and one past it.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_refactors as refactors;
use service_com_amazonaws_cloudformation_cloudformation::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_envelope::assert_identifier_shape;
use crate::generated_templates::{deploy, error_code};
use crate::stack_lifecycle::{
    call, call_in, list_members, member, router, wait_for_the_transition, xml_text, REGION,
};
use crate::template_by_reference::{create_staging_bucket, stage, STAGING_BUCKET};

/// A second Region, for the cases that pin a refactor to the Region it was created in.
const OTHER_REGION: &str = "us-west-2";

/// A template declaring one resource, the stack a move takes its resource FROM.
pub(crate) const ONE_TOPIC: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A template declaring two resources, so a move can name one of several.
pub(crate) const TOPIC_AND_BUCKET: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// The final state a definition submits for a stack that is to end up holding nothing of its
/// own, which is what a template with no resources describes.
pub(crate) const NO_RESOURCES: &str = r#"{"Resources":{}}"#;

/// The key the by-reference row stages its template object under.
const STAGED_KEY: &str = "refactors/final-state.template.json";

/// One `StackDefinition` a case submits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Definition {
    pub(crate) stack_name: Option<&'static str>,
    pub(crate) template_body: Option<&'static str>,
    /// `true` where the row submits the template BY REFERENCE instead: the staged object's
    /// URL is built at call time, since it carries the bucket and Region.
    pub(crate) staged: bool,
}

impl Definition {
    /// A definition naming a stack and its final template inline.
    pub(crate) const fn inline(stack_name: &'static str, template_body: &'static str) -> Self {
        Self {
            stack_name: Some(stack_name),
            template_body: Some(template_body),
            staged: false,
        }
    }

    /// A definition naming a stack whose own template the refactor does not change.
    pub(crate) const fn named(stack_name: &'static str) -> Self {
        Self {
            stack_name: Some(stack_name),
            template_body: None,
            staged: false,
        }
    }
}

/// One `ResourceMapping` a case submits: where the resource is now, and where it is to end up.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mapping {
    pub(crate) source: (&'static str, &'static str),
    pub(crate) destination: (&'static str, &'static str),
}

/// The URL of the staged template object, in Amazon S3's documented virtual-hosted-style
/// request form (`https://<bucket>.s3.<region>.amazonaws.com/<key>`).
fn staged_url() -> String {
    format!("https://{STAGING_BUCKET}.s3.{REGION}.amazonaws.com/{STAGED_KEY}")
}

/// The form members of one `CreateStackRefactor` request, in the awsQuery list spelling
/// (`StackDefinitions.member.1.StackName`), as an SDK sends them.
fn refactor_members(
    description: Option<&str>,
    enable_stack_creation: Option<bool>,
    definitions: &[Definition],
    mappings: &[Mapping],
) -> Vec<(String, String)> {
    let mut members = Vec::new();
    if let Some(description) = description {
        members.push(member("Description", description));
    }
    if let Some(enable) = enable_stack_creation {
        members.push(member("EnableStackCreation", &enable.to_string()));
    }
    for (index, definition) in definitions.iter().enumerate() {
        let at = format!("StackDefinitions.member.{}", index + 1);
        if let Some(stack_name) = definition.stack_name {
            members.push(member(&format!("{at}.StackName"), stack_name));
        }
        if let Some(body) = definition.template_body {
            members.push(member(&format!("{at}.TemplateBody"), body));
        }
        if definition.staged {
            members.push(member(&format!("{at}.TemplateURL"), &staged_url()));
        }
    }
    for (index, mapping) in mappings.iter().enumerate() {
        let at = format!("ResourceMappings.member.{}", index + 1);
        members.push(member(&format!("{at}.Source.StackName"), mapping.source.0));
        members.push(member(
            &format!("{at}.Source.LogicalResourceId"),
            mapping.source.1,
        ));
        members.push(member(
            &format!("{at}.Destination.StackName"),
            mapping.destination.0,
        ));
        members.push(member(
            &format!("{at}.Destination.LogicalResourceId"),
            mapping.destination.1,
        ));
    }
    members
}

/// `CreateStackRefactor` with whatever the case submits.
pub(crate) async fn create_stack_refactor(
    router: &MultiServiceRouter,
    description: Option<&str>,
    enable_stack_creation: Option<bool>,
    definitions: &[Definition],
    mappings: &[Mapping],
) -> (u16, String) {
    call(
        router,
        "CreateStackRefactor",
        &refactor_members(description, enable_stack_creation, definitions, mappings),
    )
    .await
}

/// `DescribeStackRefactor` for one refactor, by the id the create answered.
async fn describe_stack_refactor(
    router: &MultiServiceRouter,
    stack_refactor_id: &str,
) -> (u16, String) {
    describe_stack_refactor_in(router, REGION, stack_refactor_id).await
}

/// [`describe_stack_refactor`], signed for one Region: a refactor is addressable only in the
/// Region the call that created it was signed for.
async fn describe_stack_refactor_in(
    router: &MultiServiceRouter,
    region: &str,
    stack_refactor_id: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeStackRefactor",
        &[member("StackRefactorId", stack_refactor_id)],
    )
    .await
}

/// The `StackRefactorId` of a successful create, checked to be the 8-4-4-4-12 lowercase-hex
/// identifier form this service mints its ids in — through the one spelling of that check this
/// test crate has (`error_envelope.rs`), rather than a copy of it.
fn assert_stack_refactor_id_shape(case: &str, id: &str) {
    assert_identifier_shape(case, "stack refactor id", id);
}

/// What a row expects the record's `StatusReason` to name once its creation has concluded.
#[derive(Debug, Clone, Copy)]
enum ExpectedReason {
    /// A creation that completed carries none.
    None,
    /// A creation that failed carries one, and these are the values of the request it must
    /// name — the stack, and the logical resource id where the row is about a move.
    Naming(&'static [&'static str]),
}

/// The statuses a read may legitimately report for a transition that became due one
/// [`STATUS_TRANSITION_DELAY`] after `started`, given that the request was sent at `sent_at`
/// and answered at `answered_at`.
///
/// The emulator owes the in-progress status only until the due instant, and a test cannot see
/// the instant the request was really served — only that it was between those two. So the
/// in-progress status is admissible while the conclusion was not yet due when the request was
/// SENT, and the settled one once it was due by the time the answer came back. That keeps the
/// case honest on a loaded machine without accepting both answers at every instant.
fn reportable(
    started: Instant,
    sent_at: Instant,
    answered_at: Instant,
    in_progress: &'static str,
    settled: &'static str,
) -> Vec<&'static str> {
    let mut allowed = Vec::new();
    if sent_at.duration_since(started) < STATUS_TRANSITION_DELAY {
        allowed.push(in_progress);
    }
    if answered_at.duration_since(started) >= STATUS_TRANSITION_DELAY {
        allowed.push(settled);
    }
    allowed
}

/// One row of the round-trip table.
struct RefactorCase {
    case: &'static str,
    /// The stacks deployed before the refactor, one `(name, template)` each.
    stacks: &'static [(&'static str, &'static str)],
    description: Option<&'static str>,
    enable_stack_creation: Option<bool>,
    definitions: Vec<Definition>,
    mappings: Vec<Mapping>,
    /// The stacks whose ids the record reports, in the order it reports them.
    reported_stacks: &'static [&'static str],
    /// The `Status` the creation settles in.
    settled_status: &'static str,
    /// The `ExecutionStatus` that conclusion leaves.
    settled_execution_status: &'static str,
    reason: ExpectedReason,
}

/// The rows of the round-trip table.
fn refactor_cases() -> Vec<RefactorCase> {
    vec![
        RefactorCase {
            case: "a move between two stacks that both exist",
            stacks: &[("refactor-source", ONE_TOPIC), ("refactor-target", NO_RESOURCES)],
            description: Some("move the topic into the target stack"),
            enable_stack_creation: Some(false),
            definitions: vec![
                Definition::inline("refactor-source", NO_RESOURCES),
                Definition::inline("refactor-target", ONE_TOPIC),
            ],
            mappings: vec![Mapping {
                source: ("refactor-source", "Topic"),
                destination: ("refactor-target", "Topic"),
            }],
            reported_stacks: &["refactor-source", "refactor-target"],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            case: "two moves out of one stack, no description",
            stacks: &[
                ("refactor-pair", TOPIC_AND_BUCKET),
                ("refactor-split", NO_RESOURCES),
            ],
            description: None,
            enable_stack_creation: None,
            definitions: vec![
                Definition::inline("refactor-pair", NO_RESOURCES),
                Definition::inline("refactor-split", TOPIC_AND_BUCKET),
            ],
            mappings: vec![
                Mapping {
                    source: ("refactor-pair", "Alpha"),
                    destination: ("refactor-split", "Alpha"),
                },
                Mapping {
                    source: ("refactor-pair", "Bucket"),
                    destination: ("refactor-split", "Bucket"),
                },
            ],
            reported_stacks: &["refactor-pair", "refactor-split"],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            // No `ResourceMappings` at all: CloudFormation is left to work the mapping out
            // itself, so there is no move for the creation to resolve.
            case: "one stack, a final template and no mappings",
            stacks: &[("refactor-alone", ONE_TOPIC)],
            description: Some("retemplate one stack"),
            enable_stack_creation: Some(false),
            definitions: vec![Definition::inline("refactor-alone", TOPIC_AND_BUCKET)],
            mappings: vec![],
            reported_stacks: &["refactor-alone"],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            // A definition carrying NEITHER template: "the stacks being refactored" may
            // include one whose own template the refactor does not change.
            case: "a definition naming a stack and no template",
            stacks: &[("refactor-untouched", ONE_TOPIC)],
            description: None,
            enable_stack_creation: Some(false),
            definitions: vec![Definition::named("refactor-untouched")],
            mappings: vec![],
            reported_stacks: &["refactor-untouched"],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            // The SAME final state submitted by reference rather than inline: a `TemplateURL`
            // says where the template is, not what it means.
            case: "a final template submitted by reference",
            stacks: &[("refactor-staged", ONE_TOPIC)],
            description: Some("a staged final state"),
            enable_stack_creation: Some(false),
            definitions: vec![Definition {
                stack_name: Some("refactor-staged"),
                template_body: None,
                staged: true,
            }],
            mappings: vec![],
            reported_stacks: &["refactor-staged"],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            // "Determines if a new stack is created with the refactor": with it, a definition
            // naming a stack that does not exist yet is a stack the refactor would create, so
            // the refactor can be carried out and reports only the id of the stack that does
            // exist.
            case: "a move into a stack that does not exist yet, with stack creation enabled",
            stacks: &[("refactor-mover", ONE_TOPIC)],
            description: Some("move the topic into a brand new stack"),
            enable_stack_creation: Some(true),
            definitions: vec![
                Definition::inline("refactor-mover", NO_RESOURCES),
                Definition::inline("refactor-brand-new", ONE_TOPIC),
            ],
            mappings: vec![Mapping {
                source: ("refactor-mover", "Topic"),
                destination: ("refactor-brand-new", "Topic"),
            }],
            reported_stacks: &["refactor-mover"],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            // The same refactor without it: there is no stack for the definition to refactor,
            // so the creation fails and nothing is executable.
            case: "a stack that does not exist, with stack creation not asked for",
            stacks: &[("refactor-stayer", ONE_TOPIC)],
            description: Some("the same move with creation left off"),
            enable_stack_creation: None,
            definitions: vec![
                Definition::inline("refactor-stayer", NO_RESOURCES),
                Definition::inline("refactor-absent", ONE_TOPIC),
            ],
            mappings: vec![Mapping {
                source: ("refactor-stayer", "Topic"),
                destination: ("refactor-absent", "Topic"),
            }],
            reported_stacks: &["refactor-stayer"],
            settled_status: refactors::status::CREATE_FAILED,
            settled_execution_status: refactors::execution_status::UNAVAILABLE,
            reason: ExpectedReason::Naming(&["refactor-absent"]),
        },
        RefactorCase {
            // Every stack the refactor names is still to be created, so there is no existing
            // stack for it to cover: the record reports an empty `StackIds` list.
            case: "a refactor over nothing but stacks to be created",
            stacks: &[],
            description: Some("a refactor of brand new stacks"),
            enable_stack_creation: Some(true),
            definitions: vec![Definition::inline("refactor-fresh", ONE_TOPIC)],
            mappings: vec![],
            reported_stacks: &[],
            settled_status: refactors::status::CREATE_COMPLETE,
            settled_execution_status: refactors::execution_status::AVAILABLE,
            reason: ExpectedReason::None,
        },
        RefactorCase {
            // The same rule at the OTHER end of a move: a destination no definition describes
            // and this Region holds none of is a stack the refactor would have to create too.
            case: "a move into a stack that is named nowhere else and does not exist",
            stacks: &[("refactor-outbound", ONE_TOPIC)],
            description: None,
            enable_stack_creation: None,
            definitions: vec![Definition::inline("refactor-outbound", NO_RESOURCES)],
            mappings: vec![Mapping {
                source: ("refactor-outbound", "Topic"),
                destination: ("refactor-unnamed", "Topic"),
            }],
            reported_stacks: &["refactor-outbound"],
            settled_status: refactors::status::CREATE_FAILED,
            settled_execution_status: refactors::execution_status::UNAVAILABLE,
            reason: ExpectedReason::Naming(&["refactor-unnamed"]),
        },
        RefactorCase {
            // `ResourceMapping$Source` is "the source stack StackName and LogicalResourceId
            // for the resource being refactored": a logical id the source stack does not hold
            // is a resource there is nothing to move.
            case: "a move whose source resource the source stack does not hold",
            stacks: &[
                ("refactor-holder", ONE_TOPIC),
                ("refactor-destination", NO_RESOURCES),
            ],
            description: None,
            enable_stack_creation: Some(false),
            definitions: vec![
                Definition::inline("refactor-holder", NO_RESOURCES),
                Definition::inline("refactor-destination", ONE_TOPIC),
            ],
            mappings: vec![Mapping {
                source: ("refactor-holder", "NotADeclaredResource"),
                destination: ("refactor-destination", "Topic"),
            }],
            reported_stacks: &["refactor-holder", "refactor-destination"],
            settled_status: refactors::status::CREATE_FAILED,
            settled_execution_status: refactors::execution_status::UNAVAILABLE,
            reason: ExpectedReason::Naming(&["refactor-holder", "NotADeclaredResource"]),
        },
        RefactorCase {
            // The same rule one level out: the source STACK is not there either. Stack
            // creation is enabled, so the definitions are satisfied and the move is what
            // fails.
            case: "a move whose source stack does not exist",
            stacks: &[("refactor-only", ONE_TOPIC)],
            description: None,
            enable_stack_creation: Some(true),
            definitions: vec![Definition::inline("refactor-only", TOPIC_AND_BUCKET)],
            mappings: vec![Mapping {
                source: ("refactor-nowhere", "Topic"),
                destination: ("refactor-only", "Topic"),
            }],
            reported_stacks: &["refactor-only"],
            settled_status: refactors::status::CREATE_FAILED,
            settled_execution_status: refactors::execution_status::UNAVAILABLE,
            reason: ExpectedReason::Naming(&["refactor-nowhere", "Topic"]),
        },
    ]
}

/// The members `DescribeStackRefactor` reports for one refactor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireRefactor {
    stack_refactor_id: Option<String>,
    description: Option<String>,
    stack_ids: Vec<String>,
    status: Option<String>,
    status_reason: Option<String>,
    execution_status: Option<String>,
    execution_status_reason: Option<String>,
}

/// One `DescribeStackRefactor` response, read member by member so each value stays with the
/// record it belongs to.
fn wire_refactor(case: &str, body: &str) -> WireRefactor {
    let read = WireRefactor {
        stack_refactor_id: xml_text(body, "StackRefactorId"),
        description: xml_text(body, "Description"),
        stack_ids: list_members(body, "StackIds"),
        status: xml_text(body, "Status"),
        status_reason: xml_text(body, "StatusReason"),
        execution_status: xml_text(body, "ExecutionStatus"),
        execution_status_reason: xml_text(body, "ExecutionStatusReason"),
    };
    // Every caller of this reader has already asserted the read succeeded, so a response
    // carrying no refactor at all is a failure worth naming here rather than one that shows
    // up as a confusing comparison below.
    assert!(
        read.stack_refactor_id.is_some() && read.status.is_some(),
        "case {case:?}: expected a described refactor, got {body}"
    );
    read
}

/// A refactor is recorded by the call that creates it and read back by its id: the stacks it
/// covers, the description it was given, and both statuses — in progress right after the call
/// and concluded once the clock has made the conclusion due.
#[tokio::test]
async fn a_refactor_is_recorded_and_read_back_by_its_id() {
    for row in refactor_cases() {
        let case = row.case;
        let router = router();
        create_staging_bucket(&router).await;
        stage(&router, STAGED_KEY, ONE_TOPIC).await;

        let mut stack_ids: Vec<(&str, String)> = Vec::new();
        for (name, template) in row.stacks {
            stack_ids.push((name, deploy(&router, name, template).await));
        }
        let expected_ids: Vec<String> = row
            .reported_stacks
            .iter()
            .map(|name| {
                stack_ids
                    .iter()
                    .find(|(deployed, _)| deployed == name)
                    .map(|(_, id)| id.clone())
                    .unwrap_or_else(|| panic!("case {case:?}: {name} is not deployed by this row"))
            })
            .collect();

        let started = Instant::now();
        let (status_code, body) = create_stack_refactor(
            &router,
            row.description,
            row.enable_stack_creation,
            &row.definitions,
            &row.mappings,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStackRefactor to succeed, got {status_code} {body}"
        );
        let stack_refactor_id = xml_text(&body, "StackRefactorId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a StackRefactorId, got {body}")
        });
        assert_stack_refactor_id_shape(case, &stack_refactor_id);

        // Read straight after the call: the creation is still in flight and nothing is
        // executable yet.
        let sent_at = Instant::now();
        let (status_code, body) = describe_stack_refactor(&router, &stack_refactor_id).await;
        let answered_at = Instant::now();
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStackRefactor to succeed, got {status_code} {body}"
        );
        let in_flight = wire_refactor(case, &body);
        let allowed_status = reportable(
            started,
            sent_at,
            answered_at,
            refactors::status::CREATE_IN_PROGRESS,
            row.settled_status,
        );
        assert!(
            in_flight
                .status
                .as_deref()
                .is_some_and(|status| allowed_status.contains(&status)),
            "case {case:?}: expected one of {allowed_status:?} while the creation runs, got {body}"
        );
        let allowed_execution_status = reportable(
            started,
            sent_at,
            answered_at,
            refactors::execution_status::UNAVAILABLE,
            row.settled_execution_status,
        );
        assert!(
            in_flight
                .execution_status
                .as_deref()
                .is_some_and(|status| allowed_execution_status.contains(&status)),
            "case {case:?}: expected one of {allowed_execution_status:?} while the creation \
             runs, got {body}"
        );
        assert_eq!(
            in_flight.stack_refactor_id.as_deref(),
            Some(stack_refactor_id.as_str()),
            "case {case:?}: expected the read to report the id it was asked for, got {body}"
        );
        assert_eq!(
            in_flight.description.as_deref(),
            row.description,
            "case {case:?}: expected the description the call supplied, got {body}"
        );
        assert_eq!(
            in_flight.stack_ids, expected_ids,
            "case {case:?}: expected the ids of the stacks the refactor covers, got {body}"
        );

        wait_for_the_transition().await;

        let (status_code, body) = describe_stack_refactor(&router, &stack_refactor_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the settled read to succeed, got {status_code} {body}"
        );
        let settled = wire_refactor(case, &body);
        assert_eq!(
            settled.status.as_deref(),
            Some(row.settled_status),
            "case {case:?}: expected the creation to conclude {:?}, got {body}",
            row.settled_status
        );
        assert_eq!(
            settled.execution_status.as_deref(),
            Some(row.settled_execution_status),
            "case {case:?}: expected the execution status {:?}, got {body}",
            row.settled_execution_status
        );
        assert_eq!(
            settled.stack_ids, expected_ids,
            "case {case:?}: expected the covered stack ids to be unchanged, got {body}"
        );
        match row.reason {
            ExpectedReason::None => assert_eq!(
                settled.status_reason, None,
                "case {case:?}: expected no StatusReason on a completed creation, got {body}"
            ),
            ExpectedReason::Naming(named) => {
                let reason = settled.status_reason.clone().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a StatusReason on a failed creation, got {body}")
                });
                for value in named {
                    assert!(
                        reason.contains(value),
                        "case {case:?}: expected the reason to name {value:?}, got {reason:?}"
                    );
                }
            }
        }
        assert_eq!(
            settled.execution_status_reason, None,
            "case {case:?}: nothing has executed, so there is no execution reason, got {body}"
        );

        // A read reports; it does not advance anything. A second read of the settled record
        // answers exactly what the first did.
        let (_, body) = describe_stack_refactor(&router, &stack_refactor_id).await;
        assert_eq!(
            wire_refactor(case, &body),
            settled,
            "case {case:?}: expected a second read to report the same record, got {body}"
        );
    }
}

/// Each create mints its own id, and a refactor is addressable by that id in the Region it was
/// created in and nowhere else: anything else is the modelled
/// `StackRefactorNotFoundException`.
#[tokio::test]
async fn a_refactor_is_addressable_only_by_its_own_id_in_its_own_region() {
    let router = router();
    deploy(&router, "refactor-addressing", ONE_TOPIC).await;

    // Two refactors over the same stack: each must come back under its OWN id.
    let mut minted: Vec<String> = Vec::new();
    for description in ["the first refactor", "the second refactor"] {
        let (status_code, body) = create_stack_refactor(
            &router,
            Some(description),
            Some(false),
            &[Definition::named("refactor-addressing")],
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "two refactors must both be created, got {status_code} {body}"
        );
        minted.push(
            xml_text(&body, "StackRefactorId")
                .unwrap_or_else(|| panic!("expected a StackRefactorId, got {body}")),
        );
    }
    assert_ne!(
        minted[0], minted[1],
        "each CreateStackRefactor mints its own id, got {minted:?}"
    );
    for (description, id) in ["the first refactor", "the second refactor"]
        .iter()
        .zip(&minted)
    {
        let (status_code, body) = describe_stack_refactor(&router, id).await;
        assert_eq!(
            status_code, 200,
            "case {description:?}: expected the read to succeed, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Description").as_deref(),
            Some(*description),
            "case {description:?}: expected each id to answer its own record, got {body}"
        );
    }

    // An id nothing created, in every shape a client might send, and the id of a refactor
    // another Region holds.
    //
    // The near-miss row flips the LAST digit of a real id to a value it does not already
    // carry, so the id is always a different one: appending a fixed digit would leave the id
    // unchanged whenever it already ended in that digit.
    let near_miss = {
        let (head, last) = minted[0].split_at(minted[0].len() - 1);
        let flipped = if last == "0" { "1" } else { "0" };
        format!("{head}{flipped}")
    };
    assert_ne!(
        near_miss, minted[0],
        "the near-miss id must differ from the one that was minted, got {near_miss:?}"
    );
    let unaddressable: Vec<(String, String)> = vec![
        (
            "an id nothing created".to_string(),
            "7f3b1a2c-0000-4000-8000-0123456789ab".to_string(),
        ),
        (
            "a value that is not an id at all".to_string(),
            "not-a-refactor".to_string(),
        ),
        (
            "an id of this Region with one digit changed".to_string(),
            near_miss,
        ),
        (
            "a real id read in another Region".to_string(),
            minted[0].clone(),
        ),
    ];

    for (case, id) in &unaddressable {
        let region = if case.contains("another Region") {
            OTHER_REGION
        } else {
            REGION
        };
        let (status_code, body) = describe_stack_refactor_in(&router, region, id).await;
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

/// One row of the refusal table: a request that cannot be a refactor, and the member that
/// makes it one.
struct RefusalCase {
    case: &'static str,
    definitions: Vec<Definition>,
    mappings: Vec<Mapping>,
    /// The wire error code the refusal carries: CloudFormation's own `ValidationError`, which
    /// is the one code its Query API publishes for a refused request — whether the service
    /// refused it or the boundary validator did, for a required member the request does not
    /// carry at all.
    code: &'static str,
    /// A fragment of the request the refusal must name, so the caller can fix it.
    names: &'static str,
}

/// A request that cannot describe a refactor is refused 400 `ValidationError`, and nothing is
/// recorded for it.
#[tokio::test]
async fn a_request_that_cannot_describe_a_refactor_is_refused() {
    let router = router();
    deploy(&router, "refactor-refusals", ONE_TOPIC).await;

    let refusals = vec![
        RefusalCase {
            // Refused at the boundary rather than by the handler: `StackDefinitions` is
            // required, so a request carrying none never describes a refactor.
            case: "no stack definitions at all",
            definitions: Vec::new(),
            mappings: Vec::new(),
            code: "ValidationError",
            names: "stackDefinitions",
        },
        RefusalCase {
            case: "a definition that names no stack",
            definitions: vec![Definition {
                stack_name: None,
                template_body: Some(ONE_TOPIC),
                staged: false,
            }],
            mappings: Vec::new(),
            code: "ValidationError",
            names: "StackName",
        },
        RefusalCase {
            case: "a definition spelling its template both inline and by reference",
            definitions: vec![Definition {
                stack_name: Some("refactor-refusals"),
                template_body: Some(ONE_TOPIC),
                staged: true,
            }],
            mappings: Vec::new(),
            code: "ValidationError",
            names: "Template URL",
        },
        RefusalCase {
            // One past the bound on what ONE record may carry.
            case: "one stack definition more than a refactor may carry",
            definitions: (0..=refactors::MAX_STACKS_PER_REFACTOR)
                .map(|_| Definition::named("refactor-refusals"))
                .collect(),
            mappings: Vec::new(),
            code: "ValidationError",
            names: "StackDefinitions",
        },
        RefusalCase {
            // And one past the bound on the moves.
            case: "one resource mapping more than a refactor may carry",
            definitions: vec![Definition::named("refactor-refusals")],
            mappings: (0..=refactors::MAX_RESOURCE_MAPPINGS_PER_REFACTOR)
                .map(|_| Mapping {
                    source: ("refactor-refusals", "Topic"),
                    destination: ("refactor-refusals", "Topic"),
                })
                .collect(),
            code: "ValidationError",
            names: "ResourceMappings",
        },
    ];

    for row in &refusals {
        let case = row.case;
        let (status_code, body) =
            create_stack_refactor(&router, None, Some(true), &row.definitions, &row.mappings)
                .await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the request to be refused, got {status_code} {body}"
        );
        assert_eq!(
            error_code(case, &body),
            row.code,
            "case {case:?}: expected the refusal code {:?}, got {body}",
            row.code
        );
        let message = xml_text(&body, "Message")
            .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"));
        assert!(
            message.contains(row.names),
            "case {case:?}: expected the refusal to name {:?}, got {message:?}",
            row.names
        );
        assert!(
            xml_text(&body, "StackRefactorId").is_none(),
            "case {case:?}: a refused request records no refactor, got {body}"
        );
    }

    // Exactly AT the bound is accepted: the refusal above is about the request that exceeds
    // it, not about the one that reaches it.
    let at_the_bound: Vec<Definition> = (0..refactors::MAX_STACKS_PER_REFACTOR)
        .map(|_| Definition::named("refactor-refusals"))
        .collect();
    let (status_code, body) =
        create_stack_refactor(&router, None, Some(true), &at_the_bound, &[]).await;
    assert_eq!(
        status_code, 200,
        "a refactor exactly at the bound is accepted, got {status_code} {body}"
    );
}
