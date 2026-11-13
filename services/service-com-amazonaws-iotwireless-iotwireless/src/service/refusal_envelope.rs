// SPDX-License-Identifier: CC0-1.0

//! Which ENVELOPE this service answers a request its front door refused in.
//!
//! # The rule
//!
//! Two stacks front this service's operations, and a request refused before any handler ran
//! is answered by the one that fronts the operation it addressed:
//!
//! * a MULTICAST-GROUP operation — one addressed under the `/multicast-groups` resource path
//!   ([`MULTICAST_GROUP_PATH`]) — is fronted by the multicast stack, which answers the
//!   GENERIC constraint refusal: the code this protocol carries for one
//!   (`ValidationException`) and the text in that envelope's own `message` member, quoting
//!   the value it rejected;
//! * every other operation answers the MODELLED error shape, whose text travels in the
//!   `Message` member every error shape of this model declares
//!   (`smithy_query(query="shape",
//!   shape="AccessDeniedException,ConflictException,InternalServerException,ResourceNotFoundException,ThrottlingException,TooManyTagsException,ValidationException")`:
//!   one `Message` member each, none with a `@jsonName`, so a restJson1 body carries it under
//!   that Smithy name), with the rejected value withheld.
//!
//! Both halves of that split — the member the text is in, and whether the value is quoted —
//! are observable on each stack together, which is what makes it one boundary between two
//! front doors rather than two coincidences. WHICH side an operation falls on is recorded
//! for `CreateMulticastGroup` on the multicast side and for `CreateDestination`,
//! `CreateDeviceProfile`, `CreateNetworkAnalyzerConfiguration` and `CreateWirelessGateway`
//! on the other; every other operation follows the one property those two sets differ in —
//! the resource path its `@http` binding addresses — rather than a row of its own.
//!
//! The WORDS are not chosen here: [`super::refusal_wording`] composes both wordings over the
//! same violations, because the refusal seam is handed the violations alone and cannot see
//! which operation they belong to. That is exactly why the ENVELOPE is stated here, at the
//! one seam that sees both the request and the whole rendered response.
//!
//! Only a refusal the framework's own renderer wrote is touched. An error a HANDLER returned
//! is serialized from the model's own shape, so it already carries its `Message` member and
//! its own words; a success, and a failure whose class this model does not declare (the
//! scaffold's 501), keep every byte their renderer wrote.
//!
//! Nothing here is this service's own machinery: the per-request note that carries the words
//! from the refusal seam to the rendered response, and the operation wrapper that opens its
//! scope, are `service_common::restated_refusal`, and the body rewriting is
//! `service_common::error_detail_member`. What this file states, and all it states, is which
//! stack fronts a request and which two members this service's two envelopes name — the one
//! part of the rule that is a fact of THIS service's wire.
//!
//! The structural home for a per-stack envelope is the framework's refusal renderer, which
//! writes one envelope — and one `message` member — for every service; that renderer is in
//! the agent-immutable `vendor/` tree, so this service states the difference at the one seam
//! it can. A failure answered before any operation is served (a request naming no modelled
//! operation, a transport-level failure, an authentication refusal) is therefore not reached
//! from here.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::error_detail_member::{
    with_renamed_detail_member, with_restated_detail_member,
};
use service_common::restated_refusal::RestatedRefusalOperation;

/// The resource path the multicast-group stack is addressed under, from those operations'
/// own `@http` bindings (`smithy_query(query="operation",
/// operation="CreateMulticastGroup,GetMulticastGroup,…")`: `/multicast-groups`,
/// `/multicast-groups/{Id}` and the sub-resources below it).
const MULTICAST_GROUP_PATH: &str = "/multicast-groups";

/// The member the framework's own refusal renderer writes an error's text in: the member of
/// the synthetic shape it builds a pre-handler refusal from, which the multicast stack's
/// envelope keeps.
const RENDERED_MESSAGE_MEMBER: &str = "message";

/// The member every error shape of this model declares its text in, which the other stack
/// reports it in; see the module header for the `smithy_query` call it comes from.
const MODELLED_MESSAGE_MEMBER: &str = "Message";

/// Every error shape this model declares, by the wire code (`__type`) it renders under — the
/// same `smithy_query` call the module header cites, which is where this list comes from and
/// the only thing that may extend it.
///
/// The restatement is keyed on THIS set rather than on the status, so a failure whose class
/// this service does not model — the scaffold's own 501 — keeps the member the framework
/// wrote its text in.
const MODELLED_ERROR_SHAPES: [&str; 7] = [
    "AccessDeniedException",
    "ConflictException",
    "InternalServerException",
    "ResourceNotFoundException",
    "ThrottlingException",
    "TooManyTagsException",
    "ValidationException",
];

/// The wrapper this service hands back from `Service::override_operation`, which serves the
/// routed operation and then answers what it rendered through [`with_refusal_envelope`].
///
/// A `static` so the reference that seam returns outlives the call, and one value for the
/// whole process because the wrapper holds nothing per service but that one function.
pub static REFUSAL_ENVELOPE: RestatedRefusalOperation =
    RestatedRefusalOperation::new(with_refusal_envelope);

/// Whether the multicast-group stack fronts the operation `uri` addresses, and so whether a
/// refusal raised before its handler ran is answered in the generic envelope.
#[must_use]
pub fn multicast_group_stack_fronts(uri: &str) -> bool {
    let path = service_common::request_path(uri);
    match path.strip_prefix(MULTICAST_GROUP_PATH) {
        // The collection itself, or a resource below it — never another path this one is
        // merely a textual prefix of.
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// `resp` as this service answers it, for a response to `req`, with `withheld` the words
/// `Service::override_refusal` stated for the stack that withholds the rejected value.
///
/// A request the multicast stack fronts keeps the envelope the framework rendered, value and
/// `message` member together. Every other refusal the framework rendered reports its text in
/// the `Message` member this model declares, in those withheld words — and in the words the
/// body already carried for a refusal this service did not word (a body the codec could not
/// read as the operation's input, which reached no member to check).
///
/// Everything else stays with the renderer that owns it: the status, the code, every other
/// member, and the whole success path. An error a handler serialized already carries the
/// modelled member, so the rewriting leaves its body — and its own words — untouched.
#[must_use]
pub fn with_refusal_envelope(
    req: &RtRequest,
    resp: RtResponse,
    withheld: Option<String>,
) -> RtResponse {
    if multicast_group_stack_fronts(req.uri()) {
        return resp;
    }
    match withheld {
        Some(words) => with_restated_detail_member(
            resp,
            &MODELLED_ERROR_SHAPES,
            RENDERED_MESSAGE_MEMBER,
            MODELLED_MESSAGE_MEMBER,
            &words,
        ),
        None => with_renamed_detail_member(
            resp,
            &MODELLED_ERROR_SHAPES,
            RENDERED_MESSAGE_MEMBER,
            MODELLED_MESSAGE_MEMBER,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the stack that fronts a request is the one owning the resource path it
    /// addresses — every multicast-group path on one side, every other path on the other.
    #[test]
    fn the_addressed_resource_path_decides_the_stack() {
        // (case, the request target, whether the multicast stack fronts it)
        let rows: &[(&str, &str, bool)] = &[
            ("the multicast group collection", "/multicast-groups", true),
            ("one group", "/multicast-groups/id-1", true),
            ("a group's session", "/multicast-groups/id-1/session", true),
            ("a group's data", "/multicast-groups/id-1/data", true),
            (
                "a group listing with a query string",
                "/multicast-groups?maxResults=1",
                true,
            ),
            (
                "an absolute form target",
                "https://api.iotwireless.us-west-2.amazonaws.com/multicast-groups",
                true,
            ),
            ("the destinations collection", "/destinations", false),
            ("one destination", "/destinations/dest-1", false),
            ("the device profiles", "/device-profiles", false),
            (
                "the network analyzer configurations",
                "/network-analyzer-configurations",
                false,
            ),
            ("the wireless gateways", "/wireless-gateways", false),
            (
                "a group listing under another resource",
                "/fuota-tasks/id-1/multicast-groups",
                false,
            ),
            (
                "a path the prefix is only textual in",
                "/multicast-groups-elsewhere",
                false,
            ),
        ];

        for (case, uri, fronted) in rows {
            assert_eq!(
                multicast_group_stack_fronts(uri),
                *fronted,
                "case {case:?}: expected the multicast stack to front {uri:?} = {fronted}"
            );
        }
    }

    /// Every error shape named here carries its text in the modelled member, and the
    /// framework's own member is the other one, so the two are never the same name.
    #[test]
    fn the_two_envelopes_name_two_members() {
        assert_ne!(
            RENDERED_MESSAGE_MEMBER, MODELLED_MESSAGE_MEMBER,
            "a restatement that moved nothing would be a rename of one member onto itself"
        );
        assert!(
            MODELLED_ERROR_SHAPES.contains(&"ValidationException"),
            "the refusal's own shape must be among the shapes the restatement owns"
        );
    }
}
