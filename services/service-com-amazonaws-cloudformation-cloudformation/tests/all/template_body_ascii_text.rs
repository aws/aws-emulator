// SPDX-License-Identifier: CC0-1.0

//! The TEXT of the TEMPLATE a caller submits, read back over the awsQuery wire.
//!
//! **The rule every case here holds the service to: CloudFormation holds the template body it
//! was handed as ASCII, so a read of it reports every codepoint outside ASCII replaced by
//! exactly one `?` — with every ASCII character, the character count, the document's
//! structure and what it declares left as sent — whichever operation stored the body and
//! whichever operation reads it back.**
//!
//! `GetTemplate` answers `TemplateBody` for the named stack (`smithy_query(query="operation",
//! operation="GetTemplate")`) and the member carries no character-set constraint, so a request
//! may submit any text the body's documented maximum admits and the service answers 200 — what
//! the store cannot hold is the codepoint, not the request. It is the same transcoding a tag's
//! text goes through (`stack_tag_ascii_text.rs`), applied to the one other free text a caller
//! hands this service to keep.
//!
//! The rows span what the rule generalises over: the width of the encoding (a Latin-1 letter,
//! a two-, three- and four-byte encoding — one `?` per codepoint, not per byte), the ASCII
//! boundary itself (the last ASCII codepoint, which survives, and the first one past it, which
//! does not), WHERE in the document the text sits (`Description`, a nested `Metadata` value, a
//! resource property, several places at once) and the JSON punctuation that must survive
//! around it. The reads span the stages a stored body is served at (`Original` and the default
//! `Processed`), the stack's own `Description`, the summary of the stored template, the
//! comparison that decides whether an update changes the stack at all, and the four operations
//! that store a submitted body.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, router, wait_for_the_transition,
    xml_text,
};

const STACK: &str = "template-body-ascii-text";
const STACK_SET: &str = "template-body-ascii-text-set";
const CHANGE_SET: &str = "template-body-ascii-text-plan";

/// One row of the table: the template submitted, and the template a read must report.
struct BodyCase {
    case: &'static str,
    /// The body the request carries.
    sent: String,
    /// The body every read of the stored template must report.
    held: String,
}

/// A one-resource template whose `Description` is `description`: the smallest body that
/// carries free text beside the structure that must survive it.
fn described(description: &str) -> String {
    format!(
        r#"{{"Description":"{description}","Resources":{{"Topic":{{"Type":"AWS::SNS::Topic","Properties":{{"DisplayName":"ascii-template-text"}}}}}}}}"#
    )
}

/// A template whose free text sits in three places at once: its `Description`, two nested
/// `Metadata` values and a resource property.
fn metadata_body(description: &str, note: &str, arrow: &str, display_name: &str) -> String {
    format!(
        r#"{{"Description":"{description}","Metadata":{{"team":{{"note":"{note}","arrow":"{arrow}"}}}},"Resources":{{"Topic":{{"Type":"AWS::SNS::Topic","Properties":{{"DisplayName":"{display_name}"}}}}}}}}"#
    )
}

fn body_cases() -> Vec<BodyCase> {
    vec![
        BodyCase {
            case: "a wholly ASCII body, with the punctuation a template carries",
            sent: described("deploy/v1.0 (build+2024) - 100% ASCII"),
            held: described("deploy/v1.0 (build+2024) - 100% ASCII"),
        },
        BodyCase {
            case: "the last ASCII codepoint, which the store holds as it is",
            sent: described("del\u{7f}-and-tilde~"),
            held: described("del\u{7f}-and-tilde~"),
        },
        BodyCase {
            case: "the first codepoint past ASCII",
            sent: described("caf\u{80}"),
            held: described("caf?"),
        },
        BodyCase {
            case: "a Latin-1 letter inside ASCII text",
            sent: described("caf\u{e9} na\u{ef}ve, Z\u{fc}rich"),
            held: described("caf? na?ve, Z?rich"),
        },
        BodyCase {
            case: "a two-byte encoding outside Latin-1",
            sent: described("\u{41c}\u{43e}\u{441}\u{43a}\u{432}\u{430}"),
            held: described("??????"),
        },
        BodyCase {
            case: "a three-byte encoding, one question mark per codepoint",
            sent: described("\u{90e8}\u{7f72}\u{30c6}\u{30f3}\u{30d7}"),
            held: described("?????"),
        },
        BodyCase {
            case: "an em dash, which is one codepoint and so one question mark",
            sent: described("deploy \u{2014} now"),
            held: described("deploy ? now"),
        },
        BodyCase {
            case: "a four-byte encoding, one codepoint and so one question mark",
            sent: described("launch \u{1f680}"),
            held: described("launch ?"),
        },
        BodyCase {
            case: "every width at once, with the ASCII text surviving around them",
            sent: described(
                "\u{90e8}\u{7f72} \u{1f680} \u{2014} caf\u{e9} na\u{ef}ve, \
                 Z\u{fc}rich, \u{41c}\u{43e}\u{441}\u{43a}\u{432}\u{430}",
            ),
            held: described("?? ? ? caf? na?ve, Z?rich, ??????"),
        },
        BodyCase {
            case: "non-ASCII text in a nested Metadata value and in a resource property",
            sent: metadata_body(
                "caf\u{e9}",
                "\u{30c6}\u{30b9}\u{30c8}",
                "\u{2192}",
                "r\u{e9}sum\u{e9}",
            ),
            held: metadata_body("caf?", "???", "?", "r?sum?"),
        },
    ]
}

/// The operations whose input carries a template body this service STORES — so the rule is
/// asserted over the whole surface that keeps one.
#[derive(Debug, Clone, Copy)]
enum StoresABody {
    CreateStack,
    UpdateStack,
    CreateChangeSet,
    CreateStackSet,
}

const STORES_A_BODY: &[StoresABody] = &[
    StoresABody::CreateStack,
    StoresABody::UpdateStack,
    StoresABody::CreateChangeSet,
    StoresABody::CreateStackSet,
];

/// A settled stack for a row that acts on one that already exists, deployed from a body whose
/// text is wholly ASCII so the row's own body is the only one under test.
async fn settled_stack(case: &str, router: &MultiServiceRouter) {
    let (status_code, body) = create_stack(router, STACK, &described("plain"), &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the stack under test must be created, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// Submit `template` through `op` on a fresh router and read the stored body back through the
/// operation that serves the record `op` wrote.
async fn round_trip(case: &str, op: StoresABody, template: &str) -> String {
    let router = router();
    let (status_code, body) = match op {
        StoresABody::CreateStack => create_stack(&router, STACK, template, &[]).await,
        StoresABody::UpdateStack => {
            settled_stack(case, &router).await;
            call(
                &router,
                "UpdateStack",
                &[member("StackName", STACK), member("TemplateBody", template)],
            )
            .await
        }
        StoresABody::CreateChangeSet => {
            settled_stack(case, &router).await;
            call(
                &router,
                "CreateChangeSet",
                &[
                    member("StackName", STACK),
                    member("ChangeSetName", CHANGE_SET),
                    member("TemplateBody", template),
                ],
            )
            .await
        }
        StoresABody::CreateStackSet => {
            call(
                &router,
                "CreateStackSet",
                &[
                    member("StackSetName", STACK_SET),
                    member("TemplateBody", template),
                ],
            )
            .await
        }
    };
    assert_eq!(
        status_code, 200,
        "case {op:?} {case:?}: the model constrains no character set on a template body, so the \
         call must be admitted, got {status_code} {body}"
    );
    let (status_code, read) = match op {
        StoresABody::CreateStack | StoresABody::UpdateStack => {
            wait_for_the_transition().await;
            get_template(&router, STACK, None).await
        }
        StoresABody::CreateChangeSet => {
            call(
                &router,
                "GetTemplate",
                &[
                    member("StackName", STACK),
                    member("ChangeSetName", CHANGE_SET),
                ],
            )
            .await
        }
        StoresABody::CreateStackSet => {
            call(
                &router,
                "DescribeStackSet",
                &[member("StackSetName", STACK_SET)],
            )
            .await
        }
    };
    assert_eq!(
        status_code, 200,
        "case {op:?} {case:?}: the stored template must be readable, got {status_code} {read}"
    );
    xml_text(&read, "TemplateBody").unwrap_or_default()
}

#[tokio::test]
async fn a_stacks_template_body_is_held_as_ascii_and_read_back_that_way() {
    for BodyCase { case, sent, held } in body_cases() {
        let reported = round_trip(case, StoresABody::CreateStack, &sent).await;
        assert_eq!(
            reported, held,
            "case {case:?}: expected the stored body held as ASCII, got {reported:?}"
        );
        assert_eq!(
            reported.chars().count(),
            sent.chars().count(),
            "case {case:?}: expected one character held per character sent, got {reported:?}"
        );
        assert!(
            reported.is_ascii(),
            "case {case:?}: expected a body the store can hold, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn both_template_stages_and_the_stacks_description_report_the_held_text() {
    let case = "every width at once, read at both stages and as the stack's Description";
    let sent = described(
        "\u{90e8}\u{7f72} \u{1f680} \u{2014} caf\u{e9} na\u{ef}ve, \
         Z\u{fc}rich, \u{41c}\u{43e}\u{441}\u{43a}\u{432}\u{430}",
    );
    let held = described("?? ? ? caf? na?ve, Z?rich, ??????");
    let held_description = "?? ? ? caf? na?ve, Z?rich, ??????";

    let router = router();
    let (status_code, body) = create_stack(&router, STACK, &sent, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the stack must be created, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    // "To get the user-submitted template, specify Original": what the caller submitted is
    // what the STORE holds of it, so the original stage is the held body too.
    for stage in [None, Some("Original"), Some("Processed")] {
        let (status_code, read) = get_template(&router, STACK, stage).await;
        assert_eq!(
            status_code, 200,
            "case {case:?} at stage {stage:?}: GetTemplate must answer, got {status_code} {read}"
        );
        let reported = xml_text(&read, "TemplateBody").unwrap_or_default();
        assert_eq!(
            reported, held,
            "case {case:?} at stage {stage:?}: expected the held body, got {reported:?}"
        );
    }

    let (status_code, described_stack) = describe_stack(&router, STACK).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeStacks must answer, got {status_code} {described_stack}"
    );
    let reported = xml_text(&described_stack, "Description").unwrap_or_default();
    assert_eq!(
        reported, held_description,
        "case {case:?}: expected the stack's Description read from the held body, got \
         {reported:?}"
    );

    // The SUMMARY of the stored template is read from that same text.
    let (status_code, summary) = call(
        &router,
        "GetTemplateSummary",
        &[member("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: GetTemplateSummary must answer, got {status_code} {summary}"
    );
    let reported = xml_text(&summary, "Description").unwrap_or_default();
    assert_eq!(
        reported, held_description,
        "case {case:?}: expected the summary's Description read from the held body, got \
         {reported:?}"
    );

    // ...and a client that resubmits the text a read SERVED has submitted the template the
    // stack already carries, which CloudFormation refuses as a no-op rather than deploying.
    let (status_code, refused) = call(
        &router,
        "UpdateStack",
        &[member("StackName", STACK), member("TemplateBody", &held)],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the resubmitted served text to be no update at all, got \
         {status_code} {refused}"
    );
    assert_eq!(
        xml_text(&refused, "Message").as_deref(),
        Some("No updates are to be performed."),
        "case {case:?}: expected CloudFormation's own no-op refusal, got {refused}"
    );
}

#[tokio::test]
async fn every_operation_that_stores_a_template_body_holds_its_text_as_ascii() {
    // One row of the table, driven through every writer and read back through the operation
    // that serves each record's template: no writer may hold a body differently.
    let case = "a Latin-1 letter, a three-byte encoding and an emoji in one body";
    let sent = described("caf\u{e9} \u{90e8}\u{7f72} \u{1f680}");
    let held = described("caf? ?? ?");
    for op in STORES_A_BODY {
        let reported = round_trip(case, *op, &sent).await;
        assert_eq!(
            reported, held,
            "case {op:?} {case:?}: expected the stored body held as ASCII, got {reported:?}"
        );
    }
}
