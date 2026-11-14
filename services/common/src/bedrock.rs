// SPDX-License-Identifier: CC0-1.0

//! The request-claim rule for the SIX emulated services whose SigV4 signing name is
//! `bedrock`, stated once here and read by every one of them.
//!
//! `aws.auth#sigv4` names `bedrock` on all six Bedrock-family models, while each one
//! serves its own endpoint (`smithy_query(query="service_info")` on each model gives the
//! endpoint prefixes `bedrock`, `bedrock-runtime`, `bedrock-agent`,
//! `bedrock-agent-runtime`, `bedrock-data-automation` and
//! `bedrock-data-automation-runtime` against that ONE signing name). The router resolves
//! a request's routing key from the SigV4 credential scope first, so all six see the key
//! `bedrock`: a service that claims on the signing name ALONE claims every sibling's
//! traffic too, the router finds six claimants and — refusing to serve a request by an
//! arbitrary one — answers `500 AmbiguousService`, a code no AWS model carries, for every
//! Bedrock request. AWS separates them by endpoint HOST, which an emulator cannot read: a
//! client configured with one local endpoint sends the same `Host` for all six.
//!
//! The claim is therefore [`crate::claims_request`]'s rule — the signing name AND the
//! request's own modelled wire surface, which is the first URI path segment for a REST
//! model and the service the `X-Amz-Target` names for the `awsJson1_1` one — narrowed by
//! the split below wherever MORE THAN ONE of these models publishes the same first path
//! segment. Source for every segment named here: `smithy_query(query="digest")` on each
//! of the six models.

use smithy_server_runtime::ErasedServiceDefinition;

/// The `aws.api#service$arnNamespace` every Bedrock-family model shares
/// (`smithy_query(query="service_info")` on each of the six reports `bedrock`), and so
/// the only service field an ARN any of them addresses can carry.
pub const ARN_NAMESPACE: &str = "bedrock";

/// The Smithy shape id of each emulated service that signs as `bedrock`, as
/// `definition().identity().shape_id()` reports it. A member names itself by its shape id
/// rather than by a flag the call site passes, so the ownership table below cannot
/// disagree with the service reading it.
pub const CONTROL_PLANE: &str = "com.amazonaws.bedrock#AmazonBedrockControlPlaneService";
/// The `bedrock-runtime` data plane (`Converse`, `InvokeModel`, `ApplyGuardrail`).
pub const RUNTIME: &str = "com.amazonaws.bedrockruntime#AmazonBedrockFrontendService";
/// The `bedrock-agent` build-time control plane (agents, knowledge bases, flows, prompts).
pub const AGENT: &str = "com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda";
/// The `bedrock-agent-runtime` data plane (`InvokeAgent`, `Retrieve`, `InvokeFlow`).
pub const AGENT_RUNTIME: &str =
    "com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService";
/// The `bedrock-data-automation` build-time control plane (blueprints, projects).
pub const DATA_AUTOMATION: &str =
    "com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService";
/// The `bedrock-data-automation-runtime` data plane — the one `awsJson1_1` model of the
/// six, which publishes no `smithy.api#http` bindings and so posts every operation to `/`.
pub const DATA_AUTOMATION_RUNTIME: &str =
    "com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService";

/// How a first path segment that more than one Bedrock-family model publishes is split
/// between its sharers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shared {
    /// The sharers bind DIFFERENT routes under this segment, so the whole route — method
    /// plus URI pattern — tells them apart: each one claims only a request its own model
    /// binds a route for ([`crate::addresses_modeled_route`]).
    ///
    /// A path under such a segment that NO model binds therefore reaches nobody, and the
    /// router answers it `404 UnknownService` rather than one service's protocol routing
    /// error. That is the narrow cost of not guessing: no AWS operation lives on such a
    /// path, while handing the whole segment to one sharer would make every one of the
    /// other's operations in it unreachable.
    ByRoute,
    /// The sharers bind the SAME routes under this segment, so no part of the request
    /// tells them apart (AWS tells them apart by endpoint host, which is not on the
    /// wire here). The named service answers the whole segment; its sharer's operations
    /// there are unreachable, which keeps the owner's working instead of refusing both
    /// with `500 AmbiguousService`.
    OwnedBy(&'static str),
    /// The sharers bind the SAME routes under this segment and the route's one
    /// `@httpLabel` is the ARN OF THE RESOURCE being tagged, whose `resource-type`
    /// field each sharer's model enumerates in its own taggable-resource shape — and
    /// those enumerations are DISJOINT, each naming only resources that sharer's plane
    /// mints. So the ARN names the sharer: the service whose taggable-resource types
    /// admit the ARN's own type claims the request
    /// ([`tagged_resource_owner`]), and an ARN naming a resource type no sharer admits
    /// goes to the segment's declared owner.
    ByTaggedResourceType,
}

/// Every first URI path segment more than one Bedrock-family model publishes, and how it
/// is split. Each sharer reads this one table, so the claims cannot drift into a path
/// with two claimants (the `AmbiguousService` refusal) or none.
///
/// * `agents`, `flows`, `knowledgebases` — `bedrock-agent` (build time) and
///   `bedrock-agent-runtime` both publish them and bind different routes underneath
///   (`PUT /knowledgebases/` and `POST /knowledgebases/{knowledgeBaseId}/datasources/`
///   against `POST /knowledgebases/{knowledgeBaseId}/retrieve`), so the route decides.
/// * `tags` — the same two both bind `GET`, `POST` and `DELETE /tags/{resourceArn}`, the
///   identical three routes, so neither the method nor the URI template decides. What DOES
///   decide is the ARN the one `@httpLabel` carries: its `resource-type` field, because each
///   of the two models enumerates the resource types its own tagging admits and the two
///   enumerations are disjoint — the build-time plane's taggable resources are the ones it
///   mints (an agent, an agent alias, a knowledge base, a flow, a prompt, a retrieval
///   instruction) and the data plane's are the ones it mints (a session, an
///   advanced-optimize-prompt job). Each endpoint refuses an ARN its own model does not
///   admit (the recorded `bedrock-agent-runtime` refusal of a build-time plane's ARN quotes
///   the RUNTIME's pattern), and answers one it does admit, so the plane that MINTS the
///   named resource is the one that serves the request ([`Shared::ByTaggedResourceType`]).
///   An ARN naming a resource type neither model admits falls to the declared owner, as the
///   three identically-bound tagging segments below do: the data plane.
/// * `listTagsForResource`, `tagResource`, `untagResource` — the `bedrock` control plane
///   and `bedrock-data-automation` both bind `POST` on each of these three, again
///   identically, and the control plane is the member every Bedrock tagging flow uses.
const SHARED_SEGMENTS: &[(&str, Shared)] = &[
    ("agents", Shared::ByRoute),
    ("flows", Shared::ByRoute),
    ("knowledgebases", Shared::ByRoute),
    ("listTagsForResource", Shared::OwnedBy(CONTROL_PLANE)),
    ("tagResource", Shared::OwnedBy(CONTROL_PLANE)),
    ("tags", Shared::ByTaggedResourceType),
    ("untagResource", Shared::OwnedBy(CONTROL_PLANE)),
];

/// Whether the Bedrock-family service whose generated `ServiceDefinition` is `definition`
/// claims `parts`.
///
/// * `path_segments` — that service's generated `crate::ROUTE_PATH_SEGMENTS`, the first
///   URI path segments its model declares.
/// * `rpc_service_shape_id` — `Some(definition.identity().shape_id())` for the one model
///   of the six that declares an RPC protocol (`awsJson1_1`), whose wire form names its
///   service out of band; `None` for the five `restJson1` models, which are discriminated
///   by their path space.
///
/// The signing name is read off `definition`, so a member cannot claim under a name its
/// model does not declare.
pub fn claims_request(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
    path_segments: &[&str],
    rpc_service_shape_id: Option<&str>,
) -> bool {
    let identity = definition.identity();
    if !crate::claims_request(
        parts,
        identity.signing_name(),
        path_segments,
        rpc_service_shape_id,
    ) {
        return false;
    }
    match shared_split(parts.uri.path()) {
        None => true,
        Some(Shared::ByRoute) => {
            crate::addresses_modeled_route(parts, definition)
                && trailing_slash_owner(parts)
                    .is_none_or(|owner| identity.shape_id() == owner)
        }
        Some(Shared::OwnedBy(owner)) => identity.shape_id() == owner,
        Some(Shared::ByTaggedResourceType) => {
            identity.shape_id() == tagged_resource_owner(parts.uri.path())
        }
    }
}

/// Which sharer of the identically-bound `/tags/{resourceArn}` routes mints each taggable
/// resource type, read off each model's own `TaggableResourcesArn`
/// (`smithy_query(query="shape",shape="TaggableResourcesArn")` on each of the two): the
/// build-time plane's resources are the ones it mints, and the data plane's are the ones it
/// mints — the published expression of the second widening the modelled `session` with the
/// optimization job that plane also mints (see that service's own `PUBLISHED_PATTERNS`).
///
/// The two enumerations are disjoint, so a resource type names at most one sharer.
const TAGGED_RESOURCE_OWNERS: &[(&str, &[&str])] = &[
    (
        AGENT,
        &[
            "agent",
            "agent-alias",
            "knowledge-base",
            "flow",
            "prompt",
            "retrieval-instruction",
        ],
    ),
    (AGENT_RUNTIME, &["session", "advanced-optimize-prompt-job"]),
];

/// The sharer a tagging request whose ARN names no taggable resource of EITHER plane
/// belongs to: the segment's declared owner, as the three identically-bound tagging
/// segments of the other pair are owned. Neither plane can resolve such an ARN, so the
/// request still reaches exactly one claimant rather than none or both.
const TAGGED_RESOURCE_DECLARED_OWNER: &str = AGENT_RUNTIME;

/// Which sharer of the identically-bound `/tags/{resourceArn}` routes a request on `path`
/// belongs to: the plane that MINTS the resource the ARN names, read off that ARN's
/// `resource-type` field ([`TAGGED_RESOURCE_OWNERS`]), and otherwise
/// [`TAGGED_RESOURCE_DECLARED_OWNER`].
///
/// The label is read as the codec that deserializes the request reads it, percent escapes
/// resolved ([`crate::percent_decode_path_label`]), because a non-greedy `@httpLabel`
/// carrying an ARN reaches the wire percent-encoded.
fn tagged_resource_owner(path: &str) -> &'static str {
    let Some(label) = super::nth_path_segment(path, 1).map(crate::percent_decode_path_label) else {
        return TAGGED_RESOURCE_DECLARED_OWNER;
    };
    let Some(resource_type) = tagged_resource_type(&label) else {
        return TAGGED_RESOURCE_DECLARED_OWNER;
    };
    TAGGED_RESOURCE_OWNERS
        .iter()
        .find(|(_, types)| types.contains(&resource_type))
        .map_or(TAGGED_RESOURCE_DECLARED_OWNER, |(owner, _)| *owner)
}

/// The `resource-type` field of `arn` where it is an ARN of this family's namespace, or
/// `None` for anything else.
///
/// Read through [`crate::arn`], the one ARN parse of this crate: the resource field of
/// every taggable resource of this family is `<resource-type>/<resource-id>` (a flow alias
/// and a prompt version append a further `/<id>` or `:<version>`, which leaves the type
/// where it is, and [`crate::arn::Arn::resource_type`] ends it on either separator). The
/// comparison is case-sensitive, as an ARN's resource type is.
fn tagged_resource_type(arn: &str) -> Option<&str> {
    crate::arn::parse_of(arn, ARN_NAMESPACE).map(|arn| arn.resource_type())
}

/// The ONE route shape both `agents` sharers bind, which [`Shared::ByRoute`] cannot split:
/// the build-time plane's `POST /agents/{agentId}/` (`PrepareAgent`) and the data plane's
/// `POST /agents/{sessionId}` (`InvokeInlineAgent`). Source:
/// `smithy_query(query="digest")` on each of the two models.
const AGENTS_SEGMENT: &str = "agents";

/// The sharer a request on that one route shape belongs to, or `None` when the request is
/// not on it — in which case the route alone already names one claimant.
///
/// The framework's route matcher trims a trailing `/` from both the request path and the
/// URI template, so the two templates above match the SAME requests and both models bind
/// one: the route rule leaves two claimants, which the router answers `500 AmbiguousService`
/// rather than serving either. AWS separates the two by endpoint host, which is not on the
/// wire here; what IS on the wire is the spelling a client renders from the model's own URI,
/// and those two URIs differ in the trailing slash. So the request's own trailing slash
/// decides: `/agents/<id>/` is the build-time plane's `PrepareAgent`, `/agents/<id>` the
/// data plane's `InvokeInlineAgent`.
fn trailing_slash_owner(parts: &http::request::Parts) -> Option<&'static str> {
    if parts.method != http::Method::POST {
        return None;
    }
    let path = parts.uri.path();
    let ends_with_slash = path.ends_with('/');
    let mut segments = path.trim_matches('/').split('/');
    let (Some(AGENTS_SEGMENT), Some(_label), None) =
        (segments.next(), segments.next(), segments.next())
    else {
        // Anything else is not this route shape: `/agents/` and `/agents` carry no
        // identifier (both models bind `PUT`/`POST /agents/`, which the method and the
        // route already separate), and a deeper path is a route only one of them binds.
        return None;
    };
    Some(if ends_with_slash { AGENT } else { AGENT_RUNTIME })
}

/// How the first segment of `path` is split, or `None` when only one Bedrock-family model
/// publishes it (the overwhelmingly common case, where the first segment already names
/// exactly one member). Matching is exact segment equality, as the framework's own
/// discriminator is.
fn shared_split(path: &str) -> Option<Shared> {
    let segment = super::first_path_segment(path)?;
    SHARED_SEGMENTS
        .iter()
        .find(|(shared, _)| *shared == segment)
        .map(|(_, split)| *split)
}

#[cfg(test)]
mod tests {
    //! The parts of the rule that need no built service: which segments are shared, and
    //! how each one is split.

    use super::*;

    /// Every segment in the table is split exactly one way, and a segment only one model
    /// publishes is not in it at all.
    #[test]
    fn the_shared_segments_are_declared_once_each() {
        // (case, path, expected split)
        let rows: [(&str, &str, Option<Shared>); 14] = [
            ("the agent pair's agents", "/agents/", Some(Shared::ByRoute)),
            (
                "a deep path under it",
                "/agents/AGENT1/agentversions/1/knowledgebases/",
                Some(Shared::ByRoute),
            ),
            ("flows", "/flows/f-1/aliases", Some(Shared::ByRoute)),
            (
                "knowledgebases",
                "/knowledgebases/kb-1/retrieve",
                Some(Shared::ByRoute),
            ),
            (
                "the agent pair's identical tagging routes",
                "/tags/arn%3Aaws%3Abedrock%3A%3A%3Aagent%2Fa1",
                Some(Shared::ByTaggedResourceType),
            ),
            (
                "the same tagging space addressed with a session ARN",
                "/tags/arn%3Aaws%3Abedrock%3A%3A%3Asession%2Fs1",
                Some(Shared::ByTaggedResourceType),
            ),
            (
                "the same tagging space addressed with no ARN at all",
                "/tags/not-an-arn",
                Some(Shared::ByTaggedResourceType),
            ),
            (
                "the control plane's tagging routes",
                "/tagResource",
                Some(Shared::OwnedBy(CONTROL_PLANE)),
            ),
            (
                "untagResource",
                "/untagResource",
                Some(Shared::OwnedBy(CONTROL_PLANE)),
            ),
            (
                "listTagsForResource",
                "/listTagsForResource",
                Some(Shared::OwnedBy(CONTROL_PLANE)),
            ),
            // Segments exactly one of the six models publishes: not shared, so the first
            // segment alone already names one claimant.
            ("the control plane's guardrails", "/guardrails", None),
            ("the data plane's model invocations", "/model/m-1/invoke", None),
            ("a near miss on a shared segment", "/agentsextra", None),
            ("the root path", "/", None),
        ];

        for (case, path, expected) in rows {
            let actual = shared_split(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {path:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A shared segment owned by one member is owned by a member of this family, and no
    /// segment is declared twice.
    #[test]
    fn every_owner_is_a_member_and_no_segment_repeats() {
        let members = [
            CONTROL_PLANE,
            RUNTIME,
            AGENT,
            AGENT_RUNTIME,
            DATA_AUTOMATION,
            DATA_AUTOMATION_RUNTIME,
        ];
        let mut seen: Vec<&str> = Vec::new();
        for (segment, split) in SHARED_SEGMENTS {
            assert!(
                !seen.contains(segment),
                "segment {segment:?} is declared more than once"
            );
            seen.push(segment);
            match split {
                Shared::OwnedBy(owner) => assert!(
                    members.contains(owner),
                    "segment {segment:?} is owned by {owner:?}, which is not a member of \
                     the bedrock family"
                ),
                Shared::ByRoute | Shared::ByTaggedResourceType => {}
            }
        }

        // The tagged-resource split names members of this family too, and its two
        // enumerations are DISJOINT — which is what makes a resource type name at most one
        // sharer.
        let mut claimed: Vec<&str> = Vec::new();
        for (owner, types) in TAGGED_RESOURCE_OWNERS {
            assert!(
                members.contains(owner),
                "the tagged-resource split names {owner:?}, which is not a member of the \
                 bedrock family"
            );
            for resource_type in *types {
                assert!(
                    !claimed.contains(resource_type),
                    "resource type {resource_type:?} is claimed by more than one sharer"
                );
                claimed.push(resource_type);
            }
        }
        assert!(
            members.contains(&TAGGED_RESOURCE_DECLARED_OWNER),
            "the declared owner of the tagging space is a member of the bedrock family"
        );
    }

    /// RULE: on the tagging path space the sharers bind IDENTICALLY, the ARN in the
    /// `@httpLabel` names the plane that MINTS the resource — each sharer's model
    /// enumerates its own taggable resource types and the two enumerations are disjoint —
    /// so the request belongs to that plane, and a label naming no taggable resource of
    /// either goes to the segment's declared owner.
    ///
    /// The rows differ along the resource type the ARN names (each enumeration, both
    /// sharers), the partition, the spelling of the label's separators, the identifier
    /// form, the casing of the fields, another vendor's ARN, and the labels that name no
    /// resource at all.
    #[test]
    fn the_tagging_path_belongs_to_the_plane_that_mints_the_named_resource() {
        /// The label as a client that percent-encodes a non-greedy `@httpLabel` sends it.
        fn encoded(arn: &str) -> String {
            arn.replace(':', "%3A").replace('/', "%2F")
        }

        const SESSION: &str =
            "arn:aws:bedrock:us-west-2:000000000000:session/0cabde93-58b4-4d68-b230-90df328e1acb";

        let rows: Vec<(&str, String, &'static str)> = vec![
            (
                "a session ARN, percent-encoded as an SDK sends it",
                format!("/tags/{}", encoded(SESSION)),
                AGENT_RUNTIME,
            ),
            (
                "the same session ARN sent raw",
                format!("/tags/{SESSION}"),
                AGENT_RUNTIME,
            ),
            (
                "a session ARN in another partition",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:session/\
                         11111111-2222-3333-4444-555555555555"
                    )
                ),
                AGENT_RUNTIME,
            ),
            (
                "a non-canonically cased session ARN, which is no resource type either \
                 model names",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:bedrock:us-west-2:000000000000:Session/\
                         0cabde93-58b4-4d68-b230-90df328e1acb"
                    )
                ),
                AGENT_RUNTIME,
            ),
            (
                "an ARN of the other resource kind the data plane's tagging admits",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:bedrock:us-east-1:000000000000\
                         :advanced-optimize-prompt-job/job-1"
                    )
                ),
                AGENT_RUNTIME,
            ),
            (
                "an agent ARN, naming a build-time plane resource",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:agent/AGENT12345")
                ),
                AGENT,
            ),
            (
                "an agent ARN whose identifier no model's pattern admits: the type still \
                 names the plane",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:agent/NOTASESSION")
                ),
                AGENT,
            ),
            (
                "an agent alias ARN, whose resource field carries two identifiers",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:bedrock:us-east-1:000000000000:agent-alias/AGENT12345/ALIAS12345"
                    )
                ),
                AGENT,
            ),
            (
                "a knowledge-base ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:knowledge-base/KB12345678")
                ),
                AGENT,
            ),
            (
                "a flow alias ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:flow/FLOW123456/alias/AL12345678")
                ),
                AGENT,
            ),
            (
                "a prompt version ARN, whose identifier carries the version",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:prompt/PROMPT1234:3")
                ),
                AGENT,
            ),
            (
                "a retrieval-instruction ARN",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:bedrock:us-east-1:000000000000:retrieval-instruction/ri123456ab"
                    )
                ),
                AGENT,
            ),
            (
                "another service's ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:sqs:us-east-1:000000000000:session/q")
                ),
                AGENT_RUNTIME,
            ),
            (
                "a bedrock ARN naming a resource type neither model's tagging admits",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:guardrail/gr1234567890")
                ),
                AGENT_RUNTIME,
            ),
            (
                "a label that is not an ARN",
                "/tags/not-an-arn".to_string(),
                AGENT_RUNTIME,
            ),
            ("no label at all", "/tags".to_string(), AGENT_RUNTIME),
        ];

        for (case, path, expected) in rows {
            let split = shared_split(&path);
            assert_eq!(
                split,
                Some(Shared::ByTaggedResourceType),
                "case {case:?} ({path}): expected the segment to be split by the tagged \
                 resource's type, got {split:?}"
            );
            let actual = tagged_resource_owner(&path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({path}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: where the two sharers' URI templates differ in NOTHING but a trailing slash —
    /// the one route shape the framework's matcher cannot tell apart — the request's own
    /// trailing slash names the sharer, and every other request on the segment is left to
    /// the route rule (`None` here).
    ///
    /// The rows span the two spellings of that shape, the methods that are not it, the
    /// paths with no identifier, a deeper path under the same segment, and another shared
    /// segment entirely.
    #[test]
    fn the_one_route_shape_both_agent_models_bind_is_split_by_the_trailing_slash() {
        // (case, method, path, the sharer the request belongs to)
        let rows: [(&str, &str, &str, Option<&str>); 9] = [
            (
                "the data plane's inline-agent invocation",
                "POST",
                "/agents/probe-session-0a1f2a1d",
                Some(AGENT_RUNTIME),
            ),
            (
                "a session id that looks like an agent id",
                "POST",
                "/agents/ABCDEFGHIJ",
                Some(AGENT_RUNTIME),
            ),
            (
                "the build-time plane's PrepareAgent, with the slash its URI carries",
                "POST",
                "/agents/ABCDEFGHIJ/",
                Some(AGENT),
            ),
            // Not this route shape: the route rule already names one claimant.
            ("the agent listing, no identifier", "POST", "/agents/", None),
            ("the same segment with no slash", "POST", "/agents", None),
            ("the agent read, another method", "GET", "/agents/a-1/", None),
            (
                "the agent creation, another method",
                "PUT",
                "/agents/a-1/",
                None,
            ),
            (
                "a deeper path under the same segment",
                "POST",
                "/agents/a-1/agentaliases/",
                None,
            ),
            ("another shared segment", "POST", "/flows/f-1", None),
        ];

        for (case, method, path, expected) in rows {
            let parts = parts_for(method, path);
            let actual = trailing_slash_owner(&parts);
            assert_eq!(
                actual, expected,
                "case {case:?} ({method} {path}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The request parts for one method and path, which is all this rule reads.
    fn parts_for(method: &str, path: &str) -> http::request::Parts {
        http::Request::builder()
            .method(method)
            .uri(path)
            .body(())
            .expect("the test request builds")
            .into_parts()
            .0
    }
}
