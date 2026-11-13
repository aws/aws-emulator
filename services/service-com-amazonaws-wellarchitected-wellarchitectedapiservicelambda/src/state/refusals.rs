// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The `(resource key, ShareId)` of the share in `shares` that the invitation
/// `share_invitation_id` mediates, if that share was offered to `principal`.
///
/// Generic over the record because the share maps of the different resource kinds hold
/// different records, while the two things a share is matched on are the same for all of
/// them: `offer` reads the id of the invitation the share raised and the principal it was
/// offered to. The keys are answered OWNED so the search borrows nothing, which is what
/// lets [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::share_by_invitation_mut`]
/// take its one mutable borrow afterwards.
pub(super) fn share_key_by_invitation<Record>(
    shares: &HashMap<String, BTreeMap<String, Record>>,
    share_invitation_id: &str,
    principal: &str,
    offer: impl Fn(&Record) -> (&str, &str),
) -> Option<(String, String)> {
    shares.iter().find_map(|(resource_key, of_resource)| {
        of_resource.iter().find_map(|(share_id, share)| {
            let (invitation, shared_with) = offer(share);
            (invitation == share_invitation_id && shared_with == principal)
                .then(|| (resource_key.clone(), share_id.clone()))
        })
    })
}

/// Take one share out of a share map keyed by the shared resource then by `ShareId`,
/// pruning the resource's map once its last share is gone so a resource nobody shares
/// leaves nothing behind. `None` when that resource holds no such share, which is what
/// tells a delete that the share it was asked to remove does not exist.
///
/// Generic over the share record, so every resource kind whose shares are held that way
/// removes one the same way; what is keyed to a share BESIDES the record — the create
/// token that resolved to it — belongs to the kind, so each `remove_*_share` forgets its
/// own.
pub(super) fn take_share<R>(
    shares_by_resource: &mut HashMap<String, BTreeMap<String, R>>,
    resource_key: &str,
    share_id: &str,
) -> Option<R> {
    let shares = shares_by_resource.get_mut(resource_key)?;
    let record = shares.remove(share_id)?;
    if shares.is_empty() {
        shares_by_resource.remove(resource_key);
    }
    Some(record)
}

/// The form a name whose uniqueness this service decides LOOSELY is compared in:
/// capitalization and whitespace are dropped, so two spellings that differ only in
/// either are the same name.
///
/// It is the comparison the service documents for a workload name — "The name must be
/// unique within an account within an Amazon Web Services Region. Spaces and
/// capitalization are ignored when checking for uniqueness"
/// (`smithy_query(query="shape", shape="WorkloadName")`) — and it is how every name
/// this service meters for uniqueness is compared, so the rule cannot differ between
/// the resources that carry it: a workload name (through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::workload_name_index`]),
/// a review template's name within the account (through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::review_template_holding_name`])
/// and a milestone name within its workload (through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::milestone_holding_name`]).
///
/// It is also the form such a name takes inside a [`parameter_fingerprint`]: "if you retry
/// a request with the same client request token and the same parameters after the original
/// request has completed successfully, the result of the original request is returned"
/// (`ClientRequestToken` doc), and the parameter a loosely-metered name carries is the NAME
/// it names rather than the spelling it was named by — the identity this comparison decides
/// — so a retry that spells it the other way carries the SAME parameters and replays
/// instead of being refused as a different request.
pub fn normalize_unique_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A stable fingerprint of an idempotent request's parameters.
pub fn parameter_fingerprint(parameters: &::serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(parameters.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// The deepest an untyped `Document` member may nest before this service refuses it.
///
/// A `Document` carries whatever JSON the request sent, so its nesting is CALLER-chosen,
/// and every walk of one (fingerprinting it, projecting it) costs one level of nesting:
/// an unbounded document would cost stack depth a caller picks, and stack exhaustion
/// aborts the process rather than failing the call. The ceiling is this emulator's own —
/// far above any realistic `additionalContext` and far below what the stack allows — and
/// a document past it is refused with a modelled 400 by
/// [`document_depth_exceeded`]'s callers.
pub const MAX_DOCUMENT_DEPTH: usize = 32;

/// Whether `document` nests deeper than [`MAX_DOCUMENT_DEPTH`] — the check an operation
/// that stores or fingerprints a caller-supplied `Document` runs BEFORE walking it.
///
/// Iterative, over an explicit worklist, so the check itself costs heap rather than stack
/// however deep the document is.
pub fn document_depth_exceeded(document: Option<&::aws_smithy_types::Document>) -> bool {
    let Some(document) = document else {
        return false;
    };
    let mut pending: Vec<(&::aws_smithy_types::Document, usize)> = vec![(document, 1)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_DOCUMENT_DEPTH {
            return true;
        }
        match node {
            ::aws_smithy_types::Document::Array(items) => {
                pending.extend(items.iter().map(|item| (item, depth + 1)));
            }
            ::aws_smithy_types::Document::Object(members) => {
                pending.extend(members.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// An untyped `Document` member as a JSON value, for [`parameter_fingerprint`]: `null`
/// for a request that carried none, so two requests fingerprint alike exactly when they
/// carry the same document.
///
/// Object members are emitted in SORTED key order, so a document's fingerprint depends
/// neither on the order the wire carried its keys in nor on a `HashMap`'s iteration order
/// (see [`document_json`]).
///
/// The walk is DEPTH-BOUNDED at [`MAX_DOCUMENT_DEPTH`]: nesting is caller-chosen, so the
/// recursion is capped here as well as refused by the handler ahead of it
/// ([`document_depth_exceeded`]), and a node past the cap contributes `null` instead of
/// another frame.
pub fn document_fingerprint(
    document: Option<&::aws_smithy_types::Document>,
) -> ::serde_json::Value {
    document.map_or(::serde_json::Value::Null, |document| {
        document_json(document, 1)
    })
}

/// [`document_fingerprint`]'s walk, carrying the depth of the node it is at.
pub(super) fn document_json(document: &::aws_smithy_types::Document, depth: usize) -> ::serde_json::Value {
    if depth > MAX_DOCUMENT_DEPTH {
        // Unreachable for a request that got past `document_depth_exceeded`; the cap is
        // restated here so the recursion is bounded by this function alone.
        return ::serde_json::Value::Null;
    }
    match document {
        ::aws_smithy_types::Document::Null => ::serde_json::Value::Null,
        ::aws_smithy_types::Document::Bool(value) => ::serde_json::Value::Bool(*value),
        ::aws_smithy_types::Document::String(value) => {
            ::serde_json::Value::String(value.clone())
        }
        ::aws_smithy_types::Document::Number(number) => match number {
            ::aws_smithy_types::Number::PosInt(value) => ::serde_json::Value::from(*value),
            ::aws_smithy_types::Number::NegInt(value) => ::serde_json::Value::from(*value),
            ::aws_smithy_types::Number::Float(value) => {
                ::serde_json::Number::from_f64(*value).map_or(::serde_json::Value::Null, ::serde_json::Value::Number)
            }
        },
        ::aws_smithy_types::Document::Array(items) => ::serde_json::Value::Array(
            items
                .iter()
                .map(|item| document_json(item, depth + 1))
                .collect(),
        ),
        // Members are emitted in SORTED key order. `Document::Object` is a `HashMap`,
        // whose iteration order differs between two instances holding the same keys, and
        // `serde_json::Map` preserves INSERTION order whenever anything in the workspace
        // enables `serde_json/preserve_order`. Without the sort two renderings of the
        // same document can differ, which would make [`parameter_fingerprint`]
        // non-deterministic and turn an idempotent repeat into a second generation. The
        // same guard the `bedrockruntime` service's `document_to_json` carries.
        ::aws_smithy_types::Document::Object(members) => {
            let mut pairs: Vec<(&String, &::aws_smithy_types::Document)> =
                members.iter().collect();
            pairs.sort_unstable_by_key(|(name, _)| *name);
            ::serde_json::Value::Object(
                pairs
                    .into_iter()
                    .map(|(name, member)| (name.clone(), document_json(member, depth + 1)))
                    .collect(),
            )
        }
    }
}

/// A context's typed content as a JSON value, for [`parameter_fingerprint`]: every
/// member of `ContextContent` in one fixed order, so two requests fingerprint alike
/// exactly when they carry the same content. Lives here because every operation that
/// takes a `ContextContent` under an `@idempotencyToken` has to fingerprint it the same
/// way.
pub fn context_content_fingerprint(content: &ContextContent) -> ::serde_json::Value {
    ::serde_json::json!({
        "accountIds": &content.account_ids,
        "additionalContext": &content.additional_context,
        "applicationOverview": &content.application_overview,
        "applicationType": &content.application_type,
        "architectureOverview": &content.architecture_overview,
        "awsServices": &content.aws_services,
        "criticality": &content.criticality,
        "industry": &content.industry,
        "regions": &content.regions,
        "resourceTags": content.resource_tags.as_ref().map(|tags| {
            tags.iter()
                .map(|tag| ::serde_json::json!({ "key": &tag.key, "value": &tag.value }))
                .collect::<Vec<::serde_json::Value>>()
        }),
        "resourceTypes": &content.resource_types,
    })
}

/// Encode a paging cursor as an opaque, self-describing `NextToken`.
///
/// Hex of the JSON cursor: it satisfies the modelled `NextToken` pattern
/// `^[A-Za-z0-9+/=_-]+$` and carries the whole cursor, so paging needs no
/// server-side token table — a reporting operation must not write to state.
pub(super) fn encode_cursor(cursor: &::serde_json::Value) -> String {
    hex::encode(cursor.to_string())
}

/// The `NextToken` for the one-member cursor `{key: value}`: the shared mint behind every
/// single-keyed encoder below, so the token a cursor is handed out as and the token a
/// cursor is recognised by ([`decode_keyed_cursor`]) are spelled in exactly one place.
pub(super) fn encode_keyed_cursor(key: &str, value: &str) -> String {
    let mut cursor = ::serde_json::Map::new();
    cursor.insert(
        key.to_string(),
        ::serde_json::Value::String(value.to_string()),
    );
    encode_cursor(&::serde_json::Value::Object(cursor))
}

/// The JSON a `NextToken` carries, or `None` when the token is not hex of a JSON value at
/// all.
///
/// Decoding is only the first half of reading a token: `hex::decode` accepts upper-case
/// digits where [`encode_cursor`] writes lower-case ones, and `::serde_json` accepts every
/// spelling of one value (added whitespace, an escaped member name, a repeated or an extra
/// member, a respelled number), so a value that decodes here is NOT yet a cursor this
/// service handed out. Each `decode_*` below finishes the read by minting the token again
/// from the cursor it understood and keeping it only when that token is the one it was
/// given, byte for byte.
pub(super) fn decode_cursor(token: &str) -> Option<::serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    ::serde_json::from_slice(&bytes).ok()
}

/// The string a one-member cursor keyed `key` carries, or `None` when `token` is not the
/// token this service issues for exactly that cursor.
///
/// A `NextToken` is this service's OWN opaque value, minted by [`encode_keyed_cursor`] and
/// meaningful to nothing else, so the only token that carries a cursor is that token byte
/// for byte: a caller holding one can do nothing with it but send it back as it was handed
/// out. Every other string that happens to decode to the same cursor — a differently
/// spelled hex or JSON, or a cursor carrying a member the encoder does not write — is a
/// value no caller was ever handed, so it is not a resume point but a bad request.
///
/// The shared read behind every single-keyed `decode_*` below, so none of them repeats
/// either half of it.
pub(super) fn decode_keyed_cursor(token: &str, key: &str) -> Option<String> {
    let value = decode_cursor(token)?.get(key)?.as_str()?.to_string();
    (encode_keyed_cursor(key, &value) == token).then_some(value)
}

/// Encode a `ListWorkloads` cursor: the `WorkloadId` last served, in the same opaque
/// scheme as every other cursor in this service. Workloads are enumerated in `WorkloadId`
/// order ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::workload_order`]),
/// so that id is the whole cursor: it is unique and fixed for the workload's whole life,
/// where `WorkloadName` is a member `UpdateWorkload` can move. Its own key, so a cursor
/// issued by another paginated operation is not a cursor here.
pub fn encode_next_token(workload_id: &str) -> String {
    encode_keyed_cursor("i", workload_id)
}

/// The `WorkloadId` a `NextToken` points just past, or `None` when the token is not one
/// this service issued — the check [`decode_keyed_cursor`] states.
pub fn decode_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "i")
}

/// Encode a `ListWorkloadShares` cursor: the last `ShareId` served, in the same opaque
/// scheme as [`encode_next_token`]. Shares are enumerated in `ShareId` order, so that id
/// is the whole cursor.
pub fn encode_share_next_token(share_id: &str) -> String {
    encode_keyed_cursor("s", share_id)
}

/// The `ShareId` a `ListWorkloadShares` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_share_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "s")
}

/// Encode a `ListShareInvitations` cursor: the last `ShareInvitationId` served, in the
/// same opaque scheme as [`encode_next_token`]. Invitations are enumerated in
/// `ShareInvitationId` order, so that id is the whole cursor. Its own key, so a cursor
/// issued by another paginated operation is not a cursor here.
pub fn encode_share_invitation_next_token(share_invitation_id: &str) -> String {
    encode_keyed_cursor("v", share_invitation_id)
}

/// The `ShareInvitationId` a `ListShareInvitations` `NextToken` points just past, or
/// `None` when the token is not one this service issued.
pub fn decode_share_invitation_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "v")
}

/// Encode a `ListMilestones` cursor: the `MilestoneNumber` of the last milestone the
/// page returned. A milestone keeps its number for life and numbers are never reused,
/// so the number alone resumes the enumeration.
pub fn encode_milestone_next_token(milestone_number: i32) -> String {
    encode_cursor(&::serde_json::json!({ "m": milestone_number }))
}

/// The `MilestoneNumber` a `ListMilestones` `NextToken` points just past, or `None`
/// when the token is not one this service issued — the token minted again from the number
/// read back, for the reason [`decode_keyed_cursor`] states.
pub fn decode_milestone_next_token(token: &str) -> Option<i32> {
    let cursor = decode_cursor(token)?;
    let milestone_number = i32::try_from(cursor.get("m")?.as_i64()?).ok()?;
    (encode_milestone_next_token(milestone_number) == token).then_some(milestone_number)
}

/// Encode a `ListReviewTemplates` cursor: the `TemplateArn` last served, in the same
/// opaque scheme as [`encode_next_token`]. Templates are enumerated in `TemplateArn`
/// order ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::review_template_order`]),
/// so that ARN is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_review_template_next_token(template_arn: &str) -> String {
    encode_keyed_cursor("t", template_arn)
}

/// The `TemplateArn` a `ListReviewTemplates` `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_review_template_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "t")
}

/// Encode a `ListProfiles` cursor: the `ProfileArn` last served, in the same opaque
/// scheme as [`encode_next_token`]. Profiles are enumerated in `ProfileArn` order
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::profile_order`]),
/// so that ARN is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_profile_next_token(profile_arn: &str) -> String {
    encode_keyed_cursor("o", profile_arn)
}

/// The `ProfileArn` a `ListProfiles` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_profile_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "o")
}

/// Encode a `ListProfileNotifications` cursor: the `(WorkloadId, ProfileArn)` of the last
/// notification the page served, in the same opaque scheme as [`encode_next_token`].
///
/// A notification is ABOUT one association of one profile with one workload, so the pair
/// that names that association is the whole cursor; both halves are fixed for the life of
/// the resources they name, where `WorkloadName` is a member `UpdateWorkload` can move and
/// `ProfileName` one `UpdateProfile` can. Its own keys, so a cursor issued by another
/// paginated operation is not a cursor here.
pub fn encode_profile_notification_next_token(workload_id: &str, profile_arn: &str) -> String {
    encode_cursor(&::serde_json::json!({ "w": workload_id, "f": profile_arn }))
}

/// The `(WorkloadId, ProfileArn)` a `ListProfileNotifications` `NextToken` points just
/// past, or `None` when the token is not one this service issued — the token minted again
/// from the pair read back, for the reason [`decode_keyed_cursor`] states.
pub fn decode_profile_notification_next_token(token: &str) -> Option<(String, String)> {
    let cursor = decode_cursor(token)?;
    let workload_id = cursor.get("w")?.as_str()?.to_string();
    let profile_arn = cursor.get("f")?.as_str()?.to_string();
    (encode_profile_notification_next_token(&workload_id, &profile_arn) == token)
        .then_some((workload_id, profile_arn))
}

/// Encode a `ListNotifications` cursor: the `(resource key, lens alias)` of the last lens
/// notification the page served, in the same opaque scheme as [`encode_next_token`].
///
/// A lens notification is ABOUT one lens review of one resource, so the pair that names that
/// review is the whole cursor: the key of the resource holding it — a workload's `WorkloadId`
/// or a review template's `TemplateArn`, each fixed for that resource's life and the key its
/// map is held under — and the [`lens_alias`] the review is one review under. Its own keys,
/// so a cursor issued by another paginated operation is not a cursor here.
pub fn encode_lens_notification_next_token(resource_key: &str, alias: &str) -> String {
    encode_cursor(&::serde_json::json!({ "nr": resource_key, "nl": alias }))
}

/// The `(resource key, lens alias)` a `ListNotifications` `NextToken` points just past, or
/// `None` when the token is not one this service issued — the token minted again from the
/// pair read back, for the reason [`decode_keyed_cursor`] states.
pub fn decode_lens_notification_next_token(token: &str) -> Option<(String, String)> {
    let cursor = decode_cursor(token)?;
    let resource_key = cursor.get("nr")?.as_str()?.to_string();
    let alias = cursor.get("nl")?.as_str()?.to_string();
    (encode_lens_notification_next_token(&resource_key, &alias) == token)
        .then_some((resource_key, alias))
}

/// Encode a `ListAgentContexts` cursor: the context `UUID` last served, in the same
/// opaque scheme as [`encode_next_token`]. Contexts are enumerated in id order, so that
/// id is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_agent_context_next_token(context_id: &str) -> String {
    encode_keyed_cursor("x", context_id)
}

/// The context `UUID` a `ListAgentContexts` `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_agent_context_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "x")
}

/// Encode a `ListAgentRecommendationGenerations` cursor: the generation `UUID` last
/// served, in the same opaque scheme as [`encode_next_token`]. Generations are
/// enumerated in id order, so that id is the whole cursor. Its own key, so a cursor
/// issued by another paginated operation is not a cursor here.
pub fn encode_agent_generation_next_token(generation_id: &str) -> String {
    encode_keyed_cursor("j", generation_id)
}

/// The generation `UUID` a `ListAgentRecommendationGenerations` `NextToken` points just
/// past, or `None` when the token is not one this service issued.
pub fn decode_agent_generation_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "j")
}

/// Encode a `ListAgentGoals` cursor: the goal `UUID` last served, in the same opaque
/// scheme as [`encode_next_token`]. Goals are enumerated in id order, so that id is the
/// whole cursor. Its own key, so a cursor issued by another paginated operation is not a
/// cursor here.
pub fn encode_agent_goal_next_token(goal_id: &str) -> String {
    encode_keyed_cursor("g", goal_id)
}

/// The goal `UUID` a `ListAgentGoals` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_agent_goal_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "g")
}

/// Encode a `ListAgentProfiles` cursor: the `AgentProfileArn` last served, in the same
/// opaque scheme as [`encode_next_token`]. Profiles are enumerated in ARN order and an
/// ARN is never reused ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::mint_agent_profile_arn`]),
/// so that ARN is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_agent_profile_next_token(profile_arn: &str) -> String {
    encode_keyed_cursor("p", profile_arn)
}

/// The `AgentProfileArn` a `ListAgentProfiles` `NextToken` points just past, or `None`
/// when the token is not one this service issued.
pub fn decode_agent_profile_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "p")
}

/// Encode a `ListAgentRecommendations` cursor: the `RecommendationArn` last served, in
/// the same opaque scheme as [`encode_next_token`]. Recommendations are enumerated in ARN
/// order and an ARN is never reused
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::mint_agent_recommendation_arn`]),
/// so that ARN is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_agent_recommendation_next_token(recommendation_arn: &str) -> String {
    encode_keyed_cursor("a", recommendation_arn)
}

/// The `RecommendationArn` a `ListAgentRecommendations` `NextToken` points just past, or
/// `None` when the token is not one this service issued.
pub fn decode_agent_recommendation_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "a")
}

/// Encode a `ListAgentRecommendationItems` cursor: the item id last served, in the same
/// opaque scheme as [`encode_next_token`]. The items of one recommendation are enumerated
/// in id order and an id names one item of that recommendation for its whole life, so that
/// id is the whole cursor. Its own key, so a cursor issued by another paginated operation
/// is not a cursor here.
pub fn encode_agent_recommendation_item_next_token(item_id: &str) -> String {
    encode_keyed_cursor("ri", item_id)
}

/// The item id a `ListAgentRecommendationItems` `NextToken` points just past, or `None`
/// when the token is not one this service issued.
pub fn decode_agent_recommendation_item_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "ri")
}

/// What a `NextToken` this service did not issue is answered with. The token is this
/// service's own opaque cursor, so the only thing a caller can do is start the
/// pagination again — which is what `CANNOT_PARSE` tells it. Shared by every paginated
/// operation, so none of them words that refusal differently.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The next token that was passed in is invalid.";

/// The member an undecodable-token finding names (`<Operation>Input$NextToken`).
pub const NEXT_TOKEN_MEMBER: &str = "NextToken";

/// The member a share refusal about the principal offered the share names
/// (`Create<Resource>ShareInput$SharedWith`).
pub const SHARED_WITH_MEMBER: &str = "SharedWith";

/// The sentence a share offered to the principal that is already doing the sharing is
/// refused with.
///
/// A share GRANTS a principal access it does not have — "The owner of a workload can
/// share it with other Amazon Web Services accounts and users"
/// (`smithy_query(query="operation", operation="CreateWorkloadShare")`) — so the refusal
/// is about the RELATIONSHIP between the two principals of the request, not about the
/// value of either one: it states that relationship and names no account id, so a
/// refusal discloses no identity the caller did not send. Shared by every `Create*Share`
/// operation, so none of them words it differently.
pub const SELF_SHARE_MESSAGE: &str =
    "The shared with principal cannot be the same as the sharer's principal.";

/// The modelled `ValidationException` (HTTP 400) for a share offered to the principal
/// doing the sharing: [`SELF_SHARE_MESSAGE`], reported as a finding on the member that
/// named the principal. Unlike [`member_constraint_exception`] it is NOT worded as a
/// per-field constraint — the rule is the relationship between two principals rather
/// than a bound on one member's value — so the sentence stands on its own.
pub fn self_share_exception() -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        SHARED_WITH_MEMBER.to_string(),
        SELF_SHARE_MESSAGE.to_string(),
    )])
}

/// Encode a `ListLenses` cursor: the [`lens_catalog_key`] of the last lens the page
/// served, in the same opaque scheme as [`encode_next_token`]. Its own key, so a cursor
/// issued by another paginated operation is not a cursor here.
pub fn encode_lens_next_token(key: &str) -> String {
    encode_keyed_cursor("c", key)
}

/// The [`lens_catalog_key`] a `ListLenses` `NextToken` points just past, or `None` when
/// the token is not one this service issued — the check [`decode_keyed_cursor`] states.
pub fn decode_lens_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "c")
}

/// Encode a `ListLensReviews` cursor: the [`lens_alias`] of the last lens review the page
/// served, in the same opaque scheme as [`encode_next_token`]. A workload's lens reviews
/// are enumerated in lens-alias order and a lens is attached once however it was named,
/// so that alias is the whole cursor. Its own key, so a cursor issued by another
/// paginated operation is not a cursor here.
pub fn encode_lens_review_next_token(alias: &str) -> String {
    encode_keyed_cursor("r", alias)
}

/// The [`lens_alias`] a `ListLensReviews` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_lens_review_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "r")
}

/// Encode a cursor over a lens's questions: the `QuestionId` last served, in the same
/// opaque scheme as [`encode_next_token`].
pub fn encode_question_token(question_id: &str) -> String {
    encode_keyed_cursor("q", question_id)
}

/// The `QuestionId` a questions `NextToken` points just past, or `None` when the token
/// is not one this service issued.
pub fn decode_question_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "q")
}

/// Encode a cursor over the Trusted Advisor checks of one best practice: the
/// `(account id, check id)` pair last served, in the same opaque scheme as
/// [`encode_next_token`]. A check is reported once per account the workload covers
/// ([`WorkloadRecord::check_account_ids`]), so it takes both to name a position. Its own
/// keys, so a cursor issued by another paginated operation is not a cursor here.
pub fn encode_check_next_token(account_id: &str, check_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "ca": account_id, "ck": check_id }))
}

/// The `(account id, check id)` a checks `NextToken` points just past, or `None` when the
/// token is not one this service issued — the token minted again from the pair read back,
/// for the reason [`decode_keyed_cursor`] states.
pub fn decode_check_next_token(token: &str) -> Option<(String, String)> {
    let cursor = decode_cursor(token)?;
    let account_id = cursor.get("ca")?.as_str()?.to_string();
    let check_id = cursor.get("ck")?.as_str()?.to_string();
    (encode_check_next_token(&account_id, &check_id) == token).then_some((account_id, check_id))
}

/// Encode a cursor over the Trusted Advisor checks of one best practice SUMMARIZED for
/// every account at once: the `CheckId` last served, in the same opaque scheme as
/// [`encode_next_token`]. A summary covers all the accounts the workload relates to in one
/// entry, so the check alone names a position. Its own key, so a cursor issued by another
/// paginated operation — including the per-account [`encode_check_next_token`] — is not a
/// cursor here.
pub fn encode_check_summary_next_token(check_id: &str) -> String {
    encode_keyed_cursor("cs", check_id)
}

/// The `CheckId` a check-summaries `NextToken` points just past, or `None` when the token
/// is not one this service issued.
pub fn decode_check_summary_next_token(token: &str) -> Option<String> {
    decode_keyed_cursor(token, "cs")
}

/// One page of a lens's questions: the questions to report, and the `NextToken` the next
/// page resumes from when the selection does not fit in this one.
#[derive(Debug)]
pub struct QuestionPage<'a> {
    /// The questions this page reports, in the order of the question set it was paged
    /// over — the lens's own order, except where the caller ordered that set first
    /// ([`order_by_pillar_priority`]).
    pub questions: Vec<&'a ReviewQuestion>,
    /// The cursor a caller follows for the rest of the selection, `None` on the last
    /// page.
    pub next_token: Option<String>,
}

/// Page over a lens's question set: the questions `select` keeps, resumed just past the
/// question `resume_after` names and cut to `page_size`.
///
/// Shared by every operation that pages over the answers of a lens review (a workload's
/// or a review template's), so they cannot drift about the order, the cursor or where a
/// page ends. The cursor is start-EXCLUSIVE and names a `QuestionId` by its position in
/// `catalog`, so `select` can differ between pages without a question being served twice
/// or skipped — and a caller that serves the set in an order of its own
/// ([`order_by_pillar_priority`]) pages over it in that order by handing it in that order.
///
/// `None` when `resume_after` names a question the lens does not ask: that is not a
/// cursor this review issued, so it is as undecodable as a malformed token and the
/// caller answers it with its own modelled refusal.
pub fn question_page<'a, Select: Fn(&ReviewQuestion) -> bool>(
    catalog: &'a [ReviewQuestion],
    select: Select,
    resume_after: Option<&str>,
    page_size: usize,
) -> Option<QuestionPage<'a>> {
    let selected: Vec<(usize, &ReviewQuestion)> = catalog
        .iter()
        .enumerate()
        .filter(|(_, question)| select(question))
        .collect();
    let start = match resume_after {
        None => 0,
        Some(question_id) => {
            let position = catalog
                .iter()
                .position(|question| question.question_id == question_id)?;
            selected.partition_point(|(index, _)| *index <= position)
        }
    };
    let remaining = &selected[start.min(selected.len())..];
    let page = &remaining[..remaining.len().min(page_size)];
    let next_token = match page.last() {
        Some((_, question)) if remaining.len() > page.len() => {
            Some(encode_question_token(&question.question_id))
        }
        _ => None,
    };
    Some(QuestionPage {
        questions: page.iter().map(|(_, question)| *question).collect(),
        next_token,
    })
}

/// Re-order a lens review's questions into the order the IMPROVEMENT PLAN of a workload
/// reports them: the questions of the pillar the workload prioritises first, then those of
/// the next, and inside one pillar the lens's own question order.
///
/// `WorkloadPillarPriorities` is "the priorities of the pillars, which are used to order
/// items in the improvement plan. Each pillar is represented by its
/// PillarReviewSummary$PillarId" (`smithy_query(query="doc",
/// shape="WorkloadPillarPriorities")`), so the plan's order is the workload's own, not the
/// lens's: two workloads that differ only in those priorities serve the same items in
/// different orders. A pillar the priority list does not name is not prioritised at all, so
/// its questions follow every prioritised pillar's, still in the lens's order — which is
/// also the whole order for a priority list that names nothing, though no write path leaves
/// a workload carrying one ([`resolved_pillar_priorities`]). The sort is STABLE, so the
/// lens's order is what breaks every tie.
///
/// The order is applied to the question set BEFORE it is selected over and paged
/// ([`question_page`]), because the plan's cursor names a position in the order the plan
/// is served in.
///
/// `WorkloadPillarPriorities` carries no modelled list bound
/// (`smithy_query(query="shape", shape="WorkloadPillarPriorities")`), so a stored priority
/// list is as long as the request that wrote it. Each pillar's rank is therefore resolved
/// ONCE into an index and each question's key read ONCE from it, so this costs the question
/// count and the priority length ADDED rather than multiplied: no request can make the work
/// of ordering the plan grow with the product of the two.
pub fn order_by_pillar_priority(questions: &mut [ReviewQuestion], pillar_priorities: &[String]) {
    // The first position a pillar is named at is its rank, so a list naming one twice ranks
    // it where it was first named.
    let mut rank: BTreeMap<&str, usize> = BTreeMap::new();
    for (position, pillar_id) in pillar_priorities.iter().enumerate() {
        rank.entry(pillar_id.as_str()).or_insert(position);
    }
    // Every rank is below the list's length, so that length is the rank of a pillar the
    // list does not name: behind every pillar it does.
    let unprioritized = pillar_priorities.len();
    questions.sort_by_cached_key(|question| {
        rank.get(question.pillar_id.as_str())
            .copied()
            .unwrap_or(unprioritized)
    });
}

/// The sentence this service states for a parameter value it could not accept as the type
/// the parameter is read as: the subject that was passed in, and nothing about the value.
///
/// `subject` is what the caller has to fix as the service names it — the enumerated type a
/// query parameter is parsed into, say. [`INVALID_NEXT_TOKEN_MESSAGE`] is the same sentence
/// over the opaque cursor's own subject.
pub fn invalid_parameter_sentence(subject: &str) -> String {
    format!("The {subject} that was passed in is invalid.")
}

/// `ValidationExceptionReason$FIELD_VALIDATION_FAILED` — the modelled classification for
/// a request whose members were read but whose values the service refused. See
/// `VALIDATION_EXCEPTION_REASON_VALUES` for the enum's whole value set.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `ValidationExceptionReason$CANNOT_PARSE` — the modelled classification for a value
/// the service could not decode at all (an opaque pagination token, say), as opposed to
/// one that was decoded and then failed a field rule.
pub const REASON_CANNOT_PARSE: &str = "CANNOT_PARSE";

/// The modelled `ValidationException` (HTTP 400) for members whose values the service
/// refused: `Reason` is `FIELD_VALIDATION_FAILED`, `Fields` carries one entry per
/// offending member, and the required `Message` states each distinct field message once
/// (a cross-member rule names every member it covers with the same sentence, and
/// `Message` must not repeat it per member).
///
/// `Message` carries this service's `[Validation]` tag, the same form
/// [`tagged_message`] gives every other exception class; a `Fields` entry does NOT —
/// the tag names the exception's class, and a field finding is a clause inside it.
///
/// Every operation builds this refusal class here, so the envelope cannot drift between
/// operations; each operation still wraps the result in its OWN modelled error enum.
pub fn field_validation_exception(
    fields: &[(String, String)],
) -> crate::ops::types::ValidationException {
    // The distinct messages in the order they first appear, with membership tested against
    // a set rather than by scanning the messages already kept: `fields` carries one entry
    // per offending value of a request list, so a scan per entry would make building the
    // refusal cost work quadratic in a length the caller chose.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut distinct: Vec<&str> = Vec::new();
    for (_, message) in fields {
        if seen.insert(message.as_str()) {
            distinct.push(message);
        }
    }
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, &distinct.join(" ")),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(
            fields
                .iter()
                .map(|(name, message)| crate::ops::types::ValidationExceptionField {
                    name: name.clone(),
                    message: message.clone(),
                })
                .collect(),
        ),
    }
}

/// The modelled `ValidationException` for a value the service could not decode:
/// `Reason` `CANNOT_PARSE`, which tells the caller the value cannot be corrected, only
/// obtained again, plus one `Fields` entry naming the member whose value could not be
/// decoded.
pub fn cannot_parse_exception(
    member: &str,
    message: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, message),
        reason: Some(REASON_CANNOT_PARSE.to_string()),
        fields: Some(vec![crate::ops::types::ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
    }
}

/// The modelled `ValidationException` for a request the service refused without
/// classifying it: only the `@required` `Message`. `ValidationExceptionReason` has no
/// value for "a member the request had to carry is absent" (`UNKNOWN_OPERATION`,
/// `CANNOT_PARSE`, `FIELD_VALIDATION_FAILED`, `OTHER`), and the recorded service leaves
/// `Reason` and `Fields` off that answer rather than inventing one.
pub fn unclassified_exception(message: String) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, &message),
        reason: None,
        fields: None,
    }
}

/// The modelled `ValidationException` for a refusal this service CARRIES `Reason` on
/// without classifying it: the member is emitted holding no value, which tells a client
/// reading it that the failure is none of the modelled classifications — as opposed to
/// leaving the member out, which tells it nothing. No `Fields`, because no per-field rule
/// produced the finding.
///
/// The one place that empty classification is written, so every refusal of the class words
/// its envelope identically.
pub fn unstated_reason_exception(message: String) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, &message),
        reason: Some(String::new()),
        fields: None,
    }
}

/// The refusal for a request that does not carry a property the shape it is read as
/// requires: the finding the layer that READS the request reports, which names the
/// property as the schema declares it and no member path.
///
/// That layer is what fails — the request never became the shape, so no field rule ran —
/// so the refusal is [`unstated_reason_exception`]'s envelope over that finding.
pub fn missing_property_exception(property: &str) -> crate::ops::types::ValidationException {
    unstated_reason_exception(service_common::validation_message::bracketed_findings(&[
        service_common::validation_message::missing_required_properties(&[property]),
    ]))
}

/// The refusal for a member whose value is a DOCUMENT the service could not read at all:
/// the member named as invalid, and nothing else.
///
/// No parser diagnostic reaches the message: where a decoder stopped is an artefact of the
/// decoder rather than an API contract, and a caller cannot act on it — the value has to be
/// replaced whole. Classified as [`missing_property_exception`] is, for the same reason:
/// the request never became what it had to be.
pub fn unparsable_document_exception(member: &str) -> crate::ops::types::ValidationException {
    unstated_reason_exception(format!("Invalid {member}"))
}

/// The failure class every `ValidationException` message this service words opens with
/// (see [`tagged_message`]).
pub(super) const VALIDATION_TAG: &str = "Validation";

/// The sentence a member-constraint failure states, in the form the framework's own
/// constraint walk uses so a handler-enforced rule reads like a modelled one.
pub fn constraint_sentence(member: &str, constraint: &str) -> String {
    format!("Value at {member} failed to satisfy constraint: {constraint}")
}

/// The refusal for one member that failed one constraint: the sentence above, reported
/// as a field validation naming that member.
pub fn member_constraint_exception(
    member: &str,
    constraint: &str,
) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        member.to_string(),
        constraint_sentence(member, constraint),
    )])
}

/// The member a refused `SelectedChoices` value is reported under, in either operation
/// that records an answer (`UpdateAnswerInput$SelectedChoices`,
/// `UpdateReviewTemplateAnswerInput$SelectedChoices`).
pub const SELECTED_CHOICES_MEMBER: &str = "SelectedChoices";

/// The member a refused `ChoiceUpdates` key is reported under, in either operation that
/// records an answer. A finding about ONE entry names the path that reaches it
/// (`ChoiceUpdates.<ChoiceId>.<Member>`), so a caller is told WHICH choice's update to fix.
pub const CHOICE_UPDATES_MEMBER: &str = "ChoiceUpdates";

/// The member a refused answer `Reason` is reported under, in either operation that
/// records an answer.
pub const ANSWER_REASON_MEMBER: &str = "Reason";

/// The members a request names ONE best practice of a lens review by: the pillar, the
/// question that pillar asks and the choice that question offers
/// (`ListCheckDetailsInput$PillarId` / `$QuestionId` / `$ChoiceId`, each `@required`). A
/// finding about one of them names it, so a caller is told WHICH part of the path to fix.
pub const PILLAR_ID_MEMBER: &str = "PillarId";
/// See [`PILLAR_ID_MEMBER`].
pub const QUESTION_ID_MEMBER: &str = "QuestionId";
/// See [`PILLAR_ID_MEMBER`].
pub const CHOICE_ID_MEMBER: &str = "ChoiceId";

/// The refusal for the first enum member of an answer-recording request carrying a value
/// outside its modelled value set, if any does: the answer's own `Reason` (an
/// `AnswerReason`) and each `ChoiceUpdate`'s `Status` (a `ChoiceStatus`) and `Reason` (a
/// `ChoiceReason`).
///
/// The boundary's constraint walk does not reach inside a map's values, so these are
/// enforced by the handler — and both operations that record an answer (a workload's and
/// a review template's) take the same two members, so the rule and its wording live here
/// once rather than in each of them.
///
/// The `ChoiceUpdates` entries are judged in `ChoiceId` order, so the member a refusal
/// names does not depend on the request map's iteration order.
pub fn answer_enum_refusal(
    reason: Option<&str>,
    choice_updates: Option<&HashMap<String, crate::ops::types::ChoiceUpdate>>,
) -> Option<crate::ops::types::ValidationException> {
    let mut members: Vec<(String, &str, &[&str])> = Vec::new();
    if let Some(reason) = reason {
        members.push((
            ANSWER_REASON_MEMBER.to_string(),
            reason,
            crate::ops::types::ANSWER_REASON_VALUES,
        ));
    }
    let mut updates: Vec<(&String, &crate::ops::types::ChoiceUpdate)> =
        choice_updates.into_iter().flatten().collect();
    updates.sort_by_key(|(choice_id, _)| *choice_id);
    for (choice_id, update) in updates {
        members.push((
            format!("{CHOICE_UPDATES_MEMBER}.{choice_id}.Status"),
            update.status.as_str(),
            crate::ops::types::CHOICE_STATUS_VALUES,
        ));
        if let Some(reason) = update.reason.as_deref() {
            members.push((
                format!("{CHOICE_UPDATES_MEMBER}.{choice_id}.Reason"),
                reason,
                crate::ops::types::CHOICE_REASON_VALUES,
            ));
        }
    }
    members
        .into_iter()
        .find(|(_, value, allowed)| !allowed.contains(value))
        .map(|(member, _, allowed)| {
            member_constraint_exception(
                &member,
                &format!(
                    "Member must satisfy enum value set: [{}]",
                    allowed.join(", ")
                ),
            )
        })
}

/// The refusal for a `ChoiceId` an answer-recording request names that is not a choice of
/// the question being answered, if either member that carries one does.
///
/// A choice is answered BY ID and the ids are the question's own catalog (`Answer$Choices`
/// / `ReviewTemplateAnswer$Choices`, the only place a client discovers them), so an id
/// naming a choice of another question — or of none — belongs to no answer of this
/// question. Shared by both operations that record an answer, so they cannot word it
/// differently; the offending ids are sorted and deduplicated, so the refusal does not
/// depend on the order the request listed them in.
pub fn undefined_choice_refusal(
    question: &ReviewQuestion,
    selected_choices: Option<&[String]>,
    choice_updates: Option<&HashMap<String, crate::ops::types::ChoiceUpdate>>,
) -> Option<crate::ops::types::ValidationException> {
    // Each member's ids are BORROWED from the request: only the ones the question does
    // not define are collected, so a request whose choices are all valid allocates
    // nothing.
    let named: [(&str, Box<dyn Iterator<Item = &str>>); 2] = [
        (
            SELECTED_CHOICES_MEMBER,
            Box::new(selected_choices.unwrap_or_default().iter().map(String::as_str)),
        ),
        (
            CHOICE_UPDATES_MEMBER,
            Box::new(
                choice_updates
                    .into_iter()
                    .flat_map(|updates| updates.keys().map(String::as_str)),
            ),
        ),
    ];
    for (member, choice_ids) in named {
        if let Some(exception) = undefined_choice_member_refusal(member, question, choice_ids) {
            return Some(exception);
        }
    }
    None
}

/// The refusal for the `ChoiceId`s ONE member of a request names that are not choices of
/// the question the request is about, if any of them is not.
///
/// The rule [`undefined_choice_refusal`] applies to each member that carries a choice of an
/// ANSWER, stated once for any member that names a choice: a choice belongs to the question
/// that offers it, so an id naming a choice of another question — or of none — belongs to
/// no choice of this question. `member` is the member the finding names, and the offending
/// ids are sorted and deduplicated, so the refusal does not depend on the order the request
/// listed them in.
pub fn undefined_choice_member_refusal<'id>(
    member: &str,
    question: &ReviewQuestion,
    choice_ids: impl Iterator<Item = &'id str>,
) -> Option<crate::ops::types::ValidationException> {
    let mut undefined: Vec<&str> = choice_ids
        .filter(|choice_id| !question.defines_choice(choice_id))
        .collect();
    if undefined.is_empty() {
        return None;
    }
    undefined.sort_unstable();
    undefined.dedup();
    Some(member_constraint_exception(
        member,
        &format!(
            "every choice must be a choice of question {}, which {} is not",
            question.question_id,
            undefined.join(", ")
        ),
    ))
}

/// Whether an answer-recording request asks for a change to the answer at all.
///
/// Every member of such a request that writes the answer is optional, so one that supplies
/// NONE of them asks for nothing: recording an answer is what takes a question out of
/// `UNANSWERED` ([`WorkloadRecord::risk_counts`],
/// [`ReviewTemplateRecord::question_counts`]), so an answer comes into existence only for a
/// request that answers, and nothing — not even the reviewed resource's `UpdatedAt` — is
/// written for one that does not. A member the request DOES carry is a write however empty
/// the value it carries, because the member is the answer's new value.
///
/// Shared by both operations that record an answer (a workload's and a review template's),
/// which take the same five members, so the rule lives here once rather than in each of
/// them.
pub fn answer_write_requested(
    selected_choices: Option<&[String]>,
    choice_updates: Option<&HashMap<String, crate::ops::types::ChoiceUpdate>>,
    is_applicable: Option<bool>,
    reason: Option<&str>,
    notes: Option<&str>,
) -> bool {
    selected_choices.is_some()
        || choice_updates.is_some()
        || is_applicable.is_some()
        || reason.is_some()
        || notes.is_some()
}

/// The member a refused `PillarNotes` key is reported under, in either operation that
/// records the notes of a lens review (`UpdateLensReviewInput$PillarNotes`,
/// `UpdateReviewTemplateLensReviewInput$PillarNotes`).
pub const PILLAR_NOTES_MEMBER: &str = "PillarNotes";

/// The member a refused Jira question selection is reported under
/// (`UpdateLensReviewInput$JiraConfiguration`). A finding about ONE selected pillar names
/// the path that reaches it (`JiraConfiguration.SelectedPillars.<PillarId>`), so a caller
/// is told WHICH selection to fix.
pub const JIRA_CONFIGURATION_MEMBER: &str = "JiraConfiguration";

/// The refusal for a `PillarId` a request supplies that names no pillar of the lens under
/// review, if any of them does.
///
/// A pillar of a lens is what that lens's content names, so an identifier naming a pillar
/// the lens does NOT define belongs to no pillar of that lens review and is refused rather
/// than stored. Only a lens this emulator holds content for has a pillar set to judge an
/// identifier against (`content`, resolved through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_content_at`]); for
/// a lens it holds none for there is nothing to judge, so the value is taken as given
/// rather than refused for a pillar the real lens may well define.
///
/// `member` is the member the finding names, `lens` the lens as the request named it, and
/// the offending ids are sorted and deduplicated, so the refusal does not depend on the
/// order the request listed them in. Shared by every operation that names pillars of a lens
/// review in a member of ITS OWN — the pillars a workload prioritises, the pillars a Jira
/// configuration selects, the pillar of the best practice a check read addresses — so those
/// cannot word it differently: each reports WHICH ids it refused, because that is what tells
/// the caller which entry of its request to fix.
///
/// The member that merely SCOPES a read of one lens review to one pillar is worded by
/// [`crate::service::lens_content_validation::undefined_pillar`] instead: there the request
/// names a single pillar and there is no offending entry to point at, so the finding hands
/// back the pillars the lens defines, in the envelope this service's recorded refusal of
/// that class uses.
pub fn undefined_pillar_refusal<'id>(
    member: &str,
    lens: &str,
    content: &LensContentView,
    pillar_ids: impl Iterator<Item = &'id str>,
) -> Option<crate::ops::types::ValidationException> {
    let defined = content.pillar_ids();
    if defined.is_empty() {
        return None;
    }
    let mut undefined: Vec<&str> = pillar_ids
        .filter(|pillar_id| !defined.contains(pillar_id))
        .collect();
    if undefined.is_empty() {
        return None;
    }
    undefined.sort_unstable();
    undefined.dedup();
    Some(member_constraint_exception(
        member,
        &format!(
            "every pillar must be a pillar of lens {lens}, which {} is not",
            undefined.join(", ")
        ),
    ))
}

/// The refusal for a `QuestionId` a request supplies that names no question ONE pillar of
/// the lens under review asks, if any of them does.
///
/// The questions of a pillar are what that lens's content names, so an identifier naming a
/// question of another pillar — or of none — belongs to no question of that pillar's
/// review. As with [`undefined_pillar_refusal`], a lens this emulator holds no content for
/// asks nothing here, so there is no question set to judge an identifier against and the
/// value is taken as given.
pub fn undefined_pillar_question_refusal<'id>(
    member: &str,
    lens: &str,
    pillar_id: &str,
    content: &LensContentView,
    question_ids: impl Iterator<Item = &'id str>,
) -> Option<crate::ops::types::ValidationException> {
    if content.questions().is_empty() {
        return None;
    }
    let mut undefined: Vec<&str> = question_ids
        .filter(|question_id| !content.asks_in_pillar(pillar_id, question_id))
        .collect();
    if undefined.is_empty() {
        return None;
    }
    undefined.sort_unstable();
    undefined.dedup();
    Some(member_constraint_exception(
        member,
        &format!(
            "every question must be a question pillar {pillar_id} of lens {lens} asks, which {} is not",
            undefined.join(", ")
        ),
    ))
}

/// The tag this service opens an exception message with: the failure class in square
/// brackets, then the sentence — `[ResourceNotFound] …`, `[Conflict] …`,
/// `[Validation] …`. Every failure this service words goes through here, so one form
/// covers them all.
pub(super) fn tagged_message(tag: &str, sentence: &str) -> String {
    format!("[{tag}] {sentence}")
}

/// A modelled `ResourceNotFoundException` (HTTP 404) for a lookup that resolved
/// nothing.
///
/// The message is built from the request and the owning account: it names the resource
/// that was looked for (`subject`, e.g. `workload with ID <id>`) and the account it was
/// looked for in, so a caller can tell "not in this account / not shared with me" from
/// "deleted". `resource_id` is the identifier AS SENT, and `resource_type` the modelled
/// type of the resource that was missing — both are `@required` on the shape.
pub fn resource_not_found(
    resource_type: &str,
    resource_id: String,
    subject: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: tagged_message(
            "ResourceNotFound",
            &format!(
                "No {subject} was found in account {}.",
                ::engine::emulator_account_id()
            ),
        ),
        resource_id,
        resource_type: resource_type.to_string(),
    }
}

/// [`resource_not_found`] for a workload id that names no stored workload. Shared by
/// every operation that resolves a `WorkloadId`, so they cannot word it differently.
pub fn workload_not_found(workload_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        WORKLOAD_RESOURCE_TYPE,
        workload_id.to_string(),
        &format!("workload with ID {workload_id}"),
    )
}

/// [`resource_not_found`] for a share id that names no share of the workload.
///
/// A `ShareId` identifies a share on its own — the id space is unique across every
/// workload's shares — so the `ResourceId` is that id AS SENT, and the message names the
/// workload the share was looked for in, so a caller can tell a share of another
/// workload from a share that was revoked. Shared by every operation that resolves a
/// workload `ShareId`, so they cannot word it differently.
pub fn workload_share_not_found(workload_id: &str, share_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        WORKLOAD_SHARE_RESOURCE_TYPE,
        share_id.to_string(),
        &format!("share with ID {share_id} of a workload with ID {workload_id}"),
    )
}

/// [`resource_not_found`] for a `ProfileArn` that names no profile associated with the
/// workload — none is, or the one the workload holds is a different profile.
///
/// A `ProfileArn` identifies the profile on its own, so — as for a workload `ShareId` —
/// the `ResourceId` is that ARN AS SENT, and the message names the workload the profile
/// was looked for in, so a caller can tell a profile of another workload from one that
/// was already disassociated. Shared by every operation that resolves a profile OF a
/// workload, so they cannot word it differently.
pub fn workload_profile_not_found(
    workload_id: &str,
    profile_arn: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!("profile with ARN {profile_arn} associated with a workload with ID {workload_id}"),
    )
}

/// [`conflict`] for a workload that is already reviewed against a profile: a workload
/// reports at most [`MAX_PROFILES_PER_WORKLOAD`] of them, so the association it already
/// holds is what a request associating a DIFFERENT profile collides with — the modelled
/// `ConflictException`'s own "the resource has already been processed" case. `existing_arn`
/// is the association as STORED, so the caller is told which profile to disassociate, and
/// `resource_id` names the workload the collision is inside, as the other
/// association failures of a workload report it.
pub fn workload_profile_conflict(workload_id: &str, existing_arn: &str) -> ConflictException {
    conflict(
        WORKLOAD_RESOURCE_TYPE,
        workload_id.to_string(),
        &format!(
            "The workload with ID {workload_id} is already associated with the profile with \
             ARN {existing_arn}."
        ),
    )
}

/// The refusal for a request that would leave one workload reviewed against more than
/// [`MAX_PROFILES_PER_WORKLOAD`] profiles: the list the association is reported by is
/// modelled `@length(max: 1)`, so the request asks for a result the API cannot carry,
/// whatever the store holds. `Fields` names the member that carried the ARNs.
pub fn too_many_profiles_exception() -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        PROFILE_ARNS_MEMBER.to_string(),
        format!(
            "A workload is associated with at most {MAX_PROFILES_PER_WORKLOAD} profile, so a \
             request must not name more distinct profiles than that."
        ),
    )])
}

/// The refusal for a request whose `LensAliases` is longer than
/// [`MAX_LENS_ALIASES_PER_REQUEST`]: the member publishes no modelled maximum, so this is
/// the quota that keeps the per-request work of an association constant. `named` is the
/// length as sent, so the caller is told how far past the quota it is.
pub fn too_many_lens_aliases_exception(named: usize) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        LENS_ALIASES_MEMBER.to_string(),
        format!(
            "{LENS_ALIASES_MEMBER} must name at most {MAX_LENS_ALIASES_PER_REQUEST} lenses; \
             {named} were named."
        ),
    )])
}

/// The field a refusal of the lenses a workload may not lose names: `Lenses`, the member
/// the workload carries its lens list in (`Workload$Lenses`) — the collection the request
/// asks to change, as opposed to [`LENS_ALIASES_MEMBER`], the member one request happens to
/// name the lenses in.
pub const LENSES_MEMBER: &str = "Lenses";

/// The refusal for a request that asks for a DEFAULT lens — one a workload is always
/// reviewed against and cannot lose — to be disassociated from a workload.
///
/// `lens_alias` is the alias AS SENT, so the caller reads back the name it used however it
/// spelled the lens. The rule is about the workload's lens collection, so the finding names
/// [`LENSES_MEMBER`].
pub fn default_lens_refusal(lens_alias: &str) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        LENSES_MEMBER.to_string(),
        format!("Cannot disassociate default lens: {lens_alias}."),
    )])
}

/// The refusal for a request that would leave one workload reviewed against more than
/// [`MAX_LENSES_ASSOCIATED_PER_WORKLOAD`] lenses: the service publishes that maximum, so
/// the request asks for a workload the service does not serve. `attached` is what the
/// workload already holds, so the caller is told how much room is left.
pub fn too_many_lenses_associated_exception(
    workload_id: &str,
    attached: usize,
) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        LENS_ALIASES_MEMBER.to_string(),
        format!(
            "A workload is associated with at most {MAX_LENSES_ASSOCIATED_PER_WORKLOAD} lenses, \
             and the workload with ID {workload_id} is already associated with {attached}."
        ),
    )])
}

/// The refusal for a request whose `ProfileArns` is longer than
/// [`MAX_PROFILE_ARNS_PER_REQUEST`]: the member publishes no modelled maximum, so this is
/// the quota that keeps the per-request work of an association constant. `named` is the
/// length as sent, so the caller is told how far past the quota it is.
pub fn too_many_profile_arns_exception(named: usize) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        PROFILE_ARNS_MEMBER.to_string(),
        format!(
            "{PROFILE_ARNS_MEMBER} must name at most {MAX_PROFILE_ARNS_PER_REQUEST} profile \
             ARNs; {named} were named."
        ),
    )])
}

/// [`resource_not_found`] for a template ARN that names no stored review template.
/// Shared by every operation that resolves a `TemplateArn`, so they cannot word it
/// differently.
///
/// The resource noun is the service's own display noun for this resource class —
/// capitalised `Review Template`, as AWS words this failure for a review template.
pub fn review_template_not_found(template_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        REVIEW_TEMPLATE_RESOURCE_TYPE,
        template_arn.to_string(),
        &format!("Review Template with ARN {template_arn}"),
    )
}

/// [`resource_not_found`] for a `ProfileArn` that names no stored Well-Architected Tool
/// profile. Shared by every operation that resolves a `ProfileArn`, so they cannot word
/// it differently.
pub fn profile_not_found(profile_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!("profile with ARN {profile_arn}"),
    )
}

/// The wire error CODE this service's TAGGING surface answers for an ARN that names no
/// resource.
///
/// It is NOT the `ResourceNotFoundException` the rest of this service answers and the
/// model declares for these three operations: the ARN-addressed tag operations are served
/// by the shared tagging frontend in front of this service, and the recorded responses of
/// `ListTagsForResource` / `TagResource` / `UntagResource` against an ARN no resource
/// carries name that frontend's own `NotFoundException`. That shape is in no operation's
/// modelled error set, so the refusal travels through
/// [`crate::scaffolded::Scaffolded::unmodeled`].
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The HTTP status the tagging frontend answers an absent resource with.
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The whole `Message` the tagging frontend's missing-resource refusal carries: the FIXED
/// text of that failure class, with no variable part.
///
/// The frontend addresses every taggable resource type through the one ARN member, so its
/// sentence names no resource type, and it carries nothing the request sent — neither the
/// ARN nor the account the lookup ran in.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The refusal every TAG operation answers for an ARN that resolves to no resource this
/// store holds
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::resource_tags`]):
/// the tagging frontend's [`TAGGING_NOT_FOUND_CODE`] at [`TAGGING_NOT_FOUND_STATUS`],
/// stating [`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`].
///
/// Shared by every operation that resolves a resource by tag ARN, so they cannot answer it
/// differently. The code is not the modelled shape's, which is why this goes out through
/// the unmodelled channel rather than through [`resource_not_found`] — whose sentence, and
/// whose `ResourceId` / `ResourceType` members, belong to the refusal the RESOURCE
/// operations of this service answer.
pub fn tagged_resource_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_NOT_FOUND_CODE,
        TAGGING_NOT_FOUND_STATUS,
        TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
    )
}

/// [`resource_not_found`] for a `ProfileVersion` the profile `profile_arn` names does not
/// have.
///
/// A profile is read AT a version (`GetProfileInput$ProfileVersion`), so a version the
/// profile never had names no readable resource; the `ResourceId` is the profile ARN as
/// sent qualified by the version asked for, so a caller can tell which of the two it got
/// wrong.
pub fn profile_version_not_found(
    profile_arn: &str,
    profile_version: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_RESOURCE_TYPE,
        format!("{profile_arn}.{profile_version}"),
        &format!("version {profile_version} of a profile with ARN {profile_arn}"),
    )
}

/// [`resource_not_found`] for an `AgentProfileArn` that names no stored agent profile.
/// Shared by every operation that resolves one, so they cannot word it differently.
pub fn agent_profile_not_found(profile_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!("agent profile with ARN {profile_arn}"),
    )
}

/// [`conflict`] for a delete of an agent profile whose stored `deletionProtection` is
/// enabled.
///
/// Deletion protection is the profile's own recorded answer to "may this resource be
/// deleted" (`AgentProfileSummary$deletionProtection`, "Indicates whether deletion
/// protection is enabled for the profile"), so a delete of a protected profile is
/// refused by the resource's STATE rather than by anything wrong with the request — a
/// 409 `ConflictException`, not a 400. The sentence names the ARN as sent and the member
/// that has to change first, so a caller can clear the protection and retry.
pub fn agent_profile_deletion_protected(profile_arn: &str) -> ConflictException {
    conflict(
        AGENT_PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!(
            "The agent profile with ARN {profile_arn} cannot be deleted while \
             deletionProtection is enabled."
        ),
    )
}

/// [`resource_not_found`] for a goal id that names no goal of the agent profile
/// `profile_arn`.
///
/// A goal is reached through its profile, so — as for a milestone — the `ResourceId` is
/// the composite of the two and the message names both: a goal id that belongs to
/// ANOTHER profile is as absent here as one that belongs to none.
pub fn agent_goal_not_found(profile_arn: &str, id: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_GOAL_RESOURCE_TYPE,
        agent_goal_resource_id(profile_arn, id),
        &format!("goal with ID {id} in an agent profile with ARN {profile_arn}"),
    )
}

/// How a goal of an agent profile is named in an exception's `ResourceId`: the profile
/// it belongs to and its own id, which together identify it.
pub fn agent_goal_resource_id(profile_arn: &str, id: &str) -> String {
    profile_scoped_resource_id(profile_arn, id)
}

/// How a resource that belongs to an agent profile is named in an exception's
/// `ResourceId`: the profile it hangs off and its own id, which together identify it.
/// Goals, contexts and recommendation generations are all reached through their profile,
/// so the composite is spelled ONCE here and the three builders above call it.
pub(super) fn profile_scoped_resource_id(profile_arn: &str, id: &str) -> String {
    format!("{profile_arn}.{id}")
}

/// [`resource_not_found`] for a context id that names no context of the agent profile
/// `profile_arn`. Worded exactly as [`agent_goal_not_found`], because a context is
/// reached through its profile the same way: a context id that belongs to ANOTHER
/// profile is as absent here as one that belongs to none.
pub fn agent_context_not_found(profile_arn: &str, id: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_CONTEXT_RESOURCE_TYPE,
        agent_context_resource_id(profile_arn, id),
        &format!("context with ID {id} in an agent profile with ARN {profile_arn}"),
    )
}

/// How a context of an agent profile is named in an exception's `ResourceId`: see
/// [`profile_scoped_resource_id`].
pub fn agent_context_resource_id(profile_arn: &str, id: &str) -> String {
    profile_scoped_resource_id(profile_arn, id)
}

/// [`resource_not_found`] for a generation id that names no recommendation generation of
/// the agent profile `profile_arn`. Worded exactly as [`agent_goal_not_found`], because a
/// generation is reached through its profile the same way: an id that belongs to ANOTHER
/// profile is as absent here as one that belongs to none.
pub fn agent_generation_not_found(profile_arn: &str, id: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_GENERATION_RESOURCE_TYPE,
        agent_generation_resource_id(profile_arn, id),
        &format!(
            "recommendation generation with ID {id} in an agent profile with ARN {profile_arn}"
        ),
    )
}

/// How a recommendation generation of an agent profile is named in an exception's
/// `ResourceId`: see [`profile_scoped_resource_id`].
pub fn agent_generation_resource_id(profile_arn: &str, id: &str) -> String {
    profile_scoped_resource_id(profile_arn, id)
}

/// [`resource_not_found`] for a `RecommendationArn` that names no recommendation this
/// store holds.
///
/// A recommendation is named by its ARN ALONE — `UpdateAgentRecommendationStatus` and
/// `GetAgentRecommendation` carry no profile in the path — so the `ResourceId` is that ARN
/// as sent and the message names the recommendation rather than a profile it was looked
/// for in. Shared by every operation that resolves one, so they cannot word it differently.
pub fn agent_recommendation_not_found(recommendation_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_RECOMMENDATION_RESOURCE_TYPE,
        recommendation_arn.to_string(),
        &format!("recommendation with ARN {recommendation_arn}"),
    )
}

/// [`resource_not_found`] for a milestone number that names no recorded milestone of
/// `workload_id`.
///
/// A milestone is identified by the pair, so the `ResourceId` is the composite
/// `<workloadId>.<milestoneNumber>` and the message names both.
pub fn milestone_not_found(workload_id: &str, milestone_number: i32) -> ResourceNotFoundException {
    resource_not_found(
        MILESTONE_RESOURCE_TYPE,
        milestone_resource_id(workload_id, milestone_number),
        &format!(
            "milestone with number {milestone_number} in a workload with ID {workload_id}"
        ),
    )
}

/// How a milestone is named in an exception's `ResourceId`: the workload it belongs to
/// and its number, which together identify it.
pub fn milestone_resource_id(workload_id: &str, milestone_number: i32) -> String {
    format!("{workload_id}.{milestone_number}")
}

/// [`resource_not_found`] for a lens that is not associated with the workload, so the
/// workload has no review of it to report.
///
/// A lens review is identified by the `(WorkloadId, LensAlias)` pair, so — as for a
/// milestone — the `ResourceId` is the composite and the message names both. `lens` is
/// the lens AS SENT, which is how the caller named it.
pub fn lens_review_not_found(workload_id: &str, lens: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_REVIEW_RESOURCE_TYPE,
        lens_review_resource_id(workload_id, lens),
        &format!("review of lens {lens} in a workload with ID {workload_id}"),
    )
}

/// How a lens review is named in an exception's `ResourceId`: the resource it belongs
/// to — a workload's id or a review template's ARN — and the lens it reviews.
pub fn lens_review_resource_id(owner: &str, lens: &str) -> String {
    format!("{owner}.{lens}")
}

/// [`resource_not_found`] for a `QuestionId` the lens under review does not ask, so the
/// review has no answer to that question to read or to record.
///
/// A question of a review is identified by the `(WorkloadId, LensAlias, QuestionId)`
/// triple — the lens decides which questions exist, and the workload decides which lens
/// is reviewed — so the `ResourceId` is the composite and the message names all three.
/// `lens` is the lens AS SENT, which is how the caller named it. Worded like
/// [`review_template_answer_not_found`], the same failure about a review template's lens
/// review.
pub fn question_not_found(
    workload_id: &str,
    lens: &str,
    question_id: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        QUESTION_RESOURCE_TYPE,
        format!("{}.{question_id}", lens_review_resource_id(workload_id, lens)),
        &format!(
            "question with ID {question_id} in a review of lens {lens} in a workload with \
             ID {workload_id}"
        ),
    )
}

/// [`resource_not_found`] for a lens that is not applied to the review template, so the
/// template has no review of it to report.
///
/// Worded like [`lens_review_not_found`] — the same resource type and the same composite
/// `ResourceId` — because it is the same resource: a lens review, here of a template
/// rather than of a workload. `lens` is the lens AS SENT, which is how the caller named
/// it.
pub fn review_template_lens_review_not_found(
    template_arn: &str,
    lens: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        LENS_REVIEW_RESOURCE_TYPE,
        lens_review_resource_id(template_arn, lens),
        &format!("review of lens {lens} in a review template with ARN {template_arn}"),
    )
}

/// `ResourceNotFoundException$ResourceType` for a question of a lens review: the answer
/// to one question is reached through its `QuestionId`, so a `QuestionId` the lens under
/// review does not ask names a missing question rather than a missing lens review. The
/// modelled `ResourceType` is a free-form string (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`), so this is the emulator's own type name for it,
/// worded like the resource types the model does publish.
pub const QUESTION_RESOURCE_TYPE: &str = "QUESTION";

/// [`resource_not_found`] for a `QuestionId` the lens applied to a review template does
/// not ask, so the template's lens review holds no answer to it to read or write.
///
/// An answer is identified by the review template, the lens and the question together, so
/// — as for a lens review — the `ResourceId` is that composite and the message names all
/// three. `lens` and `question_id` are AS SENT, which is how the caller named them. A lens
/// this emulator holds no content for asks nothing, so every `QuestionId` of such a lens
/// resolves here.
pub fn review_template_answer_not_found(
    template_arn: &str,
    lens: &str,
    question_id: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        QUESTION_RESOURCE_TYPE,
        format!("{}.{question_id}", lens_review_resource_id(template_arn, lens)),
        &format!(
            "question with ID {question_id} in the review of lens {lens} \
             in a review template with ARN {template_arn}"
        ),
    )
}

/// [`resource_not_found`] for a `LensAlias` that names no stored custom lens.
///
/// The `ResourceId` is the alias AS SENT, which is how the request named the lens.
pub fn lens_not_found(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("custom lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a `LensAlias` that names no lens at all: neither a lens
/// Amazon Web Services publishes ([`AWS_OFFICIAL_LENS_ALIASES`]) nor a custom lens this
/// account holds. Distinct from [`lens_not_found`], which is about a name that can only be
/// a CUSTOM lens's and resolved to none.
pub fn unknown_lens_not_found(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a lens carrying no content that a version has not been
/// published from, so it has no draft a caller could read or delete: "The new or updated
/// lens will have a status of DRAFT. The lens cannot be applied to workloads or shared
/// with other Amazon Web Services accounts until it's published with CreateLensVersion"
/// (`ImportLens` doc), and a lens whose content IS published is past that state. The
/// `ResourceId` is the alias AS SENT, as the lookup failures above report it.
pub fn lens_draft_not_found(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("draft of the lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a `LensVersion` of a lens that has not been published under
/// that name. A custom lens carries a version only once `CreateLensVersion` publishes it,
/// and an Amazon Web Services official lens carries the versions this emulator ships
/// content for — so the wording speaks about the lens rather than about how it is owned.
pub fn lens_version_not_found(alias: &str, version: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("version {version} of the lens with alias {alias}"),
    )
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for a custom lens that has been
/// imported but not published, as AWS words that refusal: a lens "cannot be applied to
/// workloads or shared with other Amazon Web Services accounts until it's published with
/// CreateLensVersion" (`ImportLens` doc), and the lens-by-id read is one of the things it
/// is not yet reachable through.
///
/// The lens is identified by its ID here — the 32-hex resource part of its ARN,
/// [`LensRecord::lens_id`] — in both the `ResourceId` and the message, whichever spelling
/// of the name the request used, because that is the identity of the lens that is missing.
pub fn lens_not_published(lens_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: tagged_message(
            "ResourceNotFound",
            &format!("{LENS_RESOURCE_TYPE} with id {lens_id} not found. Lens is not published."),
        ),
        resource_id: lens_id.to_string(),
        resource_type: LENS_RESOURCE_TYPE.to_string(),
    }
}

/// [`resource_not_found`] for a lens no version has been published from, which therefore
/// has no version a caller could read or compare: "The lens cannot be applied to
/// workloads or shared with other Amazon Web Services accounts until it's published with
/// CreateLensVersion" (`ImportLens` doc). The `ResourceId` is the alias AS SENT, as the
/// lookup failures above report it.
pub fn lens_without_published_version(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("a published version of the lens with alias {alias}"),
    )
}

/// [`conflict`] for a `LensVersion` the lens has already published: a published version
/// is immutable, so a second publish under the same name collides with it rather than
/// replacing it — the modelled `ConflictException`'s own "The resource has already been
/// processed" case.
///
/// `lens_arn` is the lens's own ARN ([`LensRecord::lens_arn`]) rather than the spelling the
/// request named it by, as the [`conflict`] envelope words every collision: what the caller
/// needs named is the lens that already carries the version.
pub fn lens_version_conflict(lens_arn: &str, version: &str) -> ConflictException {
    conflict(
        LENS_RESOURCE_TYPE,
        lens_arn.to_string(),
        &format!("A lens {lens_arn} with version name {version} already exists."),
    )
}

/// A modelled `ConflictException` (HTTP 409) for a request that collides with a
/// resource that already exists.
///
/// `resource_id` identifies the resource that is IN THE WAY, so a caller can fetch it.
/// Which spelling of a NAME the sentence quotes is each refusal's own recorded fact,
/// because the uniqueness rules here compare loosely (a workload name, a review
/// template's and a milestone name all ignore spaces and capitalization —
/// [`normalize_unique_name`]) and the two spellings then differ: a workload collision
/// names the workload as stored ([`workload_name_conflict`]), while a review template's
/// and a milestone's echo the name the refused request submitted
/// ([`review_template_name_conflict`], [`milestone_name_conflict`]). Normalization stays
/// confined to the COMPARISON that found the collision either way.
pub fn conflict(resource_type: &str, resource_id: String, sentence: &str) -> ConflictException {
    ConflictException {
        message: tagged_message("Conflict", sentence),
        resource_id,
        resource_type: resource_type.to_string(),
    }
}

/// [`conflict`] for a workload name that is already taken: "The name must be unique
/// within an account within an Amazon Web Services Region. Spaces and capitalization
/// are ignored when checking for uniqueness" (`CreateWorkloadInput$WorkloadName` doc).
///
/// The sentence names the workload that is IN THE WAY by the name it is STORED under
/// rather than the spelling the refused request submitted, as this refusal's recorded
/// wording shows: where the uniqueness comparison is loose the two spellings differ, and
/// what the caller is told is which workload already holds the name. `existing_name` and
/// `existing_id` are both that workload's, the pair
/// [`crate::state::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::workload_holding_name`]
/// answers with, so `ResourceId` and the sentence name one resource.
pub fn workload_name_conflict(existing_name: &str, existing_id: &str) -> ConflictException {
    conflict(
        WORKLOAD_RESOURCE_TYPE,
        existing_id.to_string(),
        &format!("A workload with name {existing_name} already exists."),
    )
}

/// [`conflict`] for a review template name that is already taken: a review template's
/// name identifies it within the account, so a create or a rename that would leave two
/// templates holding one name is refused.
///
/// Worded as AWS words this failure for a review template, and decided by the comparison
/// this service applies to every name it meters for uniqueness
/// ([`normalize_unique_name`]). `submitted_name` is the `TemplateName` the refused request
/// carried, echoed as sent (see [`conflict`]); `existing_arn` is the ARN of the template
/// already holding the name, so the caller is still told which template to fetch.
pub fn review_template_name_conflict(
    submitted_name: &str,
    existing_arn: &str,
) -> ConflictException {
    conflict(
        REVIEW_TEMPLATE_RESOURCE_TYPE,
        existing_arn.to_string(),
        &format!("A review template with name {submitted_name} already exists."),
    )
}

/// [`conflict`] for a milestone name that is already taken within its workload:
/// "Milestone names must be unique within a workload"
/// (`smithy_query(query="shape", shape="MilestoneName")`). `submitted_name` is the
/// `MilestoneName` the refused request carried, echoed as sent — padding and
/// capitalization included (see [`conflict`]) — and `existing_number` is the number of
/// the milestone already holding it, which `ResourceId` points the caller at.
///
/// The sentence names the WORKLOAD the uniqueness is scoped to — a milestone name is
/// unique within one workload, not across the account, so the refusal has to say which
/// resource it collided inside — and states the comparison the service applies
/// ([`normalize_unique_name`]), so a caller whose spelling differs only in
/// capitalization or whitespace can tell why it was refused.
pub fn milestone_name_conflict(
    submitted_name: &str,
    workload_id: &str,
    existing_number: i32,
) -> ConflictException {
    conflict(
        MILESTONE_RESOURCE_TYPE,
        milestone_resource_id(workload_id, existing_number),
        &format!(
            "Milestone with name {submitted_name} already exists under workload \
             {workload_id}. Capitalization and whitespace are ignored in milestone names."
        ),
    )
}

/// [`conflict`] for an idempotency token replayed with different parameters: the token
/// already names a resource, so the second request cannot be the same request.
pub fn idempotency_token_conflict(
    resource_type: &str,
    resource_id: String,
    client_request_token: &str,
) -> ConflictException {
    conflict(
        resource_type,
        resource_id,
        &format!(
            "A request with ClientRequestToken {client_request_token} was already \
             submitted with different parameters."
        ),
    )
}

/// [`conflict`] for a principal a workload is already shared with: a share is the grant
/// of one permission on one workload to one principal, so the principal already holding
/// one is what a second share request for it collides with. `existing` is the share that
/// is in the way, which `resource_id` names so the caller can update or delete it.
pub fn workload_share_conflict(existing: &WorkloadShareRecord) -> ConflictException {
    conflict(
        WORKLOAD_SHARE_RESOURCE_TYPE,
        existing.share_id.clone(),
        &format!(
            "The workload with ID {} is already shared with {}.",
            existing.workload_id, existing.shared_with
        ),
    )
}

/// [`conflict`] for a principal a LENS is already shared with — the lens counterpart of
/// [`workload_share_conflict`], for the same reason: a share is the grant of access to
/// one lens to one principal, so the principal already holding one is what a second
/// share request for it collides with. `existing` is the share that is in the way, which
/// `resource_id` names so the caller can delete it.
pub fn lens_share_conflict(existing: &LensShareRecord) -> ConflictException {
    conflict(
        LENS_SHARE_RESOURCE_TYPE,
        existing.share_id.clone(),
        &format!(
            "The lens with ARN {} is already shared with {}.",
            existing.lens_arn, existing.shared_with
        ),
    )
}

/// [`conflict`] for a principal a PROFILE is already shared with — the profile
/// counterpart of [`workload_share_conflict`], for the same reason: a share is the grant
/// of access to one profile to one principal, so the principal already holding one is
/// what a second share request for it collides with. `existing` is the share that is in
/// the way, which `resource_id` names so the caller can delete it.
pub fn profile_share_conflict(existing: &ProfileShareRecord) -> ConflictException {
    conflict(
        PROFILE_SHARE_RESOURCE_TYPE,
        existing.share_id.clone(),
        &format!(
            "The profile with ARN {} is already shared with {}.",
            existing.profile_arn, existing.shared_with
        ),
    )
}

/// [`conflict`] for a principal a REVIEW TEMPLATE is already shared with — the
/// review-template counterpart of [`workload_share_conflict`], for the same reason: a
/// share is the grant of access to one template to one principal, so the principal
/// already holding one is what a second share request for it collides with. `existing` is
/// the share that is in the way, which `resource_id` names so the caller can delete it.
pub fn template_share_conflict(existing: &TemplateShareRecord) -> ConflictException {
    conflict(
        TEMPLATE_SHARE_RESOURCE_TYPE,
        existing.share_id.clone(),
        &format!(
            "The review template with ARN {} is already shared with {}.",
            existing.template_arn, existing.shared_with
        ),
    )
}

/// [`resource_not_found`] for a share id that names no share of the lens.
///
/// A `ShareId` identifies a share on its own — the id space is unique across every
/// share this store holds — so the `ResourceId` is that id AS SENT, and the message names
/// the lens the share was looked for in, so a caller can tell a share of another lens
/// from a share that was deleted. `alias` is the lens as sent, which is how the request
/// named it. Shared by every operation that resolves a lens `ShareId`, so they cannot
/// word it differently.
pub fn lens_share_not_found(alias: &str, share_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_SHARE_RESOURCE_TYPE,
        share_id.to_string(),
        &format!("share with ID {share_id} of a lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a share id that names no share of the profile.
///
/// A `ShareId` identifies a share on its own — the id space is unique across every
/// share this store holds — so the `ResourceId` is that id AS SENT, and the message names
/// the profile the share was looked for in, so a caller can tell a share of another
/// profile from a share that was deleted. Shared by every operation that resolves a
/// profile `ShareId`, so they cannot word it differently.
pub fn profile_share_not_found(profile_arn: &str, share_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_SHARE_RESOURCE_TYPE,
        share_id.to_string(),
        &format!("share with ID {share_id} of a profile with ARN {profile_arn}"),
    )
}

/// [`resource_not_found`] for a share id that names no share of the review template.
///
/// The review-template counterpart of [`profile_share_not_found`], worded the same way and
/// for the same reasons: a `ShareId` identifies a share on its own, so the `ResourceId` is
/// that id AS SENT, and the message names the template the share was looked for in, so a
/// caller can tell a share of another template from a share that was revoked. The
/// `ResourceType` is the one every refusal about a review-template SHARE in this service
/// carries ([`TEMPLATE_SHARE_RESOURCE_TYPE`], as [`template_share_conflict`] does), rather
/// than the type of the template itself.
pub fn template_share_not_found(template_arn: &str, share_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        TEMPLATE_SHARE_RESOURCE_TYPE,
        share_id.to_string(),
        &format!("share with ID {share_id} of a review template with ARN {template_arn}"),
    )
}

/// [`resource_not_found`] for a `ShareInvitationId` that names no invitation this
/// principal has been offered — none is recorded under it, or the one that is was
/// offered to another principal.
///
/// The `ResourceId` is the id AS SENT, which is how the request named the invitation.
pub fn share_invitation_not_found(share_invitation_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        SHARE_INVITATION_RESOURCE_TYPE,
        share_invitation_id.to_string(),
        &format!("share invitation with ID {share_invitation_id}"),
    )
}

/// [`conflict`] for an invitation that is no longer waiting on an answer: the share it
/// mediates has left [`DEFAULT_SHARE_STATUS`], so the invitation has already been
/// processed — the modelled `ConflictException`'s own "The resource has already been
/// processed, was deleted" case. `status` is the `ShareStatus` the share stands in, which
/// is what tells the caller whether it (or someone else) already answered it.
pub fn share_invitation_answered_conflict(
    share_invitation_id: &str,
    status: &str,
) -> ConflictException {
    conflict(
        SHARE_INVITATION_RESOURCE_TYPE,
        share_invitation_id.to_string(),
        &format!(
            "The share invitation with ID {share_invitation_id} is no longer waiting on an \
             answer; the share it mediates is {status}."
        ),
    )
}

/// The refusal for a member the request had to carry and did not, whether the MODEL
/// requires it or this operation's own documentation does: the property-level finding the
/// layer that reads a request reports for either ([`missing_property_exception`]), so a
/// member this service requires by documentation reads like one the model requires.
pub fn absent_member_exception(member: &str) -> crate::ops::types::ValidationException {
    missing_property_exception(member)
}

/// The refusal for a workload that names neither `AwsRegions` nor `NonAwsRegions`:
/// "Either AwsRegions, NonAwsRegions, or both must be specified when creating a
/// workload" (the `CreateWorkload` doc) — an invariant of the resource, so it guards
/// every call that would leave a stored workload naming no Region. The rule spans both
/// members, so `Fields` carries an entry for EACH of them with the one sentence that
/// states it, and `Message` states that sentence once.
pub fn either_regions_exception() -> crate::ops::types::ValidationException {
    let sentence = format!("Either {AWS_REGIONS_MEMBER} or {NON_AWS_REGIONS_MEMBER} is required.");
    field_validation_exception(&[
        (AWS_REGIONS_MEMBER.to_string(), sentence.clone()),
        (NON_AWS_REGIONS_MEMBER.to_string(), sentence),
    ])
}

/// The refusal for an `AwsRegions` entry that is not a Region, or `None` when every
/// entry is one. A workload records the Regions it runs in, so a value no Region
/// carries can never be true of the resource and is refused before any state is
/// written; `NonAwsRegions` is deliberately NOT checked, because it names Regions
/// outside AWS, which no published list bounds.
pub fn unknown_aws_region_exception(
    regions: &[String],
) -> Option<crate::ops::types::ValidationException> {
    let unknown: Vec<(String, String)> = regions
        .iter()
        .filter(|region| !AWS_REGIONS.contains(&region.as_str()))
        .map(|region| {
            (
                AWS_REGIONS_MEMBER.to_string(),
                format!("Unknown region {region}."),
            )
        })
        .collect();
    (!unknown.is_empty()).then(|| field_validation_exception(&unknown))
}

/// Every value a workload's `IndustryType` may carry.
///
/// SOURCE: the member's own documentation — "If specified, must be one of the following:
/// Agriculture … Other" (`smithy_query(query="doc", shape="WorkloadIndustryType")`), listed
/// here in the order it publishes them. The model types the member as a plain string bounded
/// only by `@length(max: 100)`, so the boundary validator cannot hold it to the set; it is
/// written out here for the same reason [`AWS_REGIONS`] is — a value outside the published
/// set is an industry type the service does not publish, so an operation that WRITES a
/// workload refuses it rather than storing it.
pub const WORKLOAD_INDUSTRY_TYPES: &[&str] = &[
    "Agriculture",
    "Automobile",
    "Defense",
    "Design and Engineering",
    "Digital Advertising",
    "Education",
    "Environmental Protection",
    "Financial Services",
    "Gaming",
    "General Public Services",
    "Healthcare",
    "Hospitality",
    "InfoTech",
    "Justice and Public Safety",
    "Life Sciences",
    "Manufacturing",
    "Media & Entertainment",
    "Mining & Resources",
    "Oil & Gas",
    "Power & Utilities",
    "Professional Services",
    "Real Estate & Construction",
    "Retail & Wholesale",
    "Social Protection",
    "Telecommunications",
    "Travel, Transportation & Logistics",
    "Other",
];

/// The modelled member an out-of-set industry-type finding reports
/// (`CreateWorkloadInput$IndustryType` / `UpdateWorkloadInput$IndustryType`).
const INDUSTRY_TYPE_MEMBER: &str = "IndustryType";

/// The refusal for an `IndustryType` outside the published set, or `None` when the request
/// names none or names one of [`WORKLOAD_INDUSTRY_TYPES`].
///
/// Shared by every operation that writes a workload's industry type, so a value one of them
/// refuses cannot reach the store through the other.
pub fn unknown_industry_type_exception(
    industry_type: Option<&str>,
) -> Option<crate::ops::types::ValidationException> {
    let industry_type = industry_type?;
    if WORKLOAD_INDUSTRY_TYPES.contains(&industry_type) {
        return None;
    }
    Some(member_constraint_exception(
        INDUSTRY_TYPE_MEMBER,
        &format!(
            "Member must satisfy enum value set: [{}]",
            WORKLOAD_INDUSTRY_TYPES.join(", ")
        ),
    ))
}

/// The modelled member name an unknown-Region finding reports
/// (`CreateWorkloadInput$AwsRegions` / `UpdateWorkloadInput$AwsRegions`).
const AWS_REGIONS_MEMBER: &str = "AwsRegions";

/// The modelled member that satisfies the cross-member Regions rule in place of
/// `AwsRegions` (`CreateWorkloadInput$NonAwsRegions`).
const NON_AWS_REGIONS_MEMBER: &str = "NonAwsRegions";
