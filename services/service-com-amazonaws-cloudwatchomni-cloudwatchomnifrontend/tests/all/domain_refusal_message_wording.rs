// SPDX-License-Identifier: CC0-1.0

//! The two refusals a CloudWatchOmni domain write earns, read off the rpcv2Cbor WIRE as a
//! client reads them.
//!
//! `ConflictException` and `ResourceNotFoundException` both carry a required `message` the
//! model fixes no text for, so each refusal class has one sentence of its own and that
//! sentence is a requirement of the refusal. The rules:
//!
//! * a delete refused because the domain still holds children of another kind states the
//!   kind, the identifier the request addressed, HOW MANY children the store holds for it
//!   and what to remove first — the count read from state at the moment of the refusal;
//! * a write addressing a domain the store holds nothing under is that kind's ONE fixed
//!   text, with no variable part, so nothing the caller submitted is reflected back.
//!
//! The handler-level tables for both rules live in the suites that own those resources —
//! [`super::domain_space_lifecycle`] walks the space count up and back down through
//! `DeleteDomain`, and [`super::resource_update`] spans the `DomainId` class across every
//! operation that reports a missing domain. This suite adds the dimension neither covers:
//! the sentences as the WIRE carries them, decoded out of the rpcv2Cbor error envelope, so
//! a message that never reaches a client intact fails here. The fixed text is written out
//! as literals rather than read from the crate, and the variables are decomposed out of the
//! message the wire returned, so a template that drifted to another wording, dropped the
//! count, or started quoting and echoing the requested id fails even though every handler
//! still reaches it through one shared function.
//!
//! The rows span what the rules generalise over: the live count of children (one — the
//! smallest count that blocks the delete — then two and three), the missing `domainId`
//! submitted over the `^d-[0-9a-z]{1,25}$` class the model admits, the members a patch
//! carries beside it, and the operation reporting the miss (a patch and a read).
//!
//! Every call goes through the emulator the access-profile suite builds
//! ([`super::access_profile_lifecycle::Omni`]), so the plumbing that signs, dispatches and
//! shares one engine is stated once for the crate rather than copied here. Every assertion
//! names its row and the actual value.

use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::types::{
    DeleteDomainInput, GetDomainInput, UpdateDomainInput, DELETE_DOMAIN_INPUT_SCHEMA,
    GET_DOMAIN_INPUT_SCHEMA, UPDATE_DOMAIN_INPUT_SCHEMA,
};

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads — so the spaces the setup creates are the ones the refused
/// delete counts.
use super::access_profile_lifecycle::Omni;
/// The `__type` and `message` of an rpcv2Cbor error envelope, decoded as CBOR, so the
/// message asserted on is the WHOLE message.
use super::validation_message_wording::error_envelope;

/// The status and error code the model declares for a conflict (`@httpError 409`).
const CONFLICT: (u16, &str) = (409, "ConflictException");

/// The status and error code the model declares for a miss (`@httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// The WHOLE text a miss of a domain carries: one fixed sentence with no variable part.
const DOMAIN_MISS: &str = "Domain not found";

/// The fixed text a delete refused by the spaces a domain holds opens with.
const DELETE_BLOCKED_HEAD: &str = "Cannot delete Domain ";

/// The fixed text between the domain it names and the count it states.
const DELETE_BLOCKED_MIDDLE: &str = " because it has ";

/// The fixed text the sentence closes with, after the count.
const DELETE_BLOCKED_TAIL: &str = " associated Spaces. Delete all Spaces first.";

/// Well-formed `DomainId`s the store holds nothing under, spanning the class the model
/// admits for the member (`^d-[0-9a-z]{1,25}$`): the shortest id the pattern admits, one of
/// mixed digits and letters, and the longest it admits.
///
/// Source: `smithy_query(query="operation", operation="UpdateDomain")`, member `domainId`.
fn missing_domain_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest DomainId the pattern admits", "d-0".to_string()),
        (
            "a DomainId of mixed digits and letters",
            "d-7k2wq0zx94bnra16cte5s".to_string(),
        ),
        (
            "the longest DomainId the pattern admits",
            format!("d-{}", "q".repeat(25)),
        ),
    ]
}

/// The `name`s a patch of a missing domain carries, over the class `UpdateDomainInput.name`
/// admits (`@length(3..63)`, `^[a-z0-9]+(-[a-z0-9]+)*$`): no name at all, the shortest the
/// pattern admits, and one carrying its separator — the miss is reported before any of them
/// is applied, so none of them changes the text.
///
/// Source: `smithy_query(query="operation", operation="UpdateDomain")`, member `name`.
fn patch_names() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("a patch carrying no member but the id", None),
        ("a patch carrying the shortest name", Some("oms")),
        (
            "a patch carrying a hyphenated name",
            Some("omni-refusal-wording"),
        ),
    ]
}

/// Dispatch `operation` over the wire and require it refused the request with
/// `(status, code)`, returning the WHOLE message it stated.
async fn refusal_message<I: SerializableStruct>(
    omni: &Omni,
    case: &str,
    operation: &str,
    input_schema: &Schema,
    input: &I,
    expected: (u16, &str),
) -> String {
    let (status, body) = omni.over_the_wire(operation, input_schema, input).await;
    let (code, message) = error_envelope(case, &body);
    assert_eq!(
        (status, code.as_str()),
        expected,
        "case {case:?}: {operation} must answer {} {}, got {status} {code:?}: {message:?}",
        expected.0,
        expected.1
    );
    message
}

#[tokio::test]
async fn the_delete_refused_by_a_domains_spaces_carries_the_live_count_over_the_wire() {
    let omni = Omni::new();
    let domain_id = omni.domain("a delete over the wire").await;

    // Rows walk the count up: one space is the smallest count that blocks the delete, and a
    // second and a third show the number the wire carries is read from the store rather
    // than fixed.
    for count in 1..=3_usize {
        omni.space("a delete over the wire", &format!("wire-refusal-{count}"))
            .await;
        let case = format!("{count} space(s) in the domain");
        let expected =
            format!("{DELETE_BLOCKED_HEAD}{domain_id}{DELETE_BLOCKED_MIDDLE}{count}{DELETE_BLOCKED_TAIL}");
        let message = refusal_message(
            &omni,
            &case,
            "DeleteDomain",
            DELETE_DOMAIN_INPUT_SCHEMA,
            &DeleteDomainInput {
                domain_id: domain_id.clone(),
            },
            CONFLICT,
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The sentence's only variables are the id the request addressed and the count the
        // store holds, so the text around them is fixed and the id is carried unquoted —
        // decomposed out of the message the WIRE returned rather than out of the
        // expectation above.
        let variables = message
            .strip_prefix(DELETE_BLOCKED_HEAD)
            .and_then(|rest| rest.strip_suffix(DELETE_BLOCKED_TAIL))
            .and_then(|rest| rest.split_once(DELETE_BLOCKED_MIDDLE))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the refusal is not the service's sentence: {message:?}")
            });
        assert_eq!(
            variables,
            (domain_id.as_str(), count.to_string().as_str()),
            "case {case:?}: expected the addressed domain and the live count as the \
             sentence's variables, got {variables:?}"
        );
        assert!(
            !message.contains(&format!("'{domain_id}'")),
            "case {case:?}: the refusal quotes the domainId: {message:?}"
        );
    }
}

#[tokio::test]
async fn a_missing_domain_is_the_kinds_fixed_text_over_the_wire_whichever_write_reports_it() {
    let omni = Omni::new();
    // A live domain, so every refusal below is a lookup that missed rather than a store
    // with nothing in it.
    omni.domain("a miss over the wire").await;

    for (id_case, domain_id) in missing_domain_ids() {
        // A read of the same id, so the invariant below is asserted on the two messages the
        // service actually RETURNS: one kind is worded one way, whichever operation
        // reported it.
        let read = refusal_message(
            &omni,
            id_case,
            "GetDomain",
            GET_DOMAIN_INPUT_SCHEMA,
            &GetDomainInput {
                domain_id: domain_id.clone(),
            },
            NOT_FOUND,
        )
        .await;

        for (patch_case, name) in patch_names() {
            let case = format!("{id_case}, {patch_case}");
            let patched = refusal_message(
                &omni,
                &case,
                "UpdateDomain",
                UPDATE_DOMAIN_INPUT_SCHEMA,
                &UpdateDomainInput {
                    domain_id: domain_id.clone(),
                    name: name.map(str::to_string),
                    ..Default::default()
                },
                NOT_FOUND,
            )
            .await;
            assert_eq!(
                patched, DOMAIN_MISS,
                "case {case:?}: expected {DOMAIN_MISS:?}, got {patched:?}"
            );
            assert_eq!(
                patched, read,
                "case {case:?}: UpdateDomain and GetDomain must word one missing domain the \
                 same way, got {patched:?} and {read:?}"
            );
            assert!(
                !patched.contains(&domain_id),
                "case {case:?}: the refusal reflects the requested domainId ({domain_id:?}): \
                 {patched:?}"
            );
            if let Some(submitted) = name {
                assert!(
                    !patched.contains(submitted),
                    "case {case:?}: the refusal reflects the submitted name ({submitted:?}): \
                     {patched:?}"
                );
            }
        }
    }
}
