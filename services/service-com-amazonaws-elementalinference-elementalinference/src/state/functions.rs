// SPDX-License-Identifier: CC0-1.0

//! The FUNCTION resource: the record `CreateFunction` mints, `GetFunction` and
//! `ListFunctions` report, `UpdateFunction` mutates and `DeleteFunction` retires.
//!
//! A function "analyzes a feed by building a request from the feed's media and metadata"
//! (`CreateFunction`), and it runs only once a feed carries a function output that names
//! it. So WHICH feeds reference a function is not stored on the function: it is DERIVED
//! from the feed store ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_referencing_function`]),
//! exactly as a dictionary's references are, so one copy of that fact backs the
//! function's `references` list, its `REFERENCED` status, `DeleteFunction`'s refusal and
//! `UpdateFunction`'s narrowed set of changeable properties alike.
//!
//! A function is REGIONAL like every other resource of this service: the record carries
//! the Region whose endpoint created it ([`Function::region`]) and every operation
//! resolves its id within the Region the request was signed for.

use super::*;

use crate::ops::types::{FunctionSummary, FunctionTarget};

/// Value equality for a function's destination, which the record is compared by: a
/// create this Region has already served states the SAME target
/// ([`ComAmazonawsElementalinferenceElementalinferenceState::function_created_from`]),
/// and an update of a referenced function may not move it to another one. The shapes are
/// rendered from the model, which derives only `Debug`, `Clone` and `Default` on them, so
/// the comparison lives here beside the record it serves.
impl PartialEq for FunctionTarget {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bedrock(ours), Self::Bedrock(theirs)) => {
                ours.api == theirs.api && ours.model == theirs.model
            }
            (Self::Passthrough(_), Self::Passthrough(_)) => true,
            _ => false,
        }
    }
}

impl Eq for FunctionTarget {}


/// The `eventBridgeDelivery` a create that states none is stored with:
/// `EventBridgeDeliveryMode` declares `@default: "DISABLED"`
/// (`smithy_query(query="shape",shape="CreateFunctionResponse")`).
pub const DEFAULT_EVENT_BRIDGE_DELIVERY: &str = "DISABLED";

/// The `eventBridgeDelivery` value a referenced function may not be moved TO: "you can
/// set eventBridgeDelivery to DISABLED but not to ENABLED" while a feed references the
/// function (`UpdateFunction`).
pub const EVENT_BRIDGE_DELIVERY_ENABLED: &str = "ENABLED";

/// One media path a request expression can reference, and the `ContextType` the service
/// derives from it.
///
/// "Elemental Inference derives this value from the media paths that the request
/// expression references: FRAMES - Derived from media.frames. SUBTITLES - Derived from
/// media.transcript. CONTEXTUAL_METADATA - Derived from media.contextual. CLIP_METADATA
/// - Derived from media.clip." (`smithy_query(query="doc",shape="ContextType")`). The
/// rows are in the order the `ContextType` enum declares its values, which is the order
/// `contextTypes` is reported in.
pub const MEDIA_CONTEXT_PATHS: [(&str, &str); 4] = [
    ("frames", "FRAMES"),
    ("transcript", "SUBTITLES"),
    ("contextual", "CONTEXTUAL_METADATA"),
    ("clip", "CLIP_METADATA"),
];

/// The root object a request expression reads its media context off: "Elemental
/// Inference evaluates it against the media object of the feed, which holds the media
/// and metadata that the function asked for" (`CreateFunctionRequest$requestExpression`).
const MEDIA_ROOT: &str = "media";

/// The `ContextType` values whose media paths are MUTUALLY EXCLUSIVE: "A request
/// expression can reference at most one of SUBTITLES, CONTEXTUAL_METADATA, and
/// CLIP_METADATA, in addition to FRAMES" (`smithy_query(query="doc",shape="ContextType")`).
const EXCLUSIVE_CONTEXT_TYPES: [&str; 3] = ["SUBTITLES", "CONTEXTUAL_METADATA", "CLIP_METADATA"];

/// The media context a request expression references, as the `ContextTypeList` every
/// function response reports.
///
/// DERIVED from the expression rather than stored from the request — "Elemental
/// Inference derives this field from the media paths that the request expression
/// references; you can't set it" (`CreateFunctionResponse$contextTypes`) — so the list a
/// read reports is a fact about the expression the record holds, and an expression that
/// references no media yields an EMPTY list ("An empty list means that the expression
/// references no media, so Elemental Inference supplies none at run time").
pub fn derived_context_types(request_expression: &str) -> Vec<String> {
    MEDIA_CONTEXT_PATHS
        .iter()
        .filter(|(leaf, _)| references_media_path(request_expression, leaf))
        .map(|(_, context_type)| (*context_type).to_string())
        .collect()
}

/// Whether `expression` references the media path `media.<leaf>`.
///
/// A reference is the path spelled on the media root, so the match is on the whole
/// name at both ends: a longer name that merely CONTAINS one (`mymedia.frames`,
/// `media.framesPerSecond`) is a different path and not a reference to this one. Space
/// around the `.` is insignificant in the expression language, so it is skipped.
fn references_media_path(expression: &str, leaf: &str) -> bool {
    let bytes = expression.as_bytes();
    let mut from = 0;
    while let Some(offset) = expression[from..].find(MEDIA_ROOT) {
        let start = from + offset;
        let after_root = start + MEDIA_ROOT.len();
        from = after_root;
        if start > 0 && is_name_byte(bytes[start - 1]) {
            continue;
        }
        let mut cursor = skip_spaces(bytes, after_root);
        if bytes.get(cursor) != Some(&b'.') {
            continue;
        }
        cursor = skip_spaces(bytes, cursor + 1);
        if !expression[cursor..].starts_with(leaf) {
            continue;
        }
        if bytes
            .get(cursor + leaf.len())
            .is_none_or(|byte| !is_name_byte(*byte))
        {
            return true;
        }
    }
    false
}

/// Whether `byte` can appear inside a name in the expression language: a name that runs
/// up to one of the documented media paths is a different name, not a reference to it.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// The first offset at or after `from` that is not an ASCII space or tab.
fn skip_spaces(bytes: &[u8], from: usize) -> usize {
    let mut cursor = from;
    while matches!(bytes.get(cursor), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        cursor += 1;
    }
    cursor
}

/// The refusal a request expression earns for referencing more than one of the mutually
/// exclusive media contexts, or `None` when it references at most one.
///
/// "A request expression can reference at most one of SUBTITLES, CONTEXTUAL_METADATA,
/// and CLIP_METADATA, in addition to FRAMES"
/// (`smithy_query(query="doc",shape="ContextType")`), restated for the expression by
/// `CreateFunctionRequest$requestExpression`: "The expression can reference at most one
/// of media.transcript, media.contextual, and media.clip." The sentence names the paths
/// the request may choose between rather than the one it supplied, because the refusal
/// is about the combination.
pub fn conflicting_media_context_message(context_types: &[String]) -> Option<String> {
    let exclusive = context_types
        .iter()
        .filter(|context_type| EXCLUSIVE_CONTEXT_TYPES.contains(&context_type.as_str()))
        .count();
    (exclusive > 1).then(|| {
        "The expression can reference at most one of media.transcript, media.contextual, \
         and media.clip."
            .to_string()
    })
}

/// The refusal a passthrough function's request expression earns for referencing frames,
/// or `None` when it does not.
///
/// "A passthrough function can't reference media.frames"
/// (`smithy_query(query="doc",shape="PassthroughFunctionTarget")`), restated by
/// `CreateFunctionRequest$requestExpression`: "For a passthrough target, it can't
/// reference media.frames."
pub fn passthrough_frames_message(target: &FunctionTarget, context_types: &[String]) -> Option<String> {
    let references_frames = context_types.iter().any(|context_type| context_type == "FRAMES");
    match target {
        FunctionTarget::Passthrough(_) if references_frames => {
            Some("A passthrough function can't reference media.frames.".to_string())
        }
        FunctionTarget::Passthrough(_) | FunctionTarget::Bedrock(_) => None,
    }
}

/// The refusal a referenced function earns for a change the service does not admit while
/// a feed references it, or `None` when every change the request asks for is admissible.
///
/// "While a feed references the function, you can change only its expressions and its
/// description, and you can set eventBridgeDelivery to DISABLED but not to ENABLED. The
/// request expression must keep referencing the same media context, as reported in
/// contextTypes." (`UpdateFunction`). Each clause names the property it refused, which
/// the request supplied.
pub fn referenced_function_change_message(
    held: &Function,
    name: Option<&str>,
    target: Option<&FunctionTarget>,
    event_bridge_delivery: Option<&str>,
    context_types: &[String],
) -> Option<String> {
    if name.is_some_and(|name| name != held.name) {
        return Some(referenced_property_message("name"));
    }
    if target.is_some_and(|target| *target != held.target) {
        return Some(referenced_property_message("target"));
    }
    if event_bridge_delivery.is_some_and(|mode| mode == EVENT_BRIDGE_DELIVERY_ENABLED) {
        return Some(
            "You can't set eventBridgeDelivery to ENABLED while a feed references this \
             function. Remove the function output from each feed that references it and try \
             again."
                .to_string(),
        );
    }
    (context_types != held.context_types).then(|| {
        "The request expression must keep referencing the same media context while a feed \
         references this function. Remove the function output from each feed that references \
         it and try again."
            .to_string()
    })
}

/// What the service answers a request that changes a property a referenced function does
/// not admit a change to, naming that property.
fn referenced_property_message(property: &str) -> String {
    format!(
        "You can't change the {property} of a function while a feed references it. Remove the \
         function output from each feed that references it and try again."
    )
}

/// The refusal `DeleteFunction` answers while a feed still references the function.
///
/// "If any feed still references the function, the operation returns a
/// ConflictException. A disabled function output still references the function. Use
/// GetFunction to find those feeds, then remove the function output from each one with
/// UpdateFeed, or delete the feed, before retrying." (`DeleteFunction`). The sentence
/// reports the NUMBER of feeds the refusal was decided from — derived from the same feed
/// store — and names none of them, so it discloses no other caller's resource.
pub fn function_referenced_message(reference_count: usize) -> String {
    format!(
        "This function is referenced by {reference_count} feed(s). Remove the function output \
         from each feed that references it, or delete those feeds, and try again."
    )
}

/// The lifecycle statuses a function record holds.
///
/// The model's `FunctionStatus` also carries `REFERENCED`, which is not a stored value:
/// it is DERIVED from the feeds whose function outputs name the function (see
/// [`Function::reported_status`]). `DELETED` is not one either — the model says it is
/// "Not currently returned" and a deletion that completes REMOVES the record, so no
/// record is left to report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionLifecycle {
    /// The function has been accepted; "the function is ready to use as soon as the
    /// request succeeds" (`FunctionStatus`).
    Creating,
    /// The function is settled and no feed references it.
    Available,
    /// A `DeleteFunction` has been accepted; the deletion completes when it falls due.
    Deleting,
}

impl FunctionLifecycle {
    /// This status as the modelled `FunctionStatus` enum spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Deleting => "DELETING",
        }
    }
}

/// A function: an expression pair, a destination for the request it builds, and the
/// media context the service derived from it.
#[derive(Debug, Clone)]
pub struct Function {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region whose endpoint this function was created through, and so the only one
    /// it exists for — the Region field of [`Self::arn`].
    pub region: String,
    pub target: FunctionTarget,
    pub request_expression: String,
    pub response_expression: Option<String>,
    pub description: Option<String>,
    /// `DISABLED` or `ENABLED`, from the modelled `EventBridgeDeliveryMode`.
    pub event_bridge_delivery: String,
    /// The media context [`derived_context_types`] read off [`Self::request_expression`],
    /// stored WITH the expression so the two cannot disagree and an update that rewrites
    /// the expression rewrites both.
    pub context_types: Vec<String>,
    pub status: FunctionLifecycle,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this function's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
}

impl Function {
    /// Put this function into a transient status the clock will move on by itself, and
    /// return the instant that move falls due — the function's counterpart of
    /// [`Dictionary::enter_transient`].
    #[must_use]
    pub fn enter_transient(&mut self, status: FunctionLifecycle, now: Instant) -> Instant {
        let due = now + TRANSITION_SPAN;
        self.status = status;
        self.transition_due_at = Some(due);
        due
    }

    /// The `FunctionStatus` this function reports given `references`, the feeds that
    /// currently reference it.
    ///
    /// A function in a transient lifecycle status reports that status; a settled one
    /// reports `REFERENCED` while any feed's function output names it — which is also
    /// what refuses its delete and narrows what an update may change — and `AVAILABLE`
    /// while none does.
    pub fn reported_status(&self, references: &[String]) -> &'static str {
        match self.status {
            FunctionLifecycle::Available if !references.is_empty() => "REFERENCED",
            status => status.as_wire(),
        }
    }

    /// The tags of this function as the `TagMap` every function response carries; an
    /// untagged function reports an EMPTY map rather than an absent member, as
    /// [`Dictionary::tag_map`] does.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
    }

    /// Whether this function is still EXACTLY what a `CreateFunction` left behind, in
    /// the one respect a create does not state — its lifecycle. The function's
    /// counterpart of [`Dictionary::is_as_created`].
    pub fn is_as_created(&self) -> bool {
        matches!(
            self.status,
            FunctionLifecycle::Creating | FunctionLifecycle::Available
        )
    }

    /// The `FunctionSummary` projection `ListFunctions` reports.
    ///
    /// `references` is the feeds that currently reference this function, so the
    /// summary's `status` is the SAME reported status `GetFunction` answers with and a
    /// list and a get cannot disagree about one stored function.
    pub fn summary(&self, references: &[String]) -> FunctionSummary {
        FunctionSummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            status: self.reported_status(references).to_string(),
            target: Some(self.target.clone()),
            context_types: Some(self.context_types.clone()),
        }
    }
}

/// The ARN of a function in `region`:
/// `arn:aws:elemental-inference:<region>:<account>:function/<id>`.
pub fn function_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("function/{id}"))
}

/// The ARN of the function an output config names, if it names one.
///
/// Only a function output can: `FunctionConfig$functionArn` is "The ARN of the function
/// to run", and no other member of the `OutputConfig` union names a function.
pub fn function_reference(config: &OutputConfig) -> Option<&str> {
    match config {
        OutputConfig::Function(function) => Some(function.function_arn.as_str()),
        OutputConfig::Cropping(_)
        | OutputConfig::Clipping(_)
        | OutputConfig::Subtitling(_)
        | OutputConfig::contextualMetadata(_) => None,
    }
}

/// A `functionArn` that names no function of the request's Region, named by an output's
/// function config — the function's counterpart of [`cannot_find_dictionary_message`],
/// and likewise built from the value the request supplied.
///
/// "The function must exist in the same account and the same Region as the feed"
/// (`FunctionConfig$functionArn`), so an ARN that resolves to no record of this endpoint
/// describes a reference the service's integrity rules do not admit.
pub fn cannot_find_function_message(function_arn: &str) -> String {
    format!("Can't find function {function_arn}. Use a different functionArn and try again.")
}

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// Mint a fresh `FunctionId`: [`FEED_ID_LEN`] characters of [`ID_ALPHABET`], the
    /// form this service issues its ids in (`FunctionId`'s pattern is `^[a-z0-9]{19}$`,
    /// the same as a `FeedId`'s), minted through the same seed-plus-sequence
    /// construction as every other id of this store.
    pub fn mint_function_id(&mut self) -> String {
        self.mint_id()
    }

    /// The one function `id` names IN `region`, or `None` when that Region holds no such
    /// function — the function's counterpart of [`Self::dictionary`], with the same
    /// Region rule and the same rule about a completed deletion.
    pub fn function(&self, region: &str, id: &str) -> Option<&Function> {
        self.functions
            .get(id)
            .filter(|function| function.region == region)
    }

    /// The one function `id` names in `region`, for a caller that writes it.
    pub fn function_mut(&mut self, region: &str, id: &str) -> Option<&mut Function> {
        self.functions
            .get_mut(id)
            .filter(|function| function.region == region)
    }

    /// Every function of `region`, in `FunctionId` order — what `ListFunctions` pages
    /// over, since a list reports the functions of the endpoint it was sent to and
    /// nothing else.
    pub fn functions_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Function> + 'a {
        self.functions
            .values()
            .filter(move |function| function.region == region)
    }

    /// The refusal `CreateFunction` answers in `region`, or `None` while that Region may
    /// take one more function — on the one rule every create of this service follows
    /// ([`create_quota_refusal`]), against the function store and [`MAX_FUNCTIONS`].
    pub fn function_quota_refusal(&self, region: &str) -> Option<String> {
        self.quota_refusal(
            self.functions.values().map(|function| function.region.as_str()),
            region,
            MAX_FUNCTIONS,
            "functions",
        )
    }

    /// The ids of the feeds that reference `function_arn`, in feed-id order — the
    /// `FeedReferences` list every function response carries.
    ///
    /// DERIVED from the feed store, which is the only place a reference lives: a feed
    /// references a function exactly while one of its outputs' function configs names
    /// that ARN, "enabled or disabled" (`CreateFunctionResponse$references`). Scoped to
    /// `region` because the ARN a stored output carries is the function's own, which
    /// carries the Region that minted it.
    pub fn feeds_referencing_function(&self, region: &str, function_arn: &str) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| feed.references_function(function_arn))
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// The function a `CreateFunction` request has ALREADY minted in `region`, if this
    /// store holds it — the function's counterpart of [`Self::dictionary_created_from`],
    /// over the members a function create sets.
    #[allow(clippy::too_many_arguments)]
    pub fn function_created_from<'a>(
        &'a self,
        region: &'a str,
        name: &str,
        target: &FunctionTarget,
        request_expression: &str,
        response_expression: Option<&str>,
        description: Option<&str>,
        event_bridge_delivery: &str,
        tags: &BTreeMap<String, String>,
    ) -> Option<&'a Function> {
        self.functions_in(region).find(|function| {
            function.is_as_created()
                && function.name == name
                && function.target == *target
                && function.request_expression == request_expression
                && function.response_expression.as_deref() == response_expression
                && function.description.as_deref() == description
                && function.event_bridge_delivery == event_bridge_delivery
                && &function.tags == tags
        })
    }
}
