// SPDX-License-Identifier: CC0-1.0

//! The JSON-Patch grammar this service's update operations share: the `op` kinds a
//! `patchOperations` entry may carry, the paths each resource publishes for each op, the
//! refusals an unsupported path or op earns, one parsed [`Patch`] and the appliers that
//! set, clear and bound the member a patch names.
//!
//! Split out of `state.rs` so the store's records and this grammar each live in a file of
//! their own; every name is re-exported by the parent, so a handler still reaches it as
//! `crate::state::…`.

use super::*;

/// The `op` of one [`Patch`]: the values the model's `Op` enum declares
/// (`smithy_query(query="shape", shape="Op")`: add, remove, replace, move, copy, test),
/// which [`crate::ops::types::OP_VALUES`] lists on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOpKind {
    Add,
    Remove,
    Replace,
    Move,
    Copy,
    Test,
}

impl PatchOpKind {
    /// The kind an `op` string names, or `None` when it names none of them.
    pub fn parse(op: &str) -> Option<Self> {
        match op {
            "add" => Some(Self::Add),
            "remove" => Some(Self::Remove),
            "replace" => Some(Self::Replace),
            "move" => Some(Self::Move),
            "copy" => Some(Self::Copy),
            "test" => Some(Self::Test),
            _ => None,
        }
    }

    /// The wire spelling, for a refusal message that quotes the op back.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Replace => "replace",
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Test => "test",
        }
    }
}

/// Why a patch could not be applied. A handler maps this to the modelled error its own
/// operation declares for that kind of refusal, so the same malformed patch is answered
/// with the same status whichever resource it targeted.
#[derive(Debug, Clone)]
pub enum PatchFailure {
    /// The request is not valid: an unknown op, a path this resource has no updateable
    /// member for, a value that is not of the member's type. `BadRequestException`.
    BadRequest(String),
    /// The patch names a resource that does not exist. `NotFoundException`.
    NotFound(String),
    /// Applying the patch would collide with another stored record.
    /// `ConflictException`.
    Conflict(String),
}

/// Which paths ONE resource PUBLISHES, per `op`.
///
/// Which paths a patch may name is a property of the op as much as of the resource: a
/// single-valued member is `replace`d, while an element is `add`ed to or `remove`d from a
/// multi-valued one, so the sets belonging to two ops differ and one of them can be empty.
/// A resource that enumerates its permitted paths in a refusal
/// ([`InvalidPathRefusal::PathOpAndSupported`]) therefore enumerates the set belonging to
/// the op the request actually sent, and an op the resource publishes no path for is told
/// to choose one it supports instead of being offered an empty list.
///
/// The table is what the SERVICE publishes, which is why it is given rather than derived
/// from a handler's match arms: a resource may apply an op on a path its published list
/// does not carry (an authorizer's `providerARNs`), and a handler that gates its own edits
/// on this table ([`Self::accepts`]) says so where it does.
#[derive(Debug, Clone, Copy)]
pub struct SupportedPatchPaths {
    /// The paths an `add` names: typically the resource's multi-valued members, one
    /// element at a time, and the nested settings an `add` creates.
    pub add: &'static [&'static str],
    /// The paths a `remove` names: the ones an element or a whole nested setting goes away
    /// from.
    pub remove: &'static [&'static str],
    /// The paths a `replace` names: the resource's updateable single-valued members.
    pub replace: &'static [&'static str],
}

impl SupportedPatchPaths {
    /// The table of a resource that publishes its patchable paths for `replace` ONLY.
    ///
    /// A recorded refusal of a path under `add` enumerates no list for these resources: it
    /// tells the caller to choose an op the resource supports instead, so the set belonging
    /// to `add` — and to `remove`, which these resources publish no path for either — is
    /// empty, and `replace` is the op whose set is the published list. Which ops a given
    /// member then admits is the member's own rule (an optional member can be cleared),
    /// applied where that member is patched rather than by this table.
    pub const fn replace_only(paths: &'static [&'static str]) -> Self {
        Self {
            add: &[],
            remove: &[],
            replace: paths,
        }
    }

    /// The table of a resource that publishes ONE list of patchable paths, enumerated
    /// whichever of `add`, `remove` and `replace` named the rejected path.
    ///
    /// Some of these resources publish their patchable paths per RESOURCE rather than per
    /// op — a recorded refusal enumerates the same list for every op the resource takes —
    /// and which of the three ops a given path admits is then the member's own rule (a
    /// required member cannot be removed, a collection member is not replaced whole),
    /// applied where that member is patched rather than by this table. A resource whose
    /// recorded refusal enumerates DIFFERENT lists per op (an API key's) names the three
    /// sets separately instead.
    pub const fn for_every_op(paths: &'static [&'static str]) -> Self {
        Self {
            add: paths,
            remove: paths,
            replace: paths,
        }
    }

    /// The paths `op` may name on this resource, in the order the service prints them —
    /// or `None` for an op the resource supports NO path for, which is a refusal of the
    /// op rather than of the path ([`Patch::unsupported_op`]).
    pub fn for_op(self, op: PatchOpKind) -> Option<&'static [&'static str]> {
        match op {
            PatchOpKind::Add => Some(self.add),
            PatchOpKind::Remove => Some(self.remove),
            PatchOpKind::Replace => Some(self.replace),
            PatchOpKind::Move | PatchOpKind::Copy | PatchOpKind::Test => None,
        }
    }

    /// Whether this resource accepts `patch`: its `op` must be one the resource supports
    /// at all, and the member its `path` names must be in that op's own set. Membership
    /// is by the member the pointer's FIRST reference token names, so an element of a
    /// multi-valued member (`/stages/abcde12345~1prod`) is accepted exactly where the
    /// member itself (`/stages`) is.
    pub fn accepts(self, patch: &Patch) -> bool {
        self.for_op(patch.op).is_some_and(|paths| {
            paths
                .iter()
                .any(|supported| supported.strip_prefix('/') == Some(patch.member()))
        })
    }
}

/// How ONE operation words its refusal of a patch `path` it cannot apply.
///
/// API Gateway does not word that refusal the same way for every resource: for some of
/// them it is the bare sentence [`invalid_patch_path`] builds, for others it also quotes
/// the `op` the path rode in on and enumerates the paths that op may name, and for a
/// pointer into a nested structure the structure itself owns the sentence. The wording is a
/// property of the OPERATION, not of the mistake, so each handler names its own when it
/// reads the request's `patchOperations` ([`parse_patches`] /
/// [`parse_patches_refused_with`]) and every path refusal that request goes on to produce —
/// a member the resource does not have, a pointer that is no JSON Pointer, one reaching
/// into a single-valued member, one naming a map member with no key — reads the same.
#[derive(Debug, Clone, Copy)]
pub enum InvalidPathRefusal {
    /// The fixed sentence and the path the request sent, and nothing else.
    PathOnly,
    /// The path the request sent and then the service's fixed `Invalid request input` —
    /// the wording of an operation that holds a patch path against the request as INPUT
    /// rather than against the resource's patchable set, so it offers no set at all.
    PathThenInvalidInput,
    /// The path quoted, the `op` of the same `patchOperations` entry, and the paths that
    /// op may name on the resource — read from the resource's own [`SupportedPatchPaths`]
    /// table, in the order the service itself prints them.
    PathOpAndSupported(SupportedPatchPaths),
    /// The refusal of a resource whose unapplicable pointers are read as PER-METHOD
    /// SETTING paths: the rejected path and then the whole pointer set the resource
    /// publishes, enumerated in fixed text whichever `op` named the path.
    ///
    /// A stage is such a resource — every pointer that is no member of it is read as a
    /// method's setting path — and a recorded refusal shows it enumerating the one set for
    /// the path it rejected, placeholders and all, rather than a set belonging to the op.
    MethodSettingPath(&'static [&'static str]),
    /// The refusal a NESTED structure of the resource owns: the structure's own member
    /// name and the full pointers into it, and nothing of the rejected path. A handler
    /// that dispatches a pointer into such a structure re-labels the patch with this
    /// ([`Patch::refused_with`]), so every refusal that subtree produces reads the
    /// structure's sentence rather than the resource's.
    NestedStructure {
        /// The member the pointer entered, as the structure's sentence names it.
        member: &'static str,
        /// The pointers that structure accepts, spelled from the resource root, per `op`.
        paths: SupportedPatchPaths,
    },
}

impl InvalidPathRefusal {
    /// The `BadRequestException` message for `path`, named by `op`.
    ///
    /// The enumerated set is the one belonging to `op`, so the sentence never offers a
    /// path the op that was submitted would refuse; an op the resource supports no path
    /// for has nothing to enumerate and is refused as an op instead, with the sentence
    /// `unsupported_op` — the one the resource enumerates its OPS in.
    pub(super) fn message(self, path: &str, op: PatchOpKind, unsupported_op: UnsupportedOpRefusal) -> String {
        match self {
            Self::PathOnly => invalid_patch_path(path),
            Self::PathThenInvalidInput => invalid_request_input(path),
            Self::MethodSettingPath(paths) => invalid_method_setting_path(path, paths),
            Self::PathOpAndSupported(supported) => match supported.for_op(op) {
                // The bracketed, comma-separated list is the service's own, as are the two
                // spaces the shared frame prints after `path`.
                Some(paths) if !paths.is_empty() => {
                    invalid_patch_path_for_op(path, op, &format!("[{}]", paths.join(", ")))
                }
                // An op the resource publishes NO path for: there is no list to offer, so
                // the sentence sends the caller to an op the resource does support instead
                // of enumerating nothing, as a recorded refusal under such an op shows.
                Some(_) => unsupported_patch_path_op(path, op),
                None => unsupported_op.sentence(path).to_string(),
            },
            Self::NestedStructure { member, paths } => match paths.for_op(op) {
                Some(paths) if !paths.is_empty() => invalid_nested_patch_path(member, paths),
                Some(_) => unsupported_patch_path_op(path, op),
                None => unsupported_op.sentence(path).to_string(),
            },
        }
    }
}

/// How ONE operation refuses a patch entry whose `op` the path it names cannot take.
///
/// API Gateway does not answer that condition the same way for every resource, and which
/// answer a resource gives is a property of the OPERATION, read off its own recorded
/// refusals the way [`InvalidPathRefusal`] is: some of them hold the entry against the OP
/// and enumerate the ops they take, in their own words, while others hold it against the
/// PATH — for those, a recorded refusal of a pointer the resource publishes, carried by an
/// op that pointer cannot take, names the path and says nothing of the op.
#[derive(Debug, Clone, Copy)]
pub enum UnsupportedOpRefusal {
    /// The operation enumerates the ops it takes: every op the path's member cannot take
    /// earns this one sentence, whichever pointer carried it — see
    /// [`INVALID_PATCH_OPERATION`] for the one most of these resources use.
    Ops(&'static str),
    /// The operation keys the refusal on the PATH: an op the pointer's member cannot take
    /// is refused as that pointer, in the sentence the operation words a path refusal in,
    /// so one unapplicable entry reads the same whether its op or its path is what the
    /// resource has no applier for.
    Path,
}

impl UnsupportedOpRefusal {
    /// The sentence this refusal states an op in.
    ///
    /// An operation that keys the refusal on the path states no sentence of its own, so it
    /// states the path refusal `path` earns under the wording that operation uses — which
    /// for every operation that keys refusals this way is the bare path sentence.
    pub(super) fn sentence(self, path: &str) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Ops(sentence) => std::borrow::Cow::Borrowed(sentence),
            Self::Path => std::borrow::Cow::Owned(invalid_patch_path(path)),
        }
    }
}

/// The `BadRequestException` message for a patch `path` the `op` that named it cannot be
/// applied on, stating the pointers that op CAN name as `supported`.
///
/// The frame is the service's own fixed text — the opening, the TWO spaces before the quoted
/// path, the op quoted after it, and `Must be one of: ` — and the variable parts are the path
/// and the op, both exactly as the request wrote them. What follows differs by resource: a
/// resource that publishes a SET of pointers for the op prints it bracketed and
/// comma-separated ([`InvalidPathRefusal::PathOpAndSupported`]), and one whose set for that op
/// is a single pointer TEMPLATE prints that template alone — so one frame serves both and the
/// two can never drift apart.
pub fn invalid_patch_path_for_op(path: &str, op: PatchOpKind, supported: &str) -> String {
    format!(
        "Invalid patch path  '{path}' specified for op '{}'. Must be one of: {supported}",
        op.as_str()
    )
}

/// The `BadRequestException` message for a patch `path` named by an `op` the targeted
/// resource publishes no path for at all.
///
/// Same opening as the enumerating sentence — the fixed text, the two spaces the service
/// itself prints, the rejected path and the op that named it — and then, in place of a
/// list it has nothing to fill, the fixed instruction to choose an op the resource
/// supports.
pub(super) fn unsupported_patch_path_op(path: &str, op: PatchOpKind) -> String {
    format!(
        "Invalid patch path  '{path}' specified for op '{}'. Please choose supported operations",
        op.as_str()
    )
}

/// One parsed entry of `patchOperations`.
#[derive(Debug, Clone)]
pub struct Patch {
    /// The edit to perform.
    pub op: PatchOpKind,
    /// The `path` exactly as the request spelled it, for a refusal that quotes it back.
    pub path: String,
    /// `path` split into its JSON Pointer reference tokens, each unescaped — so
    /// `/binaryMediaTypes/image~1png` is `["binaryMediaTypes", "image/png"]`.
    pub tokens: Vec<String>,
    /// The op's `value`: "applicable for the add or replace operation"
    /// (`PatchOperation$value`).
    pub value: Option<String>,
    /// The `copy`/`move` source pointer (`PatchOperation$from`).
    pub from: Option<String>,
    /// How the operation this patch was sent to words a refusal of its `path`.
    pub invalid_path: InvalidPathRefusal,
    /// How the operation this patch was sent to words a refusal of an `op` the pointer's
    /// own member cannot take — see [`UnsupportedOpRefusal`].
    pub unsupported_op: UnsupportedOpRefusal,
}

/// Decode one JSON Pointer reference token.
///
/// From `PatchOperation$path`: "Any slash ("/") character appearing in path names must
/// be escaped with "~1"". `~1` is decoded before `~0`, as RFC 6901 requires, so an
/// escaped tilde cannot go on to produce a second escape.
pub(super) fn unescape_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

impl Patch {
    /// Parse one wire `PatchOperation`, refusing a path it cannot read the way
    /// `invalid_path` words it and an op the resource takes no path for the way
    /// `unsupported_op` words that.
    pub(super) fn parse(
        operation: &crate::ops::types::PatchOperation,
        invalid_path: InvalidPathRefusal,
        unsupported_op: UnsupportedOpRefusal,
    ) -> Result<Self, PatchFailure> {
        let op = operation.op.as_deref().ok_or_else(|| {
            PatchFailure::BadRequest(
                "each patchOperations entry must carry an op".to_string(),
            )
        })?;
        let op = PatchOpKind::parse(op).ok_or_else(|| {
            PatchFailure::BadRequest(format!(
                "Invalid patch operation specified: {op}; expected one of {}",
                crate::ops::types::OP_VALUES.join(", ")
            ))
        })?;
        let path = operation.path.as_deref().ok_or_else(|| {
            PatchFailure::BadRequest(
                "each patchOperations entry must carry a path".to_string(),
            )
        })?;
        bound_chars("a patchOperations path", path, MAX_STRING_CHARS)
            .map_err(PatchFailure::BadRequest)?;
        // A JSON Pointer to a member of the targeted resource starts at the resource
        // root, so it begins with `/` and names at least one member.
        let tokens: Vec<String> = match path.strip_prefix('/') {
            Some(rest) if !rest.is_empty() => rest.split('/').map(unescape_pointer_token).collect(),
            _ => {
                return Err(PatchFailure::BadRequest(invalid_path.message(
                    path,
                    op,
                    unsupported_op,
                )))
            }
        };
        Ok(Self {
            op,
            path: path.to_string(),
            tokens,
            value: operation.value.clone(),
            from: operation.from.clone(),
            invalid_path,
            unsupported_op,
        })
    }

    /// This patch's own refusal of its `path`, worded the way the operation it was sent
    /// to words one.
    pub(super) fn invalid_path_message(&self) -> String {
        self.invalid_path
            .message(&self.path, self.op, self.unsupported_op)
    }

    /// The member the path's first reference token names.
    pub fn member(&self) -> &str {
        self.tokens.first().map_or("", String::as_str)
    }

    /// Whether this patch UNSETS the single-valued member its path names rather than
    /// setting it to a value.
    ///
    /// A `remove` is one such op. So is a `replace` that carries NO `value`, and one that
    /// carries an EMPTY one: `value` is "applicable for the add or replace operation"
    /// (`PatchOperation$value`) and is the member's new value, so a `replace` that names
    /// none — by omitting it or by sending no characters at all — names no value to set,
    /// and API Gateway reads it as the clear of the member the path addresses. That is how
    /// a caller unsets an optional field through a path that publishes a `replace`, as the
    /// recorded 200 answers to an emptied `cloudwatchRoleArn` and an emptied
    /// `contentHandling` — each reported with the member gone — show. The member's own rule
    /// then decides whether it CAN be unset: an optional member is cleared and a member the
    /// service always reports a value for refuses the op, exactly as it refuses a `remove`.
    ///
    /// Read where a single-valued member is applied, so one answer serves every member of
    /// every resource this service patches rather than each applier deciding again. A path
    /// that names one ENTRY of a map or a collection is not this: such an entry is dropped
    /// by the `remove` the service publishes for it, and its value is the entry's own.
    pub fn unsets_member(&self) -> bool {
        match self.op {
            PatchOpKind::Remove => true,
            PatchOpKind::Replace => self.value.as_deref().is_none_or(str::is_empty),
            _ => false,
        }
    }

    /// The op's `value`, refusing an add or replace that carries none: there is nothing
    /// to set the member to.
    pub fn require_value(&self) -> Result<&str, PatchFailure> {
        self.value.as_deref().ok_or_else(|| {
            PatchFailure::BadRequest(format!(
                "a {} of {} requires a value",
                self.op.as_str(),
                self.path
            ))
        })
    }

    /// The op's `value` read as the boolean a boolean member takes.
    pub fn require_bool(&self) -> Result<bool, PatchFailure> {
        let value = self.require_value()?;
        match value.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(PatchFailure::BadRequest(format!(
                "{} takes a boolean value of true or false, got {value:?}",
                self.path
            ))),
        }
    }

    /// The op's `value` read as the integer an integer member takes.
    pub fn require_i32(&self) -> Result<i32, PatchFailure> {
        let value = self.require_value()?;
        value.trim().parse().map_err(|_| {
            PatchFailure::BadRequest(format!(
                "{} takes an integer value, got {value:?}",
                self.path
            ))
        })
    }

    /// The op's `value` read as the number a double member takes.
    pub fn require_f64(&self) -> Result<f64, PatchFailure> {
        let value = self.require_value()?;
        value.trim().parse().map_err(|_| {
            PatchFailure::BadRequest(format!(
                "{} takes a number value, got {value:?}",
                self.path
            ))
        })
    }

    /// This patch with a refusal of its `path` worded the way `invalid_path` words one,
    /// for a handler that dispatches the pointer into a part of the resource whose
    /// refusals the service words differently — a nested structure that enumerates its
    /// own pointers ([`InvalidPathRefusal::NestedStructure`]). Re-labelled ONCE, where the
    /// dispatch happens, so every refusal that subtree goes on to produce reads the same
    /// sentence.
    pub fn refused_with(&self, invalid_path: InvalidPathRefusal) -> Self {
        Self {
            invalid_path,
            ..self.clone()
        }
    }

    /// This patch as it addresses a member of a NESTED structure: the first `depth`
    /// reference tokens — the ones naming the structure itself — are dropped, so the
    /// structure's own members are addressed exactly the way a top-level member is.
    /// `path` is kept whole, so a refusal still quotes the pointer the request sent.
    pub fn rebased(&self, depth: usize) -> Self {
        Self {
            tokens: self.tokens.iter().skip(depth).cloned().collect(),
            ..self.clone()
        }
    }

    /// Refuse a path that reaches INTO a single-valued member (`/name/first`): such a
    /// member has no sub-member a pointer could name.
    pub fn require_single_valued(&self) -> Result<(), PatchFailure> {
        if self.tokens.len() == 1 {
            return Ok(());
        }
        Err(PatchFailure::BadRequest(self.invalid_path_message()))
    }

    /// The refusal for a path the targeted resource has no updateable member for.
    ///
    /// "Attempts to apply an unsupported operation on a resource will return an error
    /// message" (`PatchOperation$op`), and which message that is belongs to the operation
    /// the patch was sent to — see [`InvalidPathRefusal`].
    pub fn unsupported_path(&self) -> PatchFailure {
        PatchFailure::BadRequest(self.invalid_path_message())
    }

    /// The refusal for an op the member this path names does not support.
    ///
    /// "Not all valid operations are supported for a given resource. Support of the
    /// operations depends on specific operational contexts. Attempts to apply an
    /// unsupported operation on a resource will return an error message"
    /// (`PatchOperation$op`), and WHICH message that is belongs to the operation the patch
    /// was sent to: one that enumerates the ops it takes answers that enumeration, and one
    /// that keys an unapplicable entry on its PATH answers the path refusal instead — see
    /// [`UnsupportedOpRefusal`].
    pub fn unsupported_op(&self) -> PatchFailure {
        match self.unsupported_op {
            UnsupportedOpRefusal::Ops(sentence) => PatchFailure::BadRequest(sentence.to_string()),
            UnsupportedOpRefusal::Path => self.unsupported_path(),
        }
    }

    /// The entry of a collection member this patch names.
    ///
    /// `depth` is how many reference tokens the member itself takes, so the entry is
    /// what the path carries beyond it (`/binaryMediaTypes/image~1png`) and otherwise
    /// the op's own `value` (`op=add, path=/binaryMediaTypes, value=image/png`) — the
    /// two spellings a client can address one entry of a string collection with.
    pub fn collection_entry(&self, depth: usize) -> Result<String, PatchFailure> {
        match self.tokens.get(depth..) {
            Some(rest) if !rest.is_empty() => Ok(rest.join("/")),
            _ => Ok(self.require_value()?.to_string()),
        }
    }

    /// The key of a map member this patch names: the reference token beyond the member,
    /// which is the key, unescaped.
    pub fn map_key(&self, depth: usize) -> Result<String, PatchFailure> {
        match self.tokens.get(depth..) {
            Some(rest) if !rest.is_empty() => Ok(rest.join("/")),
            _ => Err(PatchFailure::BadRequest(self.invalid_path_message())),
        }
    }
}

/// The `BadRequestException` message for a patch `path` the targeted resource cannot be
/// addressed by.
///
/// This is the wording of [`InvalidPathRefusal::PathOnly`] — the fixed text `Invalid
/// patch path` and then the path exactly as the request spelled it, as a recorded refusal
/// of a `replace` of `/id` shows (`Invalid patch path /id`) — and it is used whichever way
/// the pointer cannot be applied: a member the resource does not have, a pointer that is
/// no JSON Pointer at all, one reaching into a single-valued member, or one naming a map
/// member without a key. One copy for all of them, so a caller reading the text sees the
/// same sentence for the same kind of mistake on every operation that words it this way.
pub fn invalid_patch_path(path: &str) -> String {
    format!("Invalid patch path {path}")
}

/// The `BadRequestException` message for a patch path refused as REQUEST INPUT: the path
/// exactly as the request spelled it, then API Gateway's fixed `Invalid request input`.
///
/// This is the wording of [`InvalidPathRefusal::PathThenInvalidInput`] — the sentence a
/// recorded refusal of a `replace` of the custom domain name's own `/domainName` shows
/// (`/domainName Invalid request input`). It carries no enumeration, because an operation
/// that words it this way publishes no patchable set in the refusal.
pub fn invalid_request_input(path: &str) -> String {
    format!("{path} Invalid request input")
}

/// The `BadRequestException` message for a patch path a resource reads as a PER-METHOD
/// SETTING path and cannot apply.
///
/// The variable part is the path exactly as the request spelled it; the rest — the fixed
/// opening and then the whole set of pointers the resource publishes, bracketed and
/// comma-separated — is the service's own, and is the same sentence whichever `op` named
/// the path. See [`InvalidPathRefusal::MethodSettingPath`].
pub fn invalid_method_setting_path(path: &str, paths: &[&str]) -> String {
    format!(
        "Invalid method setting path: {path}. Must be one of: [{}]",
        paths.join(", ")
    )
}

/// The `BadRequestException` message for a patch path that names a NESTED structure of a
/// resource, or reaches into one, where the structure rather than the resource owns the
/// refusal.
///
/// API Gateway words this one by the STRUCTURE: it names the member the pointer entered
/// and enumerates that member's full pointers, spelled from the resource root, and carries
/// no part of the rejected path at all — so the same sentence answers the structure named
/// as a whole, a sub-member it does not have, and a pointer reaching past one it does. The
/// spacing around the colon is the service's own, as is the bracketed, comma-separated
/// list. See [`InvalidPathRefusal::NestedStructure`].
pub fn invalid_nested_patch_path(member: &str, paths: &[&str]) -> String {
    format!(
        "Invalid {member} path. Must be one of : [{}]",
        paths.join(", ")
    )
}

/// The `BadRequestException` message for an `op` the targeted resource supports no path
/// for.
///
/// The ops MOST of these resources support are `add`, `replace` and `remove` — `move`,
/// `copy` and `test` name nothing on them — and a recorded AWS refusal of an op outside
/// that set names the three it does take, as quoted alternatives, and nothing about the
/// path that rode in on it. One copy for every resource that words it this way, so an op
/// outside that set reads the same whichever one it was sent to and whichever pointer it
/// carried; a resource whose recorded refusal words that sentence DIFFERENTLY (an API's
/// own, which brackets the same three ops, or a stage's, which takes a `copy`) passes its
/// own to [`parse_patches_refused_with_op`], and one that keys the refusal on the path
/// states no sentence at all ([`UnsupportedOpRefusal::Path`]).
pub const INVALID_PATCH_OPERATION: &str =
    "Invalid patch operation specified. Must be 'add'|'remove'|'replace'";

/// Parse a request's `patchOperations` into the edits to apply, in request order.
///
/// Called BEFORE anything is written, so a list this service cannot read leaves the
/// store untouched. The list is caller-sized, so it is bounded like every other
/// caller-sized collection — see [`bound_entries`].
pub fn parse_patches(
    operations: Option<&Vec<crate::ops::types::PatchOperation>>,
) -> Result<Vec<Patch>, PatchFailure> {
    parse_patches_refused_with(operations, InvalidPathRefusal::PathOnly)
}

/// As [`parse_patches`], for an operation that words a refusal of a patch path it cannot
/// apply its own way — see [`InvalidPathRefusal`].
pub fn parse_patches_refused_with(
    operations: Option<&Vec<crate::ops::types::PatchOperation>>,
    invalid_path: InvalidPathRefusal,
) -> Result<Vec<Patch>, PatchFailure> {
    parse_patches_refused_with_op(
        operations,
        invalid_path,
        UnsupportedOpRefusal::Ops(INVALID_PATCH_OPERATION),
    )
}

/// As [`parse_patches_refused_with`], for an operation that answers an `op` its pointer's
/// member cannot take its own way — see [`UnsupportedOpRefusal`].
pub fn parse_patches_refused_with_op(
    operations: Option<&Vec<crate::ops::types::PatchOperation>>,
    invalid_path: InvalidPathRefusal,
    unsupported_op: UnsupportedOpRefusal,
) -> Result<Vec<Patch>, PatchFailure> {
    let operations: &[crate::ops::types::PatchOperation] =
        operations.map(Vec::as_slice).unwrap_or_default();
    bound_entries("patchOperations", operations.len(), MAX_COLLECTION_ENTRIES)
        .map_err(PatchFailure::BadRequest)?;
    operations
        .iter()
        .map(|operation| Patch::parse(operation, invalid_path, unsupported_op))
        .collect()
}

/// The map key a patch names, refused when it is longer than a stored key may be.
///
/// `depth` is how many reference tokens the map member itself takes, so the key is what
/// the path carries beyond it. One copy for every handler that patches a map member, so
/// two of them cannot disagree about how a key is read or how long one may be.
pub fn bounded_map_key(patch: &Patch, depth: usize) -> Result<String, PatchFailure> {
    let key = patch.map_key(depth)?;
    bound_chars(&patch.path, &key, MAX_STRING_CHARS).map_err(PatchFailure::BadRequest)?;
    Ok(key)
}

/// Apply `patch` to a single-valued OPTIONAL string member.
///
/// `add` and `replace` set it to the op's value — `value` is "applicable for the add or
/// replace operation" (`PatchOperation$value`) — and an op that names no value to set
/// clears it ([`Patch::unsets_member`]), which an optional member can be. Any other op is
/// unsupported on such a member.
pub fn patch_optional_string(
    current: &mut Option<String>,
    patch: &Patch,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    if patch.unsets_member() {
        *current = None;
        return Ok(());
    }
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            bound_chars(&patch.path, value, max_chars).map_err(PatchFailure::BadRequest)?;
            *current = Some(value.to_string());
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a record's `description`: [`patch_optional_string`], with the value it
/// sets NORMALIZED the way this service stores a description
/// ([`crate::state::stored_description`]).
///
/// A patch that replaces a description with the empty string leaves the record with none,
/// exactly as a create that sets one does — the rule belongs to the MEMBER, so one helper
/// serves every operation that patches a `description`.
pub fn patch_description(
    current: &mut Option<String>,
    patch: &Patch,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    patch_optional_string(current, patch, max_chars)?;
    *current = crate::state::stored_description(current.take());
    Ok(())
}

/// Apply `patch` to a single-valued REQUIRED string member: as
/// [`patch_optional_string`], except that `remove` is refused — the member always has a
/// value — and an empty value is refused for the same reason.
pub fn patch_required_string(
    current: &mut String,
    patch: &Patch,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            if value.is_empty() {
                return Err(PatchFailure::BadRequest(format!(
                    "{} must not be empty",
                    patch.path
                )));
            }
            bound_chars(&patch.path, value, max_chars).map_err(PatchFailure::BadRequest)?;
            *current = value.to_string();
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a single-valued string member whose value set the model declares as
/// an enum: as [`patch_optional_string`], plus the enum's own values, because a patch
/// `value` is a bare string on the wire and so reaches the handler unvalidated.
///
/// The member is refused with the generic sentence this service has for a value no
/// declared set holds — the member, its values and the rejected one. A member whose
/// out-of-set value API Gateway answers with a sentence of its OWN passes that sentence
/// to [`patch_enum_string_refused_with`] instead.
pub fn patch_enum_string(
    current: &mut Option<String>,
    patch: &Patch,
    values: &[&str],
) -> Result<(), PatchFailure> {
    patch_enum_string_refused_with(current, patch, values, |value| {
        format!(
            "{} takes one of {}, got {value:?}",
            patch.path,
            values.join(", ")
        )
    })
}

/// Apply `patch` to an enum-typed member as [`patch_enum_string`] does, refusing a value
/// the declared set does not hold with the sentence `refusal` builds.
///
/// The op rules and the bound live here once, so a member with its own refusal wording
/// cannot drift from the members that take the generic one.
pub fn patch_enum_string_refused_with(
    current: &mut Option<String>,
    patch: &Patch,
    values: &[&str],
    refusal: impl FnOnce(&str) -> String,
) -> Result<(), PatchFailure> {
    // An op that names no value to set has none to check against the declared set: it
    // clears the member instead, which [`patch_optional_string`] below applies.
    if !patch.unsets_member() && matches!(patch.op, PatchOpKind::Add | PatchOpKind::Replace) {
        patch.require_single_valued()?;
        let value = patch.require_value()?;
        if !values.contains(&value) {
            return Err(PatchFailure::BadRequest(refusal(value)));
        }
    }
    patch_optional_string(current, patch, MAX_STRING_CHARS)
}

/// The `BadRequestException` message for a TLS security policy that is not one of the
/// values the modelled `SecurityPolicy` enum declares
/// (`smithy_query(query="shape", shape="SecurityPolicy")`).
///
/// API Gateway answers this condition with ONE sentence of entirely fixed text: it names
/// neither the rejected value, nor the member or patch path that carried it, nor the
/// policies it would have accepted, as a recorded refusal of a `/securityPolicy` replace
/// shows. One copy for every resource whose `securityPolicy` a caller can set, so a
/// client classifying this refusal against the operation's other bad requests reads the
/// same sentence whichever resource it was made on.
pub const INVALID_SECURITY_POLICY: &str = "Invalid security policy specified";

/// Apply `patch` to a `securityPolicy` member: as [`patch_enum_string`], except that a
/// value outside the member's declared set is refused with [`INVALID_SECURITY_POLICY`],
/// the service's own sentence for that condition.
pub fn patch_security_policy(
    current: &mut Option<String>,
    patch: &Patch,
) -> Result<(), PatchFailure> {
    patch_enum_string_refused_with(current, patch, SECURITY_POLICY_VALUES, |_| {
        INVALID_SECURITY_POLICY.to_string()
    })
}

/// The `BadRequestException` message for a content handling strategy that is not one of
/// the values the modelled `ContentHandlingStrategy` enum declares
/// (`smithy_query(query="shape", shape="ContentHandlingStrategy")`, which
/// [`CONTENT_HANDLING_STRATEGY_VALUES`] lists on the wire).
///
/// API Gateway answers this condition with ONE sentence that names the TYPE it rejected
/// the value for and the values it would have taken, comma-separated with no space: it
/// names neither the rejected value nor the member or patch path that carried it, as a
/// recorded refusal of a `/contentHandling` replace shows. The value set is read from the
/// model rather than written out, so the sentence cannot drift from the set the same patch
/// is checked against.
pub fn invalid_content_handling() -> String {
    format!(
        "Invalid ContentHandlingType specified. Valid options are {}",
        CONTENT_HANDLING_STRATEGY_VALUES.join(",")
    )
}

/// Apply `patch` to a `contentHandling` member: as [`patch_enum_string`], except that a
/// value outside the member's declared set is refused with [`invalid_content_handling`],
/// the service's own sentence for that condition.
///
/// One copy for every record whose `contentHandling` a patch can set — an integration and
/// an integration response carry the same member of the same type — so a client
/// classifying this refusal against the operation's other bad requests reads the same
/// sentence whichever record it was made on.
pub fn patch_content_handling(
    current: &mut Option<String>,
    patch: &Patch,
) -> Result<(), PatchFailure> {
    patch_enum_string_refused_with(current, patch, CONTENT_HANDLING_STRATEGY_VALUES, |_| {
        invalid_content_handling()
    })
}

/// Apply `patch` to a boolean member. `remove` is refused: a boolean member declared
/// with a default always has one of its two values.
pub fn patch_bool(current: &mut bool, patch: &Patch) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => *current = patch.require_bool()?,
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to an integer member EVERY value of which the service refuses with the
/// member's OWN sentence, `refusal`.
///
/// Where a setting publishes a sentence of its own, that sentence is the answer for every
/// value the setting cannot take: a text that is no integer is as far outside the member as
/// an integer the member will not have, so one `refusal` words both and the caller is told
/// the rule rather than which half of the read failed — and neither the pointer the value
/// arrived on nor the value itself appears in it. `accept` is the member's own rule, given
/// once by the member that owns it; `remove` is refused for the reason [`patch_bool`] gives.
pub fn patch_i32_refused_with(
    current: &mut i32,
    patch: &Patch,
    accept: impl Fn(i32) -> bool,
    refusal: impl FnOnce() -> String,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            *current = patch
                .require_value()?
                .trim()
                .parse()
                .ok()
                .filter(|parsed| accept(*parsed))
                .ok_or_else(|| PatchFailure::BadRequest(refusal()))?;
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a double member EVERY value of which earns the member's own sentence;
/// see [`patch_i32_refused_with`].
///
/// A double member of this service's records carries a finite number — a rate, a share of
/// traffic — so a text spelling an infinity or a not-a-number is outside the member as much
/// as a text that is no number at all, and all three earn `refusal`.
pub fn patch_f64_refused_with(
    current: &mut f64,
    patch: &Patch,
    accept: impl Fn(f64) -> bool,
    refusal: impl FnOnce() -> String,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            *current = patch
                .require_value()?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|parsed| parsed.is_finite() && accept(*parsed))
                .ok_or_else(|| PatchFailure::BadRequest(refusal()))?;
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to an integer member the SERVICE checks itself: `accept` is the member's
/// own checker — the one the create or put operation for that member runs — so the value
/// is refused with that member's own sentence whichever operation set it. `remove` is
/// refused for the reason [`patch_bool`] gives.
pub fn patch_i32_checked(
    current: &mut i32,
    patch: &Patch,
    accept: impl FnOnce(i32) -> Result<i32, String>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            *current = accept(patch.require_i32()?).map_err(PatchFailure::BadRequest)?;
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a nullable integer member the service checks itself; an op that names
/// no value to set clears it ([`Patch::unsets_member`]). See [`patch_i32_checked`] for
/// what `accept` is.
pub fn patch_optional_i32_checked(
    current: &mut Option<i32>,
    patch: &Patch,
    accept: impl FnOnce(i32) -> Result<i32, String>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    if patch.unsets_member() {
        *current = None;
        return Ok(());
    }
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            *current = Some(accept(patch.require_i32()?).map_err(PatchFailure::BadRequest)?);
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a double member the service checks itself; see
/// [`patch_i32_checked`].
pub fn patch_f64_checked(
    current: &mut f64,
    patch: &Patch,
    accept: impl FnOnce(f64) -> Result<f64, String>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            *current = accept(patch.require_f64()?).map_err(PatchFailure::BadRequest)?;
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a member that is a set of strings (`binaryMediaTypes`,
/// `authorizationScopes`, an API key's stage associations).
///
/// `add` appends `entry` when the collection does not hold it already, so adding twice
/// is the same as adding once; `remove` drops it. `replace` is unsupported: the path
/// names one entry, not the whole collection. The collection is caller-sized, so it is
/// bounded like every other.
pub fn patch_string_set(
    current: &mut Vec<String>,
    patch: &Patch,
    entry: &str,
    max_entries: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add => {
            if !current.iter().any(|held| held == entry) {
                bound_entries(&patch.path, current.len() + 1, max_entries)
                    .map_err(PatchFailure::BadRequest)?;
                current.push(entry.to_string());
            }
        }
        PatchOpKind::Remove => current.retain(|held| held != entry),
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a map member whose values are strings: `add` and `replace` set
/// `key` to the op's value, `remove` drops the key.
///
/// `max_chars` bounds one entry's VALUE, which differs by member — a mapping template is
/// far longer than a model name — so each caller passes the bound its own member carries,
/// the same one the create operation for that member enforces.
pub fn patch_string_map(
    current: &mut BTreeMap<String, String>,
    patch: &Patch,
    key: String,
    max_entries: usize,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            bound_chars(&patch.path, value, max_chars).map_err(PatchFailure::BadRequest)?;
            if !current.contains_key(&key) {
                bound_entries(&patch.path, current.len() + 1, max_entries)
                    .map_err(PatchFailure::BadRequest)?;
            }
            current.insert(key, value.to_string());
        }
        PatchOpKind::Remove => {
            current.remove(&key);
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a map member whose values are booleans; see [`patch_string_map`].
pub fn patch_bool_map(
    current: &mut BTreeMap<String, bool>,
    patch: &Patch,
    key: String,
    max_entries: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_bool()?;
            if !current.contains_key(&key) {
                bound_entries(&patch.path, current.len() + 1, max_entries)
                    .map_err(PatchFailure::BadRequest)?;
            }
            current.insert(key, value);
        }
        PatchOpKind::Remove => {
            current.remove(&key);
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}
