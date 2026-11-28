// SPDX-License-Identifier: CC0-1.0

//! WHICH RESOURCE an action of this service is judged against — the one half of the
//! authorization gate that is about the REQUEST's target rather than about the caller.
//!
//! IAM evaluates an action against the resource the request addresses, so the gate has to
//! answer what a request addressed: the ARN it names of the family its URI space addresses,
//! the one member that space addresses by its own ARN resource id, that family's whole
//! collection, or no resource of this service at all ([`judged_resource`], the one answer
//! [`super::authorize`] is called with). Everything that decides it lives here: the table of
//! this model's URI spaces and the ARN resource types each addresses
//! ([`RESOURCE_FAMILIES`]), the spaces that hold no resource of this service
//! ([`RESOURCE_FREE_SPACES`]), the spaces whose target is the ARN the request itself carries
//! ([`ARN_ADDRESSED_SPACES`]), and the readers over a request target that place one
//! ([`addressed`], [`resource_space`]).
//!
//! Beside [`super`] rather than in it because the two halves answer different questions —
//! WHO is calling and WHAT they addressed — and this one is a table of this model's wire
//! surface that grows with every URI space a model refresh adds, while the decision it feeds
//! does not. Nothing here reads the store, serves a response or decides anything about a
//! caller: it maps a request target onto the ARN the shared evaluator is asked about.
//!
//! `super::tests` holds the cases for both halves, so each one still reads the private items
//! it is about.

use super::*;

/// The ARN of the resource of `resource_type` that `resource_id` names, in the Region the
/// caller signed for — built through the same builder every resource ARN this service
/// mints comes from, so what IAM evaluates is spelled exactly as the ARNs of the resources
/// the action reaches are, and a policy scoped to them covers it.
pub(super) fn family_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    resource_type: &str,
    resource_id: &str,
) -> String {
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx.parts()),
        &format!("{resource_type}/{resource_id}"),
    )
}

/// The ARN a request that names no single resource of its own is judged against: every
/// resource of `resource_type` in the Region the caller signed for.
///
/// A create and a list name no one resource — the id a create would mint does not exist
/// while the create is still being authorized — so what IAM evaluates is the collection
/// the action reaches, written with IAM's own resource wildcard.
#[must_use]
pub fn collection_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    resource_type: &str,
) -> String {
    family_arn(ctx, resource_type, ANY_RESOURCE)
}

/// The ARN an operation is judged against, given the ARN of one of this service's own
/// resources that the request carries (`named`, when it carries one) and the request `uri`
/// it was addressed to.
///
/// IAM evaluates an action against the resource the request addresses, and a request
/// addresses one in exactly four ways:
///
/// * it NAMES it, by spelling the ARN of a resource of the family its URI space addresses —
///   `GetCustomModel$modelIdentifier` spelled as a `custom-model` ARN,
///   `DeleteResourcePolicy$resourceArn`. The resource TYPE has to be that family's, because
///   several operations carry an ARN of ANOTHER family beside their target —
///   `CreateModelInvocationJob$modelId` admits a `foundation-model`, `custom-model` or
///   `inference-profile` ARN, `ListModelCopyJobs$sourceModelArnEquals` a source model — and
///   the model a job runs is not the resource the job-creating action acts on;
/// * it carries that ONE resource's own ARN resource id in its URI instead of the whole ARN
///   ([`ID_ADDRESSED_FAMILIES`]) — the bare id the identifier member's modelled form admits
///   beside the ARN — so the resource it addresses is the member that id names, and its ARN
///   is built from the id by this service's own builder for the family ([`family_arn`]);
/// * it names no single member of that family — a create, a listing, a read addressed by a
///   NAME — and so addresses the family's whole collection in this account and Region
///   ([`collection_arn`]): the id a create would mint does not exist while the create is
///   still being authorized, and the id behind a name is one this service minted, so
///   neither is an ARN this gate could resolve without reading the store;
/// * its URI space addresses no family of its own and the request carries the TARGET's ARN
///   instead ([`ARN_ADDRESSED_SPACES`]) — the tagging and resource-policy operations, whose
///   target may be a resource of any taggable family.
///
/// Anything else is judged against [`ANY_RESOURCE`], as IAM writes it for a request that
/// addresses no one resource: an account-level setting, a configuration, and an ARN in a
/// request whose space addresses neither that ARN's family nor the ARN the caller named.
#[must_use]
pub fn judged_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    named: Option<String>,
    uri: &str,
) -> String {
    match addressed(uri) {
        Addressed::Family(resource_types) => {
            // The ARN the request spells, only where it is one of THIS family's.
            if let Some(arn) = named {
                if resource_types
                    .iter()
                    .any(|resource_type| crate::state::names_arn_of(&arn, resource_type))
                {
                    return arn;
                }
            }
            // Else the one member the URI addresses by its own id, where it does.
            if let Some((resource_type, resource_id)) = id_addressed_member(uri) {
                return family_arn(ctx, resource_type, resource_id);
            }
            // Else the family's collection, written with the type its own resources carry
            // first.
            match resource_types.first() {
                Some(resource_type) => collection_arn(ctx, resource_type),
                None => ANY_RESOURCE.to_string(),
            }
        }
        Addressed::NamedByTheRequest => named.unwrap_or_else(|| ANY_RESOURCE.to_string()),
        Addressed::NoResource => ANY_RESOURCE.to_string(),
    }
}

/// The ARN resource type and resource id of the ONE member `uri` addresses by that
/// member's own id, or `None` for a request that addresses no single member that way.
///
/// The id sits at the segment after the space ([`ID_ADDRESSED_FAMILIES`]), and is read as
/// an id only where it is spelled as THAT family spells the resource id inside its ARNs
/// ([`IdAddressedFamily::admits_id`]) — so the percent-encoded ARN a caller may put in the
/// same label, a sub-resource segment, and a label of any other form fall through to the
/// ARN the request named or to the family's collection rather than being read as an id, and
/// the ARN this gate builds is always one a resource of that family could carry.
pub(super) fn id_addressed_member(uri: &str) -> Option<(&'static str, &str)> {
    let space = path_segment(uri, 0);
    let family = ID_ADDRESSED_FAMILIES
        .iter()
        .find(|family| family.space == space)?;
    let identifier = path_segment(uri, 1);
    family
        .admits_id(identifier)
        .then_some((family.resource_type, identifier))
}

/// One URI space whose single-resource operations address that resource by its OWN ARN
/// resource id.
pub(super) struct IdAddressedFamily {
    /// The URI space: the first path segment of those operations' `@http` URIs, with the
    /// id at the segment right after it.
    pub(super) space: &'static str,
    /// The ARN resource type the id belongs to — the type this service's own ARN builder
    /// for that family spells.
    pub(super) resource_type: &'static str,
    /// The lengths, inclusive, the model admits for that resource id.
    pub(super) id_length: (usize, usize),
}

impl IdAddressedFamily {
    /// Whether `value` is spelled as this family spells the resource id inside its ARNs: a
    /// run of lowercase letters and digits of a length the model admits, so an ARN built
    /// from it is one a resource of this family could carry.
    pub(super) fn admits_id(&self, value: &str) -> bool {
        let (shortest, longest) = self.id_length;
        let length = value.chars().count();
        length >= shortest
            && length <= longest
            && value
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    }
}

/// Every URI space whose single-resource operations address that resource by its OWN ARN
/// resource id, with the ARN resource type the id belongs to and the lengths the model
/// admits for it.
///
/// A space is here exactly when the identifier member its operations carry declares the
/// bare ARN resource id as an alternative beside the whole ARN, so a value that is no ARN
/// IS that resource's id and the ARN follows from it without reading the store:
///
/// * `GuardrailIdentifier` — `^(([a-z0-9]+)|(arn:…:guardrail/[a-z0-9]+))$`, whose id is a
///   `GuardrailId`: `^[a-z0-9]+$` with `@length(max: 64)`;
/// * `ModelInvocationJobIdentifier` — `^((arn:…:model-invocation-job/)?[a-z0-9]{12})$`;
/// * `AdvancedPromptOptimizationJobIdentifier` — the same shape over that family
///   (`smithy_query(query="operation",operation="GetGuardrail,GetModelInvocationJob,GetAdvancedPromptOptimizationJob")`).
///
/// Every OTHER space of [`RESOURCE_FAMILIES`] is absent for one of two stated reasons, and
/// a request there keeps the resource it had:
///
/// * its identifier admits nothing but the ARN (`EvaluationJobIdentifier`), or the label IS
///   the ARN (`automated-reasoning-policies`, `model-copy-jobs`, `prompt-routers`), so the
///   ARN the request named is already the resource it addresses;
/// * its bare alternative is a resource NAME (`ModelIdentifier` and
///   `ImportedModelIdentifier`'s `([0-9a-zA-Z][_-]?)+`, `ProvisionedModelId`,
///   `CustomModelDeploymentIdentifier`, `ModelImportJobIdentifier`'s job name), or its ARN
///   carries a second segment this gate cannot know (`custom-model/<base model>/<id>`,
///   `model-customization-job/<base model>/<id>`), or the id belongs to either of two
///   resource types (`InferenceProfileIdentifier`) — in each case the id this service
///   minted is not derivable from the label, so the action reaches the family's collection.
pub(super) const ID_ADDRESSED_FAMILIES: &[IdAddressedFamily] = &[
    IdAddressedFamily {
        space: "advanced-prompt-optimization-jobs",
        resource_type: "advanced-prompt-optimization-job",
        id_length: (12, 12),
    },
    IdAddressedFamily {
        space: "guardrails",
        resource_type: "guardrail",
        id_length: (1, 64),
    },
    IdAddressedFamily {
        space: "model-invocation-job",
        resource_type: "model-invocation-job",
        id_length: (12, 12),
    },
];

/// What the URI space a request was sent to addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Addressed {
    /// A family of this service's OWN resources, named by these ARN resource types
    /// ([`RESOURCE_FAMILIES`]).
    Family(&'static [&'static str]),
    /// The resource the REQUEST names, of whichever family it belongs to
    /// ([`ARN_ADDRESSED_SPACES`]).
    NamedByTheRequest,
    /// No resource of this service: a space in [`RESOURCE_FREE_SPACES`], or a target this
    /// model binds no operation to.
    NoResource,
}

/// How `uri` addresses a resource, read off the first path segment of the request target
/// that names a SPACE ([`resource_space`], which steps over the version prefix the
/// data-retention-storage URIs carry).
pub(super) fn addressed(uri: &str) -> Addressed {
    let space = resource_space(uri);
    if let Some((_, resource_types)) = RESOURCE_FAMILIES
        .iter()
        .find(|(known, _)| *known == space)
    {
        return Addressed::Family(resource_types);
    }
    if ARN_ADDRESSED_SPACES.contains(&space) {
        return Addressed::NamedByTheRequest;
    }
    // Every remaining space this service SERVES holds no resource of its own
    // ([`RESOURCE_FREE_SPACES`]); a target outside this model's own spaces reaches no
    // operation at all, so the framework answers it as a routing failure. The invariant is
    // about the MODEL rather than about any request, so no caller can reach this assertion,
    // and `tests::every_uri_space_of_this_service_is_classified` holds it in every build.
    debug_assert!(
        RESOURCE_FREE_SPACES.contains(&space) || !crate::ROUTE_PATH_SEGMENTS.contains(&space),
        "every URI space this service serves is classified"
    );
    Addressed::NoResource
}

/// The `index`-th path segment of a request target, in either of the two forms one arrives
/// in, or the empty string where the target carries no such segment.
///
/// Both readings are the shared crate's: the target is reduced to its PATH by the one
/// reader of a whole request URI (`service_common::uri_path`, which strips the scheme, the
/// authority, the query string and the fragment) and split by the one segment reader
/// (`service_common::nth_path_segment`) — so the origin form a client sends
/// (`/guardrails?maxResults=1`) and the absolute form the AWS SDK sends
/// (`https://bedrock.<region>.amazonaws.com/guardrails`) read the same, and this crate grows
/// no private copy of either.
pub(super) fn path_segment(uri: &str, index: usize) -> &str {
    ::service_common::nth_path_segment(::service_common::uri_path(uri), index).unwrap_or_default()
}

/// The path segment of `uri` that names the URI SPACE the request was sent to: the first
/// one, or the one after it where the first is the VERSION prefix this model gives some of
/// its URIs ([`VERSION_PREFIXES`]).
///
/// A version prefix names no resource — `/v1/projects/{projectIdentifier}/data_retention/
/// storage/{modelProvider}` addresses the same project `/projects/{projectIdentifier}`
/// does — so the space is read past it, and a target that carries the prefix and nothing
/// else reads as the empty space, which addresses no resource like any other unbound
/// target.
pub(super) fn resource_space(uri: &str) -> &str {
    let first = path_segment(uri, 0);
    if VERSION_PREFIXES.contains(&first) {
        return path_segment(uri, 1);
    }
    first
}

/// The path segments this model spells as a VERSION of the API rather than as a space
/// holding resources: the prefix the data-retention-storage URIs carry
/// (`smithy_query(query="operation",operation="GetAccountDataRetentionStorage,GetProjectDataRetentionStorage")`
/// reports `/v1/data_retention/storage/{modelProvider}` and
/// `/v1/projects/{projectIdentifier}/data_retention/storage/{modelProvider}`), where every
/// other URI of this model starts at its space.
pub(super) const VERSION_PREFIXES: &[&str] = &["v1"];

/// The URI spaces whose operations address a resource of ANY family, named by the request
/// itself: the tagging operations, whose `resourceARN` is "the Amazon Resource Name (ARN) of
/// the resource" being tagged, and the resource-policy operations, whose `resourceArn` is
/// the resource the policy is attached to
/// (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource,PutResourcePolicy,GetResourcePolicy,DeleteResourcePolicy")`).
///
/// They map to no entry of [`RESOURCE_FAMILIES`] because the family is not a property of the
/// space: it is whichever family the ARN in the request belongs to, which is exactly the
/// resource IAM evaluates such an action against.
pub(super) const ARN_ADDRESSED_SPACES: &[&str] = &[
    "listTagsForResource",
    "resource-policy",
    "tagResource",
    "untagResource",
];

/// Every URI space of this service that addresses a family of its OWN resources, and the
/// ARN resource types that family's members are named by — the FIRST being the one a
/// resource this service mints in that space carries, so it is the type the family's
/// collection is written with.
///
/// The spaces are the first segments of this model's `@http` URIs (`OPERATIONS.md`, the
/// same list `crate::ROUTE_PATH_SEGMENTS` claims a request by, and
/// [`tests::every_uri_space_of_this_service_is_classified`] holds this table to it), and
/// every resource type is one [`crate::state`]'s ARN builder for that family spells — so
/// the collection a request is judged against is written exactly as the ARNs of the
/// resources it reaches are, and a policy scoped to this account's resources of that type
/// covers it.
///
/// A space whose operations address no resource of this service is in
/// [`RESOURCE_FREE_SPACES`]; one whose target is the ARN the REQUEST carries is in
/// [`ARN_ADDRESSED_SPACES`].
pub(super) const RESOURCE_FAMILIES: &[(&str, &[&str])] = &[
    (
        "advanced-prompt-optimization-job",
        &["advanced-prompt-optimization-job"],
    ),
    (
        "advanced-prompt-optimization-jobs",
        &["advanced-prompt-optimization-job"],
    ),
    (
        "automated-reasoning-policies",
        &["automated-reasoning-policy"],
    ),
    ("create-foundation-model-agreement", &[FOUNDATION_MODEL]),
    ("custom-models", &[crate::state::CUSTOM_MODEL_RESOURCE_TYPE]),
    ("delete-foundation-model-agreement", &[FOUNDATION_MODEL]),
    ("evaluation-job", &["evaluation-job"]),
    ("evaluation-jobs", &["evaluation-job"]),
    ("foundation-model-availability", &[FOUNDATION_MODEL]),
    ("foundation-models", &[FOUNDATION_MODEL]),
    ("guardrails", &["guardrail"]),
    ("imported-models", &["imported-model"]),
    // This space holds BOTH profile kinds: a create mints the application profile
    // (`crate::state::application_inference_profile_arn`), while the profiles AWS publishes
    // are system-defined ones (`crate::state::system_defined_inference_profile_arn`), and a
    // read or a delete may name either.
    (
        "inference-profiles",
        &["application-inference-profile", "inference-profile"],
    ),
    ("list-foundation-model-agreement-offers", &[FOUNDATION_MODEL]),
    (
        "model-customization",
        &[crate::state::CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE],
    ),
    ("model-copy-jobs", &["model-copy-job"]),
    ("model-customization-jobs", &["model-customization-job"]),
    ("model-import-jobs", &["model-import-job"]),
    ("model-invocation-job", &["model-invocation-job"]),
    ("model-invocation-jobs", &["model-invocation-job"]),
    ("projects", &[PROJECT]),
    // Every prompt-router ARN this service mints spells `default-prompt-router/<id>`, as
    // `PromptRouterArn` does (`crate::state::prompt_router_arn`); the taggable form of the
    // same resource is `prompt-router/<id>`, which a caller may name it by.
    ("prompt-routers", &["default-prompt-router", "prompt-router"]),
    ("provisioned-model-throughput", &["provisioned-model"]),
    ("provisioned-model-throughputs", &["provisioned-model"]),
    ("trusted-access-programs", &[TRUSTED_ACCESS_PROGRAM]),
];

/// The URI spaces whose operations address no resource of this service, so an action of
/// theirs is judged against [`ANY_RESOURCE`], as IAM writes it for such a request: the
/// account-level settings and configurations (`/data-retention`, `/logging/modelinvocations`,
/// `/use-case-for-model-access`, `/enforcedGuardrailsConfiguration`), and
/// `/marketplace-model`, whose endpoints are named by an Amazon SageMaker ARN rather than
/// by one of this service's (`crate::state::marketplace_model_endpoint_arn`).
///
/// Stated rather than left to the fall-through, so a space this model adds later cannot
/// become an unjudged default without a case saying so
/// ([`tests::every_uri_space_of_this_service_is_classified`]).
pub(super) const RESOURCE_FREE_SPACES: &[&str] = &[
    "data-retention",
    "enforcedGuardrailsConfiguration",
    "logging",
    "marketplace-model",
    "use-case-for-model-access",
    // The version prefix itself ([`VERSION_PREFIXES`]): the space a versioned URI addresses
    // is read past it, and the prefix alone names nothing.
    "v1",
];

/// The ARN resource type a PROJECT is named by: `ProjectArn` is
/// `arn:aws:bedrock:<region>:<account>:project/<id>`
/// (`smithy_query(query="operation",operation="GetProject")`), so the collection a request
/// about one is judged against is this account's projects in this Region.
pub(super) const PROJECT: &str = "project";

/// The ARN resource type a TRUSTED ACCESS PROGRAM is named by:
/// `TrustedAccessProgramIdentifier` admits
/// `arn:aws:bedrock::aws:trusted-access-program/<id>`
/// (`smithy_query(query="operation",operation="GetTrustedAccessProgram")`). AWS publishes
/// the programs, so a program's ARN carries no account of this caller's — the same position
/// a foundation model is in ([`FOUNDATION_MODEL`]).
pub(super) const TRUSTED_ACCESS_PROGRAM: &str = "trusted-access-program";

/// The ARN resource type a foundation model is named by: AWS publishes it, so its ARN
/// carries no account, and the collection a request about one is judged against is this
/// Region's.
pub(super) const FOUNDATION_MODEL: &str = "foundation-model";
