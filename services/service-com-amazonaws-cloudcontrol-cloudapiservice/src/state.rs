// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudcontrol-cloudapiservice service.
//!
//! Cloud Control API is a uniform control plane over resources of a named type: a
//! request names a `TypeName` and the resource's primary identifier, and every answer
//! is rendered from one of two records this store holds.
//!
//! * A [`StoredResource`] is a provisioned resource — the properties a `GetResource` or
//!   `ListResources` reports, and the properties an `UpdateResource`'s patch is applied
//!   to. It is keyed by the pair that identifies it, `(TypeName, Identifier)`, so two
//!   types may hold the same identifier and one type's listing never sees another's.
//! * A [`RequestRecord`] is one resource operation request — the `ProgressEvent` the
//!   mutating operations answer with and `GetResourceRequestStatus` resolves a
//!   `RequestToken` to. A request is answered while the operation is still running, so
//!   the record also carries the work the operation still owes
//!   ([`ResourceEffect`]) and the instant that work falls due: it is applied by
//!   whichever call arrives at or after that instant, and nothing waits for it.
//!
//! Every read goes through [`ComAmazonawsCloudcontrolCloudapiserviceState::materialise_due`]
//! first, so no two operations can disagree about which requests have completed or which
//! resources exist.
//!
//! A [`StoredResource`] is NOT where every resource of every type lives. Cloud Control API
//! is a front end over the resource type's own provider, so a type whose resources live in
//! another service's store of this emulator is resolved and acted on THERE: that is
//! [`native`], and every resolution in this module goes through it. This store holds the
//! resources of the types no provider stands behind, which is the only place such a
//! resource can live.

pub mod native;
mod primary_identifiers;
mod request_listing;

pub use primary_identifiers::handler_rendered_identifier;
pub use request_listing::{
    decode_request_page_token, request_page_token, RequestFilter, RequestPosition,
    INVALID_NEXT_TOKEN,
};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

use native::{NativeRefusal, NativeResources};

/// The secret this service's `NextToken`s are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED.
pub use ::service_common::page_tokens::PageTokenKey;

// ---------------------------------------------------------------------------
// The wire vocabulary
// ---------------------------------------------------------------------------

/// The `Operation` a create request reports (`Operation` enum member).
pub const OPERATION_CREATE: &str = "CREATE";
/// The `Operation` an update request reports (`Operation` enum member).
pub const OPERATION_UPDATE: &str = "UPDATE";
/// The `Operation` a delete request reports (`Operation` enum member).
pub const OPERATION_DELETE: &str = "DELETE";

/// "`IN_PROGRESS`: The resource operation is currently in progress" — the status every
/// resource operation request is answered in, because the operation it starts is still
/// running when the call returns.
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// "`SUCCESS`: The resource operation has successfully completed" — the status a request
/// reaches once the work it owes has been applied.
pub const STATUS_SUCCESS: &str = "SUCCESS";

/// "`FAILED`: The resource operation has failed. Refer to the error code and status
/// message for more information."
pub const STATUS_FAILED: &str = "FAILED";

/// "`CANCEL_IN_PROGRESS`: The resource operation is in the process of being canceled" —
/// the status a canceled request is answered in, because the cancellation it starts is
/// still running when the call returns.
pub const STATUS_CANCEL_IN_PROGRESS: &str = "CANCEL_IN_PROGRESS";

/// "`CANCEL_COMPLETE`: The resource operation has been canceled" — the status a canceled
/// request reaches once the cancellation has run.
pub const STATUS_CANCEL_COMPLETE: &str = "CANCEL_COMPLETE";

/// "`PENDING`: The resource operation hasn't yet started" — a status of the model's own
/// `OperationStatus` enum, and with [`STATUS_IN_PROGRESS`] one of the two a request can be
/// canceled in ([`CANCELABLE_STATUSES`]).
pub const STATUS_PENDING: &str = "PENDING";

/// The statuses a resource operation request can be CANCELED in.
///
/// "Only resource operations requests with a status of PENDING or IN_PROGRESS can be
/// canceled" (`CancelResourceRequest`): the cancelable set is those two and no other, which
/// is narrower than "not over" — a request already being canceled is still running and is
/// not cancelable again, which is what keeps this `@idempotent` operation from moving the
/// instant that cancellation completes.
pub const CANCELABLE_STATUSES: [&str; 2] = [STATUS_PENDING, STATUS_IN_PROGRESS];

/// Whether a request in `status` can be canceled — see [`CANCELABLE_STATUSES`].
pub fn is_cancelable_status(status: &str) -> bool {
    CANCELABLE_STATUSES.contains(&status)
}

/// How long after the call that starts it a resource operation completes.
///
/// A resource operation request is returned in a non-terminal status, so the clock owes
/// it the rest of the progression; the span is this emulator's and is deliberately not
/// zero, so a caller that polls straight away still observes the `IN_PROGRESS` status
/// and the states that go with it.
pub const RESOURCE_OPERATION_SPAN: Duration = Duration::from_secs(3);

/// How long a `ClientToken` keeps naming the request it first carried.
///
/// `CreateResource$ClientToken` states the rule: "A client token is valid for 36 hours
/// once used. After that, a resource request with the same client token is treated as a
/// new request."
pub const CLIENT_TOKEN_LIFETIME: Duration = Duration::from_secs(36 * 60 * 60);

/// The most characters an identifier may carry — the maximum length the model gives the
/// `Identifier` shape, so a value longer than this could not be reported back at all.
pub const MAX_IDENTIFIER_CHARACTERS: usize = 1024;

/// The most characters a rendered resource model may carry — the maximum length the
/// model gives the `Properties` shape, which both `ResourceDescription$Properties` and
/// `ProgressEvent$ResourceModel` target.
pub const MAX_PROPERTIES_CHARACTERS: usize = 262_144;

/// The largest page a paginated read answers — the maximum the model gives
/// `ListResources$MaxResults` (`@range(min: 1, max: 100)`), which is therefore also the
/// page a read that names no `MaxResults` answers.
pub const MAX_PAGE_SIZE: usize = 100;

/// How many resources this store holds before a create is refused.
///
/// A create is caller-driven growth, so the store is bounded; the bound is reported as
/// the service-side resource limit the model declares for exactly this.
pub const MAX_RESOURCES: usize = 10_000;

/// How many resource operation requests this store keeps.
///
/// At the bound the oldest COMPLETED record leaves to make room — a record still owing
/// work is never evicted, or the work it owes would never be applied.
pub const MAX_REQUESTS: usize = 1_000;

/// How many `ClientToken` records this store keeps, so a caller cannot grow the store by
/// inventing tokens. At the bound the oldest leaves, which only costs that token its
/// idempotent replay.
pub const MAX_CLIENT_TOKENS: usize = 1_000;

/// How many candidates [`ComAmazonawsCloudcontrolCloudapiserviceState::allocate_request_token`]
/// draws before giving up, so the allocation cannot spin.
const TOKEN_ALLOCATION_ATTEMPTS: usize = 8;

/// What a handler says when this service's store is not mounted on the engine it was
/// built from. Reported as the modelled server-side failure, never as a fresh store.
pub const STORE_NOT_MOUNTED: &str =
    "the Cloud Control API resource store is not mounted on this engine";

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// The pair that identifies a provisioned resource: its type, then its primary
/// identifier. Ordered type-first, so one type's resources are contiguous and a listing
/// of that type is a range scan in identifier order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceKey {
    /// The `TypeName` the resource was created under.
    pub type_name: String,
    /// The resource's primary identifier.
    pub identifier: String,
}

impl ResourceKey {
    /// The key of `identifier` under `type_name`.
    pub fn new(type_name: &str, identifier: &str) -> Self {
        Self {
            type_name: type_name.to_string(),
            identifier: identifier.to_string(),
        }
    }
}

/// One provisioned resource: the properties the reads report, and the property its
/// primary identifier is carried by.
#[derive(Debug, Clone)]
pub struct StoredResource {
    /// The `TypeName` this resource belongs to.
    pub type_name: String,
    /// The primary identifier, which is also the value of [`Self::identifier_property`]
    /// inside [`Self::properties`].
    pub identifier: String,
    /// The property of the resource model that carries the primary identifier.
    pub identifier_property: String,
    /// The current resource model: a JSON object of each property and its value.
    pub properties: Value,
    /// The `TypeVersionId` the create named, when it named one.
    pub type_version_id: Option<String>,
    /// When the create that provisioned this resource completed.
    pub created_at: SystemTime,
    /// When the last operation that changed this resource completed.
    pub updated_at: SystemTime,
}

/// One resource as it exists NOW, whichever store holds it: what a read reports and what a
/// patch is applied to.
///
/// A resource of a type [`native`] stands behind is rendered from the OWNING service's
/// record, and one of any other type from this store's [`StoredResource`] — so no caller
/// of this module has to know which of the two a resource lives in.
#[derive(Debug, Clone)]
pub struct ResolvedResource {
    /// The `TypeName` the resource belongs to.
    pub type_name: String,
    /// Its primary identifier.
    pub identifier: String,
    /// The property of the resource model that carries that identifier.
    pub identifier_property: String,
    /// The current resource model: a JSON object of each property and its value.
    pub properties: Value,
}

impl From<&StoredResource> for ResolvedResource {
    fn from(stored: &StoredResource) -> Self {
        Self {
            type_name: stored.type_name.clone(),
            identifier: stored.identifier.clone(),
            identifier_property: stored.identifier_property.clone(),
            properties: stored.properties.clone(),
        }
    }
}

/// The work a resource operation request still owes the store.
#[derive(Debug, Clone)]
pub enum ResourceEffect {
    /// Provision the resource with these properties.
    Create {
        /// The resource model to store, identifier property included.
        properties: Value,
        /// The property that model carries the primary identifier in.
        identifier_property: String,
        /// The `TypeVersionId` the create named, when it named one.
        type_version_id: Option<String>,
        /// The Region the call that accepted the create was signed for, which is the
        /// Region a resource the OWNING service provisions is created in.
        region: String,
    },
    /// Replace the resource's properties with the patched model.
    Update {
        /// The resource model the patch produced.
        properties: Value,
        /// The Region the call that accepted the update was signed for, which is the
        /// Region the resource is patched in.
        region: String,
    },
    /// Remove the resource from the store.
    Delete {
        /// The Region the call that accepted the delete was signed for, which is the
        /// Region the resource is removed from.
        region: String,
    },
}

/// The work a request owes, and the instant it falls due.
#[derive(Debug, Clone)]
struct PendingWork {
    /// When the work becomes due. Stored, never waited for.
    due: SystemTime,
    /// What is applied then — `None` for a request a cancel abandoned the work of, which
    /// has nothing left to apply and only its status to reach.
    effect: Option<ResourceEffect>,
}

/// One resource operation request, as `ProgressEvent` reports it.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// The token the client was handed, and resolves this record by.
    pub request_token: String,
    /// The `TypeName` the request named.
    pub type_name: String,
    /// The primary identifier the request acts on.
    pub identifier: Option<String>,
    /// `CREATE`, `UPDATE` or `DELETE`.
    pub operation: String,
    /// Where the request has got to.
    pub operation_status: String,
    /// "When the resource operation request was initiated" — set once, at the call that
    /// started it, and never moved.
    pub event_time: SystemTime,
    /// The rendered resource model the request left behind, once it completed.
    pub resource_model: Option<String>,
    /// Any message explaining the current status.
    pub status_message: Option<String>,
    /// For a `FAILED` request, the handler error code.
    pub error_code: Option<String>,
    /// "When to next request the status of this resource operation request" — the
    /// instant the work below falls due, while there is work left.
    pub retry_after: Option<SystemTime>,
    /// The work still owed, or `None` once it has been applied.
    pending: Option<PendingWork>,
}

impl RequestRecord {
    /// A request that has just been accepted: `IN_PROGRESS`, owing `effect` one
    /// [`RESOURCE_OPERATION_SPAN`] from `now`.
    pub fn accepted(
        request_token: String,
        type_name: String,
        identifier: Option<String>,
        operation: &str,
        now: SystemTime,
        effect: ResourceEffect,
    ) -> Self {
        let due = now + RESOURCE_OPERATION_SPAN;
        Self {
            request_token,
            type_name,
            identifier,
            operation: operation.to_string(),
            operation_status: STATUS_IN_PROGRESS.to_string(),
            event_time: now,
            // The resource model the request leaves behind is what the store holds once
            // the work has been applied, so there is none to report yet.
            resource_model: None,
            status_message: None,
            error_code: None,
            retry_after: Some(due),
            pending: Some(PendingWork {
                due,
                effect: Some(effect),
            }),
        }
    }

    /// Whether this request still owes the store work.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// What one `ClientToken` has been used for.
#[derive(Debug, Clone)]
pub struct ClientTokenEntry {
    /// The request this token carried.
    pub request_token: String,
    /// A digest of that request's caller-owned members ([`request_fingerprint`]), so a
    /// reuse with different parameters is told from a retry.
    pub request_fingerprint: String,
    /// When the token was first used, which is what [`CLIENT_TOKEN_LIFETIME`] runs from.
    pub used_at: SystemTime,
}

/// What this store says about a `ClientToken` a mutating operation was handed.
#[derive(Debug, PartialEq, Eq)]
pub enum ClientTokenVerdict {
    /// This store has never served this token (or the request carries none), so the
    /// operation goes ahead.
    Fresh,
    /// This token already carried THIS request: the named request is replayed rather
    /// than a second one started.
    Replay(String),
    /// This token already carried a DIFFERENT request: the named request is the one it
    /// was first used for, which is what the conflict reports back.
    Conflict(String),
}

/// Why a request could not be recorded.
#[derive(Debug)]
pub struct RequestStoreFull;

/// What a cancel found when it resolved the `RequestToken` it was handed.
#[derive(Debug)]
pub enum CancelVerdict {
    /// This store holds no request under that token.
    NotFound,
    /// The request is in a status no cancel acts on — one outside
    /// [`CANCELABLE_STATUSES`]: carries that status, so the refusal can state it.
    NotCancelable(String),
    /// The request was canceled: the work it owed is abandoned and it is now
    /// [`STATUS_CANCEL_IN_PROGRESS`]. Carries the record as it stands, which is the
    /// `ProgressEvent` the cancel answers with — boxed, so the verdict stays the size of
    /// its smallest variant.
    Canceling(Box<RequestRecord>),
}

/// Why a request's owed work could not be applied when it fell due.
///
/// A request is accepted against the store as it was at the call and its work applied
/// when the clock makes it due; this is what a record reports when the store has moved in
/// a way that leaves that work nothing to do.
#[derive(Debug, PartialEq, Eq)]
enum EffectFailure {
    /// The resource the request was accepted against is no longer in the store.
    ResourceGone,
    /// The resource a create was to provision is already in the store.
    ResourceExists,
    /// The service that OWNS the resource type refused the work, in its own terms: the
    /// `HandlerErrorCode` it failed with and its own diagnostic.
    Owning {
        /// The handler error code the owning service's refusal is reported under.
        code: &'static str,
        /// That service's own diagnostic.
        detail: String,
    },
}

impl From<NativeRefusal> for EffectFailure {
    /// The owning service's refusal, as this store reports it: the two verdicts every type
    /// shares are reported in Cloud Control API's own generic terms, and anything the
    /// owning service states in its own terms carries that statement.
    fn from(refusal: NativeRefusal) -> Self {
        match &refusal {
            NativeRefusal::NotFound => Self::ResourceGone,
            NativeRefusal::AlreadyExists => Self::ResourceExists,
            NativeRefusal::AtCapacity(detail)
            | NativeRefusal::Invalid(detail)
            | NativeRefusal::Rejected(detail) => Self::Owning {
                code: refusal.handler_error_code(),
                detail: detail.clone(),
            },
        }
    }
}

impl EffectFailure {
    /// The `HandlerErrorCode` the record reports (`ProgressEvent$ErrorCode`), which is
    /// the owning resource type's handler verdict: a resource the handler could not find
    /// is `NotFound`, and one that is already there is `AlreadyExists`.
    fn error_code(&self) -> &'static str {
        match self {
            Self::ResourceGone => HANDLER_ERROR_NOT_FOUND,
            Self::ResourceExists => HANDLER_ERROR_ALREADY_EXISTS,
            Self::Owning { code, .. } => code,
        }
    }

    /// The `StatusMessage` the record reports: the owning handler's own diagnostic for
    /// the resource the work was owed on. The handler error code is reported beside it in
    /// `ProgressEvent$ErrorCode`, so the message carries the detail alone.
    fn status_message(&self, record: &RequestRecord) -> String {
        let identifier = record.identifier.as_deref().unwrap_or_default();
        match self {
            Self::ResourceGone => handler_not_found_detail(&record.type_name, identifier),
            Self::ResourceExists => handler_already_exists_detail(&record.type_name, identifier),
            Self::Owning { detail, .. } => detail.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-cloudcontrol-cloudapiservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsCloudcontrolCloudapiserviceState {
    /// Provisioned resources, by type then primary identifier.
    resources: BTreeMap<ResourceKey, StoredResource>,
    /// Every resource operation request, by the token handed to the client.
    requests: BTreeMap<String, RequestRecord>,
    /// Those tokens in the order the requests were accepted, so the oldest completed
    /// record is the one that leaves at [`MAX_REQUESTS`].
    request_order: VecDeque<String>,
    /// `ClientToken` -> what it has been used for.
    client_tokens: BTreeMap<String, ClientTokenEntry>,
    /// Those client tokens in first-use order, so the oldest leaves at
    /// [`MAX_CLIENT_TOKENS`].
    client_token_order: VecDeque<String>,
    /// The secret this store's `NextToken`s are authenticated under. PRIVATE, and
    /// reached only through [`Self::page_token_key`]: a key a caller could read or
    /// replace would no longer tell a token this service minted from one it did not.
    page_token_key: PageTokenKey,
}

impl Default for ComAmazonawsCloudcontrolCloudapiserviceState {
    fn default() -> Self {
        Self {
            // Nothing exists until a create provisions it.
            resources: BTreeMap::new(),
            requests: BTreeMap::new(),
            request_order: VecDeque::new(),
            client_tokens: BTreeMap::new(),
            client_token_order: VecDeque::new(),
            // One key per store, from the system's randomness: see `PageTokenKey`.
            page_token_key: PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsCloudcontrolCloudapiserviceState {
    /// Apply everything the CLOCK already owes this store at `now`.
    ///
    /// Two clock-driven changes, neither of which depends on which call arrives first:
    /// every request whose work has fallen due settles — the work is applied and the
    /// status moves to `SUCCESS`, or the owning handler would not do it and the status
    /// moves to `FAILED` carrying that verdict, or a cancel had abandoned the work and the
    /// status moves to `CANCEL_COMPLETE` — and every `ClientToken` past
    /// [`CLIENT_TOKEN_LIFETIME`] is forgotten. Both were owed at `now` whether or not
    /// anybody called, which is why a reporting operation may make them.
    ///
    /// Every operation of this service calls this before it reads anything, so no two of
    /// them can disagree about which requests have completed or which resources exist.
    pub fn materialise_due(&mut self, now: SystemTime, native: &NativeResources) {
        let due: Vec<String> = self
            .requests
            .iter()
            .filter(|(_, record)| {
                record
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.due <= now)
            })
            .map(|(token, _)| token.clone())
            .collect();
        for token in due {
            // Taken out of the map so the effect below can write the resource store
            // while the record is being settled, and put back once it is.
            let Some(mut record) = self.requests.remove(&token) else {
                continue;
            };
            if let Some(pending) = record.pending.take() {
                match pending.effect {
                    Some(effect) => match self.apply(&record, effect, now, native) {
                        Ok(resource_model) => {
                            record.resource_model = resource_model;
                            record.operation_status = STATUS_SUCCESS.to_string();
                        }
                        Err(failure) => {
                            // The work could not be applied, so the record reports what
                            // actually happened rather than a success nothing performed.
                            record.operation_status = STATUS_FAILED.to_string();
                            record.error_code = Some(failure.error_code().to_string());
                            record.status_message = Some(failure.status_message(&record));
                        }
                    },
                    // A cancel abandoned the work this request owed, so there is nothing
                    // to apply and nothing to report a resource model of: what falls due
                    // is the cancellation itself, which is now complete.
                    None => record.operation_status = STATUS_CANCEL_COMPLETE.to_string(),
                }
            }
            // Nothing is left to poll for.
            record.retry_after = None;
            self.requests.insert(token, record);
        }
        self.forget_expired_client_tokens(now);
    }

    /// Apply one request's owed work, answering the resource model the request leaves
    /// behind (none, for a delete: there is no resource left to report) — or why the work
    /// could not be applied.
    fn apply(
        &mut self,
        record: &RequestRecord,
        effect: ResourceEffect,
        now: SystemTime,
        native: &NativeResources,
    ) -> Result<Option<String>, EffectFailure> {
        let identifier = record.identifier.clone().unwrap_or_default();
        // A type another service owns the resources of is acted on THERE, so the work this
        // record owes is applied to that service's store and the model it leaves behind is
        // the one that service then holds.
        if let Some(provider) = native.provider(&record.type_name) {
            return match effect {
                ResourceEffect::Create {
                    properties, region, ..
                } => provider
                    .create(&region, &identifier, &properties, now)
                    .map(|model| Some(rendered_properties(&model)))
                    .map_err(EffectFailure::from),
                ResourceEffect::Update { properties, region } => provider
                    .update(&region, &identifier, &properties, now)
                    .map(|model| Some(rendered_properties(&model)))
                    .map_err(EffectFailure::from),
                ResourceEffect::Delete { region } => provider
                    .delete(&region, &identifier, now)
                    .map(|()| None)
                    .map_err(EffectFailure::from),
            };
        }
        let key = ResourceKey::new(&record.type_name, &identifier);
        match effect {
            ResourceEffect::Create {
                properties,
                identifier_property,
                type_version_id,
                region: _,
            } => {
                // The create was accepted against a store that held no such resource;
                // if one is there now, the owning handler is the one that finds it, so
                // the request FAILS with that handler's verdict rather than overwriting
                // a resource this request never named.
                if self.resources.contains_key(&key) {
                    return Err(EffectFailure::ResourceExists);
                }
                let model = rendered_properties(&properties);
                self.resources.insert(
                    key,
                    StoredResource {
                        type_name: record.type_name.clone(),
                        identifier,
                        identifier_property,
                        properties,
                        type_version_id,
                        created_at: now,
                        updated_at: now,
                    },
                );
                Ok(Some(model))
            }
            ResourceEffect::Update { properties, .. } => {
                // The resource this update was accepted against is gone, so there is
                // nothing to patch: the request FAILED, and reporting it as anything else
                // would claim a write that was discarded.
                let stored = self
                    .resources
                    .get_mut(&key)
                    .ok_or(EffectFailure::ResourceGone)?;
                stored.properties = properties;
                stored.updated_at = now;
                Ok(Some(rendered_properties(&stored.properties)))
            }
            ResourceEffect::Delete { .. } => {
                // A delete is accepted before the resource is looked for — the existence
                // of the resource is the owning HANDLER's verdict, which is reported when
                // the work falls due — so a delete that finds nothing to remove FAILS
                // with that handler's not-found rather than reporting a removal nothing
                // performed.
                self.resources
                    .remove(&key)
                    .ok_or(EffectFailure::ResourceGone)?;
                Ok(None)
            }
        }
    }

    /// Forget every `ClientToken` whose [`CLIENT_TOKEN_LIFETIME`] has elapsed, so a
    /// request carrying it again is treated as a new request.
    fn forget_expired_client_tokens(&mut self, now: SystemTime) {
        self.client_tokens
            .retain(|_, entry| entry.used_at + CLIENT_TOKEN_LIFETIME > now);
        self.client_token_order
            .retain(|token| self.client_tokens.contains_key(token));
    }

    /// The resource `identifier` of `type_name` as it exists NOW, or `None` when no store
    /// holds it — a resource of ANOTHER type never answers here, which is what makes the
    /// same identifier under two types two resources.
    ///
    /// Where a provider stands behind the type, the OWNING service's state is what
    /// answers: a resource that service holds is resolved whichever API created it, and
    /// one it no longer holds resolves to nothing even if this service created it. Such a
    /// resource is resolved in `region` — the Region the call was signed for — because that
    /// is the account and Region this service reports a resource of.
    pub fn resolve(
        &self,
        native: &NativeResources,
        region: &str,
        type_name: &str,
        identifier: &str,
    ) -> Option<ResolvedResource> {
        if let Some(provider) = native.provider(type_name) {
            let identifier_property = provider.identifier_property();
            return provider.read(region, identifier).map(|properties| {
                // A resource is reported under the primary identifier it HOLDS, read off
                // the model's own identifier property, rather than under the reference the
                // request addressed it by: a type whose owning service ASSIGNS the
                // identifier resolves more than one reference to one resource, and every
                // answer about it names the one value it answers to.
                let own = properties
                    .get(identifier_property)
                    .and_then(Value::as_str)
                    .unwrap_or(identifier)
                    .to_string();
                ResolvedResource {
                    type_name: type_name.to_string(),
                    identifier: own,
                    identifier_property: identifier_property.to_string(),
                    properties,
                }
            });
        }
        self.resources
            .get(&ResourceKey::new(type_name, identifier))
            .map(ResolvedResource::from)
    }

    /// The resources of `type_name` that exist NOW, in identifier order: at most `limit`
    /// of them, and only those whose identifier sorts AFTER `after`.
    ///
    /// Bounded at the inventory rather than after it, so the work one listing drives is
    /// the window it is about to report and not the size of the inventory behind it: the
    /// owning service's own ordered read where a provider stands behind the type, and
    /// otherwise a range scan of this store bounded by the type, whose keys are
    /// contiguous, so it reads exactly that type's entries and stops at the first key
    /// belonging to another.
    pub fn resolve_type(
        &self,
        native: &NativeResources,
        region: &str,
        type_name: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Vec<ResolvedResource> {
        if let Some(provider) = native.provider(type_name) {
            return provider
                .list(region, after, limit)
                .into_iter()
                .map(|(identifier, properties)| ResolvedResource {
                    type_name: type_name.to_string(),
                    identifier,
                    identifier_property: provider.identifier_property().to_string(),
                    properties,
                })
                .collect();
        }
        // `Identifier` is at least one character long, so the empty identifier sorts
        // before every real one and excluding it excludes nothing.
        let start = ResourceKey::new(type_name, after.unwrap_or_default());
        self.resources
            .range((Bound::Excluded(start), Bound::Unbounded))
            .take_while(move |(key, _)| key.type_name == type_name)
            .take(limit)
            .map(|(_, stored)| ResolvedResource::from(stored))
            .collect()
    }

    /// How many resources this store holds — what [`MAX_RESOURCES`] bounds.
    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    /// The request still running against `identifier` of `type_name`, which is what
    /// makes a second operation on the same resource the modelled concurrent-operation
    /// refusal — and whose `RequestToken` is what that refusal names, so the caller can
    /// poll the operation that holds the resource.
    ///
    /// A request is recorded under the identifier the resource ANSWERS TO
    /// ([`Self::resolve`]), which for a type whose owning service assigns that identifier
    /// is not the only reference a caller can address the resource by — so a caller that
    /// addressed it by another of its references asks this under both
    /// ([`Self::in_flight_request_on_either`]).
    pub fn in_flight_request(&self, type_name: &str, identifier: &str) -> Option<&str> {
        self.requests
            .values()
            .find(|record| {
                record.is_pending()
                    && record.type_name == type_name
                    && record.identifier.as_deref() == Some(identifier)
            })
            .map(|record| record.request_token.as_str())
    }

    /// The request still running against the resource a caller addressed as `reference`
    /// and which itself answers to `identifier`, with the identifier the guard matched it
    /// under.
    ///
    /// The guard that admits ONE operation at a time has to be asked under both: a type
    /// whose owning service ASSIGNS the primary identifier resolves more than one
    /// reference to one resource, the running operation was recorded under the one
    /// identifier that resource answers to, and a second operation naming another of its
    /// references would otherwise not see it at all.
    pub fn in_flight_request_on_either<'a>(
        &'a self,
        type_name: &str,
        reference: &'a str,
        identifier: &'a str,
    ) -> Option<(&'a str, &'a str)> {
        if let Some(holding) = self.in_flight_request(type_name, reference) {
            return Some((reference, holding));
        }
        self.in_flight_request(type_name, identifier)
            .map(|holding| (identifier, holding))
    }

    /// The request `request_token` names, or `None` when this store holds none.
    pub fn request(&self, request_token: &str) -> Option<&RequestRecord> {
        self.requests.get(request_token)
    }

    /// Cancel the request `request_token` names, as of `now`.
    ///
    /// "Only resource operations requests with a status of PENDING or IN_PROGRESS can be
    /// canceled" (`CancelResourceRequest`), so a request in any other status — one already
    /// over, and one already being canceled — is reported back in that status rather than
    /// moved, and a token this store holds no request under is reported as holding none.
    /// That is what makes a repeated cancel leave the request exactly where the first one
    /// put it, as an `@idempotent` operation must.
    ///
    /// A canceled request ABANDONS the work it owed — the resource it was to provision,
    /// patch or remove is left exactly as it is — and is answered while the cancellation
    /// is still running, so it reaches [`STATUS_CANCEL_COMPLETE`] one
    /// [`RESOURCE_OPERATION_SPAN`] later, the way every other resource operation reaches
    /// its own terminal status.
    ///
    /// Call [`Self::materialise_due`] first: a request whose work has fallen due is over,
    /// and a cancel must see the status it has actually reached.
    pub fn cancel_request(&mut self, request_token: &str, now: SystemTime) -> CancelVerdict {
        let Some(record) = self.requests.get_mut(request_token) else {
            return CancelVerdict::NotFound;
        };
        if !is_cancelable_status(&record.operation_status) {
            return CancelVerdict::NotCancelable(record.operation_status.clone());
        }
        let due = now + RESOURCE_OPERATION_SPAN;
        record.operation_status = STATUS_CANCEL_IN_PROGRESS.to_string();
        // The work is dropped rather than applied: nothing the request was accepted for
        // happens, which is what cancelling it means. What is still owed is the
        // cancellation reaching its own terminal status.
        record.pending = Some(PendingWork { due, effect: None });
        record.retry_after = Some(due);
        CancelVerdict::Canceling(Box::new(record.clone()))
    }

    /// A `RequestToken` this store holds no request under.
    ///
    /// Answers `None` when [`TOKEN_ALLOCATION_ATTEMPTS`] candidates were all taken, so
    /// the allocation cannot spin.
    pub fn allocate_request_token(&self) -> Option<String> {
        (0..TOKEN_ALLOCATION_ATTEMPTS)
            .map(|_| mint_token())
            .find(|candidate| !self.requests.contains_key(candidate))
    }

    /// Record `record`, evicting the oldest COMPLETED request when the store is at
    /// [`MAX_REQUESTS`].
    ///
    /// A record that still owes work is never evicted, or that work would never be
    /// applied; a store whose every record is still running therefore has no room, which
    /// is the throttling answer rather than a silently dropped operation.
    pub fn record_request(&mut self, record: RequestRecord) -> Result<(), RequestStoreFull> {
        while self.request_order.len() >= MAX_REQUESTS {
            let evictable = self.request_order.iter().position(|token| {
                self.requests
                    .get(token)
                    .is_none_or(|held| !held.is_pending())
            });
            let Some(position) = evictable else {
                return Err(RequestStoreFull);
            };
            if let Some(evicted) = self.request_order.remove(position) {
                self.requests.remove(&evicted);
            }
        }
        self.request_order.push_back(record.request_token.clone());
        self.requests.insert(record.request_token.clone(), record);
        Ok(())
    }

    /// What this store says about `client_token` for a request whose caller-owned
    /// members digest to `fingerprint`.
    ///
    /// The one rule all three mutating operations apply: a token that already carried
    /// the same request replays THAT request instead of starting a second one, and a
    /// token reused for a different request is refused rather than answered with an
    /// unrelated record. A token whose request record has since been evicted names
    /// nothing to replay, so it is treated as a new request.
    pub fn judge_client_token(
        &self,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> ClientTokenVerdict {
        let Some(token) = client_token else {
            return ClientTokenVerdict::Fresh;
        };
        let Some(entry) = self.client_tokens.get(token) else {
            return ClientTokenVerdict::Fresh;
        };
        if entry.request_fingerprint != fingerprint {
            return ClientTokenVerdict::Conflict(entry.request_token.clone());
        }
        if self.requests.contains_key(&entry.request_token) {
            return ClientTokenVerdict::Replay(entry.request_token.clone());
        }
        ClientTokenVerdict::Fresh
    }

    /// Record that `client_token` carried the request `request_token`, so a retry
    /// carrying it again replays that request.
    pub fn remember_client_token(
        &mut self,
        client_token: Option<&str>,
        fingerprint: String,
        request_token: &str,
        now: SystemTime,
    ) {
        let Some(token) = client_token else {
            return;
        };
        while self.client_token_order.len() >= MAX_CLIENT_TOKENS {
            let Some(oldest) = self.client_token_order.pop_front() else {
                break;
            };
            self.client_tokens.remove(&oldest);
        }
        if self
            .client_tokens
            .insert(
                token.to_string(),
                ClientTokenEntry {
                    request_token: request_token.to_string(),
                    request_fingerprint: fingerprint,
                    used_at: now,
                },
            )
            .is_none()
        {
            self.client_token_order.push_back(token.to_string());
        }
    }

    /// The key this store's paginated read mints and recognises its `NextToken`s under.
    /// Copied out rather than borrowed, so a read can check the token it was handed and
    /// go on to borrow the resources it is about to report in the same call.
    pub(crate) fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudcontrolCloudapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudcontrolCloudapiserviceState>)
}

// ---------------------------------------------------------------------------
// The audit record a state-changing call leaves
// ---------------------------------------------------------------------------

/// The region a request is signed for, read from its SigV4 credential scope.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope. The
/// scope is caller-controlled text, so anything that is not an AWS region token is
/// refused here and the default used instead: a record never carries arbitrary text a
/// caller chose.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_CHARACTERS
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The most characters a credential scope's region may carry to be read as one:
/// comfortably above every published AWS region name.
pub const MAX_REGION_CHARACTERS: usize = 64;

/// The region an unsigned request is taken to have addressed — the region the emulator's
/// generated client configuration uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// How a record names the resource a call acted on: the pair that identifies it.
pub fn resource_label(type_name: &str, identifier: &str) -> String {
    format!("{type_name}/{identifier}")
}

/// Emit the structured audit record a state-changing call owes: the operation, the
/// resource it acts on, the request it started, how the call ended, and which credential
/// was presented for it.
///
/// Only IDENTIFIERS are recorded. The resource model and the patch document a request
/// carries are values the caller may hold secrets in — the model marks both members
/// sensitive — so neither appears here; nor does any header beyond the access key id
/// that names the caller. The subscriber stamps the time.
pub fn audit(
    parts: &::http::request::Parts,
    operation: &str,
    resource: &str,
    request_token: &str,
    outcome: &str,
) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        request_token,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "Cloud Control API resource operation"
    );
}

/// The outcome a record carries for a call that started a resource operation.
pub const OUTCOME_ACCEPTED: &str = "accepted";

/// The outcome a record carries for a call whose `ClientToken` replayed a request this
/// service had already started, so the call changed nothing.
pub const OUTCOME_REPLAYED: &str = "replayed";

/// The outcome a record carries for a call that canceled a resource operation request,
/// abandoning the work that request owed.
pub const OUTCOME_CANCELED: &str = "canceled";

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// A fresh token, in the form Cloud Control API hands one out: a random UUID written in
/// the canonical hyphenated form, which satisfies the `RequestToken` pattern
/// (`^[-A-Za-z0-9+/=]+$`, 1..128) and the `Identifier` pattern (`^.+$`, 1..1024) alike.
///
/// Every `RequestToken` the service hands out, and every primary identifier it assigns
/// itself, comes from here.
pub fn mint_token() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The resource's own name inside a `TypeName` — the third `::` segment of
/// `<Organization>::<Service>::<Resource>`.
pub fn resource_segment(type_name: &str) -> &str {
    type_name.split("::").nth(2).unwrap_or(type_name)
}

/// The properties a resource type's primary identifier can be carried by, most specific
/// first.
///
/// A resource type's schema names its own primary identifier, and across the registry
/// that property is named after the resource the type describes — `BucketName` for
/// `AWS::S3::Bucket`, `LogGroupName` for `AWS::Logs::LogGroup`, `VpcId` for
/// `AWS::EC2::VPC` — falling back to the bare `Name`/`Id`/`Arn` a type uses when the
/// resource's name would only repeat itself. The order is most specific first, so a
/// model carrying both `RoleName` and `Arn` is identified by the first.
///
/// A candidate is matched against the model's property names IGNORING ASCII case
/// ([`derive_identifier`]): a type name spells an acronym in full where the property
/// spells it as a word (`AWS::EC2::VPC` carries `VpcId`), and that is a difference in
/// spelling rather than a different property.
pub fn identifier_property_candidates(type_name: &str) -> Vec<String> {
    let resource = resource_segment(type_name);
    vec![
        format!("{resource}Name"),
        format!("{resource}Id"),
        format!("{resource}Arn"),
        "Name".to_string(),
        "Id".to_string(),
        "Arn".to_string(),
    ]
}

/// The property a SERVER-ASSIGNED primary identifier is recorded under, for a type whose
/// desired state names none: the type's own id property, which is how the registry names
/// an identifier the service allocates rather than the caller.
pub fn assigned_identifier_property(type_name: &str) -> String {
    format!("{}Id", resource_segment(type_name))
}

/// Where a resource's primary identifier came from.
#[derive(Debug, PartialEq, Eq)]
pub enum DerivedIdentifier {
    /// The desired state names it, under `property`.
    Carried {
        /// The property that carries it.
        property: String,
        /// The identifier itself.
        identifier: String,
    },
    /// The desired state names none, so the service assigned one under `property`.
    Assigned {
        /// The property it is recorded under.
        property: String,
        /// The identifier itself.
        identifier: String,
    },
    /// `property` is the type's primary identifier but holds something that cannot be
    /// one: not a scalar, empty, or longer than [`MAX_IDENTIFIER_CHARACTERS`].
    Unusable {
        /// The offending property.
        property: String,
    },
}

/// The identifier text a property's value carries, or `None` when its value cannot be
/// an identifier: not a scalar, empty, or longer than [`MAX_IDENTIFIER_CHARACTERS`].
///
/// An identifier travels on the wire as text, so a numeric or boolean property still
/// names the resource — and this is the ONE rule that says so, applied both where a
/// create derives the identifier and where an update checks the patch left it alone.
pub fn identifier_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text.chars().count() <= MAX_IDENTIFIER_CHARACTERS).then_some(text)
}

/// The primary identifier of the resource `properties` describes, by the one rule every
/// create applies ([`identifier_property_candidates`]).
pub fn derive_identifier(properties: &Value, type_name: &str) -> DerivedIdentifier {
    for candidate in identifier_property_candidates(type_name) {
        let Some((property, value)) = property_named(properties, &candidate) else {
            continue;
        };
        return match identifier_text(value) {
            Some(identifier) => DerivedIdentifier::Carried {
                property,
                identifier,
            },
            None => DerivedIdentifier::Unusable { property },
        };
    }
    DerivedIdentifier::Assigned {
        property: assigned_identifier_property(type_name),
        identifier: mint_token(),
    }
}

/// The property of `properties` named `candidate` ignoring ASCII case, with the name as
/// the model actually spells it — so a refusal and the record both name the property the
/// caller sent rather than the candidate that matched it.
fn property_named<'a>(properties: &'a Value, candidate: &str) -> Option<(String, &'a Value)> {
    properties
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(candidate))
        .map(|(name, value)| (name.clone(), value))
}

/// A digest over the caller-owned members of one request, length-prefixed so no two
/// different member sets can digest alike and absence is distinguished from the empty
/// string.
///
/// This is what a `ClientToken`'s record holds, so a reuse with different parameters is
/// told from a retry without the store keeping the request bodies themselves.
pub fn request_fingerprint(members: &[Option<&str>]) -> String {
    let mut digest = Sha256::new();
    for member in members {
        match member {
            Some(text) => {
                digest.update([1_u8]);
                digest.update((text.len() as u64).to_be_bytes());
                digest.update(text.as_bytes());
            }
            None => digest.update([0_u8]),
        }
    }
    ::hex::encode(digest.finalize())
}

// ---------------------------------------------------------------------------
// The resource model
// ---------------------------------------------------------------------------

/// The resource model `desired_state` describes, or the detail of why it is not one.
///
/// `DesiredState` is "structured data format representing the desired state of the
/// resource, consisting of that resource's properties and their desired values. Cloud
/// Control API currently supports JSON as a structured data format", so it is read by
/// the JSON reader and has to be an object of properties. The reader's own recursion
/// limit is what bounds how deeply nested a caller's document may be.
pub fn parse_properties(member: &str, desired_state: &str) -> Result<Value, String> {
    let parsed: Value = ::serde_json::from_str(desired_state)
        .map_err(|_| format!("{member} is not valid JSON"))?;
    if !parsed.is_object() {
        return Err(format!("{member} must be a JSON object of resource properties"));
    }
    Ok(parsed)
}

/// One resource model as the wire carries it: a JSON string of each property and its
/// current value.
pub fn rendered_properties(properties: &Value) -> String {
    // `Value`'s own rendering is infallible, so a resource model is never unreportable.
    properties.to_string()
}

/// The most operations one `PatchDocument` may carry.
///
/// A patch document is bounded by the model at 262144 characters, but what RFC 6902's
/// `copy` and `move` operations can DUPLICATE is not bounded by that: each one can clone
/// a whole subtree, so a short document can describe a document that grows
/// multiplicatively. The work one request can drive is therefore bounded here by the
/// operation count as well as by the size of the document between every pair of them
/// ([`apply_patch`]).
///
/// This limit is the EMULATOR's, not the service's: nothing AWS publishes caps a patch
/// document's operation count, so a document past this is refused as an invalid request
/// rather than with a code or wording the service has for it.
pub const MAX_PATCH_OPERATIONS: usize = 100;

/// The deepest a resource model may nest.
///
/// This is the depth the JSON reader itself admits in a document, so a model deeper than
/// this is one no `DesiredState` or `ResourceModel` could have been read from the wire in
/// the first place ([`parse_properties`]) and one no reader could report back. A PATCH can
/// reach past it even though a document cannot: `copy` and `move` duplicate a whole
/// subtree, so an operation that copies the model into one of its own children DOUBLES its
/// depth while adding only a few bytes — which is why depth is bounded beside size
/// ([`MAX_PROPERTIES_CHARACTERS`]) rather than left to it.
pub const MAX_PROPERTIES_DEPTH: usize = 128;

/// Why a patch document was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PatchRefusal {
    /// The document carries more operations than a patch may.
    TooManyOperations,
    /// One of its operations could not be applied to the model: the operation, as the
    /// document spells it, and the path it names.
    NotApplicable {
        /// The operation RFC 6902 name of the step that failed.
        operation: &'static str,
        /// The pointer that step named.
        path: String,
    },
    /// The model it produced is not an object of properties.
    NotAnObject,
    /// The model it produced is longer than one this service can report.
    TooLarge,
    /// The model it produced nests deeper than one this service can read or report.
    TooDeep,
}

impl PatchRefusal {
    /// What the refusal states, behind the prefix the refusal is carried under.
    pub fn detail(&self) -> String {
        match self {
            Self::TooManyOperations => format!(
                "PatchDocument must carry at most {MAX_PATCH_OPERATIONS} patch operations"
            ),
            // Naming the operation and the path it failed on is what tells this refusal
            // from every other reason a document may be rejected: the caller has to know
            // WHICH step of its document the resource model would not take.
            Self::NotApplicable { operation, path } => format!(
                "the {operation} operation at path '{path}' could not be applied to the \
                 resource model"
            ),
            Self::NotAnObject => {
                "the patch document would leave a resource model that is not a JSON object"
                    .to_string()
            }
            Self::TooLarge => format!(
                "the patch document would leave a resource model longer than \
                 {MAX_PROPERTIES_CHARACTERS} characters"
            ),
            Self::TooDeep => format!(
                "the patch document would leave a resource model nested deeper than \
                 {MAX_PROPERTIES_DEPTH} levels"
            ),
        }
    }

    /// Whether this refusal is a fault in the DOCUMENT, which the front door judges from
    /// the request and the resource model alone, rather than a verdict on the model the
    /// handler would have been asked to store.
    ///
    /// A patch operation that names a path the model does not carry cannot be applied to
    /// any model, so it is the front door's refusal. The remaining refusals are about
    /// what applying the document WOULD leave behind — a value that is not a resource
    /// model, one past the length the model can report — or about this emulator's own
    /// cap on the work one document may drive ([`MAX_PATCH_OPERATIONS`]), none of which
    /// the front door decides.
    pub fn is_document_fault(&self) -> bool {
        matches!(self, Self::NotApplicable { .. })
    }
}

/// The RFC 6902 name of one patch operation, as a patch document spells it, and the
/// pointer it names.
fn operation_description(operation: &::json_patch::PatchOperation) -> (&'static str, String) {
    use ::json_patch::PatchOperation;

    match operation {
        PatchOperation::Add(operation) => ("add", operation.path.to_string()),
        PatchOperation::Remove(operation) => ("remove", operation.path.to_string()),
        PatchOperation::Replace(operation) => ("replace", operation.path.to_string()),
        PatchOperation::Move(operation) => ("move", operation.path.to_string()),
        PatchOperation::Copy(operation) => ("copy", operation.path.to_string()),
        PatchOperation::Test(operation) => ("test", operation.path.to_string()),
    }
}

/// The top-level properties of the resource model a patch document would CHANGE.
///
/// A patch operation names its target as a JSON pointer into the model, so the property
/// it changes is that pointer's first token: `/Tags/0/Key` changes `Tags`. An operation
/// that MOVES a value changes the property it writes to and the one it takes the value
/// from, since a move removes the source; a `copy` only reads its source, and a `test`
/// changes nothing at all. The empty pointer names the whole model rather than any one
/// property, so it names none here and the invariants on the model the patch leaves
/// behind are what judge it.
///
/// This is what lets a refusal be decided from the DOCUMENT: a property a patch may not
/// change is refused because the document names it, whether or not the operation could
/// have been applied.
pub fn patched_properties(patch: &::json_patch::Patch) -> BTreeSet<String> {
    use ::json_patch::PatchOperation;

    let mut properties = BTreeSet::new();
    for operation in &patch.0 {
        let targets: Vec<&::json_patch::jsonptr::Pointer> = match operation {
            PatchOperation::Add(operation) => vec![&operation.path],
            PatchOperation::Remove(operation) => vec![&operation.path],
            PatchOperation::Replace(operation) => vec![&operation.path],
            PatchOperation::Move(operation) => vec![&operation.path, &operation.from],
            PatchOperation::Copy(operation) => vec![&operation.path],
            PatchOperation::Test(_) => Vec::new(),
        };
        for target in targets {
            if let Some(token) = target.first() {
                properties.insert(token.decoded().into_owned());
            }
        }
    }
    properties
}

/// Apply `patch` to `properties`, ONE operation at a time, holding the document to the
/// `Properties` bounds after every one of them.
///
/// The bounds are checked BETWEEN operations rather than only at the end, because that is
/// the only place they bound the work: `copy` and `move` duplicate a whole subtree, so a
/// document measured only once could have grown by a factor per operation on the way
/// there — in SIZE, and in nesting DEPTH, which a few bytes of document can double. With
/// the checks between each pair, the largest and deepest document that can exist is one
/// operation's growth past the bound, and the operation count caps how many such steps a
/// request can ask for ([`MAX_PATCH_OPERATIONS`]).
///
/// Depth is checked FIRST, and iteratively ([`properties_within_depth`]): every other walk
/// of a `Value` — rendering it to measure its size, and dropping it — recurses once per
/// level, so the depth bound is what keeps those walks inside the thread's stack.
///
/// On a refusal `properties` may hold the operations applied before it, so a caller
/// applies this to a COPY of the stored model and discards it unless the whole patch
/// succeeded.
pub fn apply_patch(properties: &mut Value, patch: &::json_patch::Patch) -> Result<(), PatchRefusal> {
    if patch.0.len() > MAX_PATCH_OPERATIONS {
        return Err(PatchRefusal::TooManyOperations);
    }
    for operation in &patch.0 {
        let step = ::json_patch::Patch(vec![operation.clone()]);
        ::json_patch::patch(properties, &step).map_err(|_| {
            let (named, path) = operation_description(operation);
            PatchRefusal::NotApplicable {
                operation: named,
                path,
            }
        })?;
        if !properties_within_depth(properties) {
            return Err(PatchRefusal::TooDeep);
        }
        if !properties_within_bound(properties) {
            return Err(PatchRefusal::TooLarge);
        }
    }
    // Only the WHOLE patch has to leave a resource model: an intermediate value of any
    // shape is RFC 6902's business, and a later operation may put an object back.
    if !properties.is_object() {
        return Err(PatchRefusal::NotAnObject);
    }
    Ok(())
}

/// Whether `properties` nests no deeper than [`MAX_PROPERTIES_DEPTH`].
///
/// Walked ITERATIVELY, with an explicit stack of the values still to look at, so measuring
/// a document deeper than this service will take cannot itself exhaust the thread's stack
/// — which a recursive walk of exactly such a document is how it would.
fn properties_within_depth(properties: &Value) -> bool {
    let mut pending = vec![(properties, 1_usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_PROPERTIES_DEPTH {
            return false;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => {
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    true
}

/// Whether `properties` renders inside [`MAX_PROPERTIES_CHARACTERS`].
///
/// Measured through a counting sink that refuses the first byte past the bound, so a
/// document far past it is abandoned where it is crossed instead of rendered whole, and
/// measuring one allocates nothing.
fn properties_within_bound(properties: &Value) -> bool {
    let mut sink = BoundedSink {
        written: 0,
        limit: MAX_PROPERTIES_CHARACTERS,
    };
    ::serde_json::to_writer(&mut sink, properties).is_ok()
}

/// A sink that counts what is written to it and stops accepting at `limit`.
struct BoundedSink {
    /// How many bytes have been offered so far.
    written: usize,
    /// The most it accepts.
    limit: usize,
}

impl ::std::io::Write for BoundedSink {
    fn write(&mut self, buf: &[u8]) -> ::std::io::Result<usize> {
        self.written = self.written.saturating_add(buf.len());
        if self.written > self.limit {
            return Err(::std::io::Error::other("past the size bound"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> ::std::io::Result<()> {
        Ok(())
    }
}

/// Whether `properties` carries every member of `selector` with the same value — the
/// rule `ListResources$ResourceModel` ("the resource model to use to select the
/// resources to return") selects by, so a listing narrowed by a partial model reports
/// the resources that match it and no others.
pub fn properties_match_selector(properties: &Value, selector: &Value) -> bool {
    let Some(selector) = selector.as_object() else {
        return true;
    };
    selector
        .iter()
        .all(|(property, wanted)| properties.get(property) == Some(wanted))
}

// ---------------------------------------------------------------------------
// The registry of resource types
// ---------------------------------------------------------------------------

/// The namespaces this emulator's CloudFormation registry holds resource types under —
/// the `<Organization>::<Service>` half of a `TypeName`.
///
/// Catalog source: the namespaces CloudFormation publishes resource types under, as the
/// AWS CloudFormation resource and property type reference lists them (one
/// `AWS::<Service>::…` section per service, plus Alexa's own `Alexa::ASK`).
///
/// A `TypeName` is `<Organization>::<Service>::<Resource>` and the registry is keyed by
/// the whole triple, but WHICH resources one service publishes is that service's own
/// business and changes with every release, while the namespace a type has to live in
/// does not: a namespace the registry holds nothing under can hold no type at all, which
/// is what [`type_is_registered`] decides. A private type a particular account
/// registered is not here either, because this emulator's registry holds none.
const REGISTERED_NAMESPACES: &[&str] = &[
    "Alexa::ASK",
    "AWS::ACMPCA",
    "AWS::AccessAnalyzer",
    "AWS::AmazonMQ",
    "AWS::Amplify",
    "AWS::AmplifyUIBuilder",
    "AWS::ApiGateway",
    "AWS::ApiGatewayV2",
    "AWS::AppConfig",
    "AWS::AppFlow",
    "AWS::AppIntegrations",
    "AWS::AppMesh",
    "AWS::AppRunner",
    "AWS::AppStream",
    "AWS::AppSync",
    "AWS::ApplicationAutoScaling",
    "AWS::ApplicationInsights",
    "AWS::Athena",
    "AWS::AuditManager",
    "AWS::AutoScaling",
    "AWS::AutoScalingPlans",
    "AWS::Backup",
    "AWS::Batch",
    "AWS::Budgets",
    "AWS::CE",
    "AWS::Cassandra",
    "AWS::CertificateManager",
    "AWS::Chatbot",
    "AWS::Cloud9",
    "AWS::CloudFormation",
    "AWS::CloudFront",
    "AWS::CloudTrail",
    "AWS::CloudWatch",
    "AWS::CodeArtifact",
    "AWS::CodeBuild",
    "AWS::CodeCommit",
    "AWS::CodeDeploy",
    "AWS::CodeGuruProfiler",
    "AWS::CodeGuruReviewer",
    "AWS::CodePipeline",
    "AWS::CodeStar",
    "AWS::CodeStarConnections",
    "AWS::Cognito",
    "AWS::Comprehend",
    "AWS::Config",
    "AWS::Connect",
    "AWS::ControlTower",
    "AWS::CustomerProfiles",
    "AWS::DAX",
    "AWS::DLM",
    "AWS::DMS",
    "AWS::DataBrew",
    "AWS::DataPipeline",
    "AWS::DataSync",
    "AWS::Detective",
    "AWS::DevOpsGuru",
    "AWS::DirectoryService",
    "AWS::DocDB",
    "AWS::DynamoDB",
    "AWS::EC2",
    "AWS::ECR",
    "AWS::ECS",
    "AWS::EFS",
    "AWS::EKS",
    "AWS::EMR",
    "AWS::EMRContainers",
    "AWS::EMRServerless",
    "AWS::ElastiCache",
    "AWS::ElasticBeanstalk",
    "AWS::ElasticLoadBalancing",
    "AWS::ElasticLoadBalancingV2",
    "AWS::Elasticsearch",
    "AWS::EventSchemas",
    "AWS::Events",
    "AWS::Evidently",
    "AWS::FIS",
    "AWS::FMS",
    "AWS::FSx",
    "AWS::FinSpace",
    "AWS::Forecast",
    "AWS::FraudDetector",
    "AWS::GameLift",
    "AWS::GlobalAccelerator",
    "AWS::Glue",
    "AWS::Grafana",
    "AWS::Greengrass",
    "AWS::GreengrassV2",
    "AWS::GroundStation",
    "AWS::GuardDuty",
    "AWS::HealthLake",
    "AWS::IAM",
    "AWS::IVS",
    "AWS::ImageBuilder",
    "AWS::Inspector",
    "AWS::InspectorV2",
    "AWS::InternetMonitor",
    "AWS::IoT",
    "AWS::IoTAnalytics",
    "AWS::IoTEvents",
    "AWS::IoTFleetHub",
    "AWS::IoTSiteWise",
    "AWS::IoTTwinMaker",
    "AWS::IoTWireless",
    "AWS::KMS",
    "AWS::KafkaConnect",
    "AWS::Kendra",
    "AWS::Kinesis",
    "AWS::KinesisAnalytics",
    "AWS::KinesisAnalyticsV2",
    "AWS::KinesisFirehose",
    "AWS::KinesisVideo",
    "AWS::LakeFormation",
    "AWS::Lambda",
    "AWS::Lex",
    "AWS::LicenseManager",
    "AWS::Lightsail",
    "AWS::Location",
    "AWS::Logs",
    "AWS::LookoutEquipment",
    "AWS::LookoutMetrics",
    "AWS::LookoutVision",
    "AWS::M2",
    "AWS::MSK",
    "AWS::Macie",
    "AWS::ManagedBlockchain",
    "AWS::MediaConnect",
    "AWS::MediaConvert",
    "AWS::MediaLive",
    "AWS::MediaPackage",
    "AWS::MediaStore",
    "AWS::MediaTailor",
    "AWS::MemoryDB",
    "AWS::Neptune",
    "AWS::NetworkFirewall",
    "AWS::NetworkManager",
    "AWS::NimbleStudio",
    "AWS::OpenSearchServerless",
    "AWS::OpenSearchService",
    "AWS::OpsWorks",
    "AWS::Organizations",
    "AWS::Panorama",
    "AWS::Personalize",
    "AWS::Pinpoint",
    "AWS::Pipes",
    "AWS::QLDB",
    "AWS::QuickSight",
    "AWS::RAM",
    "AWS::RDS",
    "AWS::RUM",
    "AWS::Redshift",
    "AWS::RedshiftServerless",
    "AWS::RefactorSpaces",
    "AWS::Rekognition",
    "AWS::ResilienceHub",
    "AWS::ResourceExplorer2",
    "AWS::ResourceGroups",
    "AWS::RoboMaker",
    "AWS::Route53",
    "AWS::Route53RecoveryControl",
    "AWS::Route53RecoveryReadiness",
    "AWS::Route53Resolver",
    "AWS::S3",
    "AWS::S3ObjectLambda",
    "AWS::S3Outposts",
    "AWS::SDB",
    "AWS::SES",
    "AWS::SNS",
    "AWS::SQS",
    "AWS::SSM",
    "AWS::SSMContacts",
    "AWS::SSMIncidents",
    "AWS::SSO",
    "AWS::SageMaker",
    "AWS::SecretsManager",
    "AWS::SecurityHub",
    "AWS::ServiceCatalog",
    "AWS::ServiceCatalogAppRegistry",
    "AWS::ServiceDiscovery",
    "AWS::Shield",
    "AWS::Signer",
    "AWS::SimSpaceWeaver",
    "AWS::StepFunctions",
    "AWS::SupportApp",
    "AWS::Synthetics",
    "AWS::Timestream",
    "AWS::Transfer",
    "AWS::VoiceID",
    "AWS::VpcLattice",
    "AWS::WAF",
    "AWS::WAFRegional",
    "AWS::WAFv2",
    "AWS::Wisdom",
    "AWS::WorkSpaces",
    "AWS::XRay",
];

/// The `<Organization>::<Service>` namespace of `type_name`, or `None` for a value that
/// is not a three-segment type name at all.
///
/// The `TypeName` pattern already holds a request to three non-empty alphanumeric
/// segments, so this reads the segments rather than re-checking them.
fn type_namespace(type_name: &str) -> Option<String> {
    let mut segments = type_name.split("::");
    let organization = segments.next()?;
    let service = segments.next()?;
    let resource = segments.next()?;
    (!resource.is_empty() && segments.next().is_none())
        .then(|| format!("{organization}::{service}"))
}

/// Whether this emulator's CloudFormation registry holds `type_name` —
/// what every operation that names a `TypeName` asks before it resolves anything that
/// type name names ([`REGISTERED_NAMESPACES`] states what the registry holds).
pub fn type_is_registered(type_name: &str) -> bool {
    type_namespace(type_name)
        .is_some_and(|namespace| REGISTERED_NAMESPACES.contains(&namespace.as_str()))
}

/// Whether a resource type's provider publishes handlers for this service to call at all.
///
/// Cloud Control API acts on a resource ONLY through the handlers the resource type's
/// provider publishes — "the necessary permissions for each event handler are defined in
/// the handlers section of the resource type definition schema"
/// (`CreateResource$RoleArn`) — so a type whose provider publishes none of them, the
/// provisioning class the CloudFormation registry reports as `NON_PROVISIONABLE`, has
/// nothing for any action of this service to run. Which types those are is
/// [`::service_common::cfn_provisioning`]'s to answer, so the registry that REPORTS the
/// class and this service, which REFUSES the action, cannot disagree about one type.
pub fn type_supports_actions(type_name: &str) -> bool {
    !::service_common::cfn_provisioning::is_non_provisionable(type_name)
}

/// The `READ` action, which `GetResource` performs: the action names this service refuses
/// by are its resource actions, which the three a `ProgressEvent` reports as its
/// `Operation` ([`OPERATION_CREATE`], [`OPERATION_UPDATE`], [`OPERATION_DELETE`]) are the
/// mutating half of — a read and a listing start no resource operation request, so neither
/// is an `Operation` the model carries.
pub const ACTION_READ: &str = "READ";

/// The `LIST` action, which `ListResources` performs; see [`ACTION_READ`].
pub const ACTION_LIST: &str = "LIST";

// ---------------------------------------------------------------------------
// Refusal wording
// ---------------------------------------------------------------------------

/// The `HandlerErrorCode` for a resource the owning handler could not find.
pub const HANDLER_ERROR_NOT_FOUND: &str = "NotFound";

/// The `HandlerErrorCode` for a resource the owning handler found already there.
pub const HANDLER_ERROR_ALREADY_EXISTS: &str = "AlreadyExists";

/// One resource type's own not-found diagnostic, for the types whose handler states one
/// of its own rather than Cloud Control API's generic sentence.
struct HandlerDiagnostic {
    /// The sentence that handler states, carrying [`IDENTIFIER_PLACEHOLDER`] wherever the
    /// sentence names the resource itself — which is a value taken from the request rather
    /// than part of the fixed text.
    sentence: &'static str,
    /// The `Service` the owning handler's client is for, where that handler carries its
    /// client's rendering of the service's exception rather than the bare sentence.
    sdk_service: Option<&'static str>,
}

/// Where a [`HandlerDiagnostic::sentence`] names the resource the refusal is about.
const IDENTIFIER_PLACEHOLDER: &str = "{Identifier}";

/// What the owning resource type's handler states for a resource it cannot find, keyed
/// by `TypeName` — the registry's own key.
///
/// Cloud Control API is a facade over the type's handler, so the diagnostic inside a
/// not-found refusal is that HANDLER's, not Cloud Control's: CloudWatch's handler says an
/// alarm does not exist, SNS's says a topic does not exist. A type absent from here is
/// reported in Cloud Control's own generic sentence
/// ([`resource_not_found_message`]), which is what a handler built on the generic
/// resource-provider base class states.
///
/// Each sentence is the one RECORDED for that type's handler through this API, which is
/// not always how the owning service words the same absence on its own wire: the
/// `AWS::S3::Bucket` handler states "Bucket not found" where `s3:HeadBucket` answers
/// `NoSuchBucket` "The specified bucket does not exist", and the `AWS::DynamoDB::Table`
/// handler states "Table: <name> does not exist." where `dynamodb:DescribeTable` answers
/// "Requested resource not found". The handler's own sentence is what a Cloud Control API
/// client reads, so it is what this catalog holds; only the resource's name inside it
/// comes from the request ([`IDENTIFIER_PLACEHOLDER`]).
const HANDLER_NOT_FOUND_DIAGNOSTICS: &[(&str, HandlerDiagnostic)] = &[
    (
        "AWS::CloudWatch::Alarm",
        HandlerDiagnostic {
            sentence: "Alarm does not exist",
            sdk_service: None,
        },
    ),
    (
        "AWS::DynamoDB::Table",
        HandlerDiagnostic {
            sentence: "Table: {Identifier} does not exist.",
            sdk_service: None,
        },
    ),
    (
        "AWS::S3::Bucket",
        HandlerDiagnostic {
            sentence: "Bucket not found",
            sdk_service: None,
        },
    ),
    (
        "AWS::SNS::Topic",
        HandlerDiagnostic {
            sentence: "Topic does not exist",
            sdk_service: Some("Sns"),
        },
    ),
];

/// What the owning resource type's handler states for a create whose resource is already
/// there, keyed by `TypeName`.
///
/// Same rule as [`HANDLER_NOT_FOUND_DIAGNOSTICS`]: the sentence belongs to the service
/// that owns the type. A type absent from here is reported in Cloud Control's own
/// generic sentence ([`already_exists_message`]).
const HANDLER_ALREADY_EXISTS_DIAGNOSTICS: &[(&str, &str)] = &[(
    "AWS::SQS::Queue",
    "Queue creation failed because the queue already exists",
)];

/// The status code an owning service reports for a resource that is not there.
const HANDLER_NOT_FOUND_STATUS: u16 = 404;

/// How every operation of this service words "a resource with the specified identifier
/// can't be found", with the type the request named and the resource as the owning
/// handler names it ([`handler_rendered_identifier`]).
pub fn resource_not_found_message(type_name: &str, identifier: &str) -> String {
    format!(
        "Resource of type '{type_name}' with identifier '{}' was not found.",
        handler_rendered_identifier(type_name, identifier)
    )
}

/// The diagnostic the handler that owns `type_name` states for a resource it cannot
/// find.
pub fn handler_not_found_detail(type_name: &str, identifier: &str) -> String {
    owning_not_found_detail(type_name, identifier)
        .unwrap_or_else(|| resource_not_found_message(type_name, identifier))
}

/// The diagnostic the HANDLER that owns `type_name` states for a resource it cannot find,
/// or `None` for a type whose handler reports a not-found in Cloud Control API's own terms
/// ([`resource_not_found_message`]) — which is what a handler built on the generic
/// resource-provider base class states, because the not-found exception that base class
/// throws names nothing but the type and the identifier.
fn owning_not_found_detail(type_name: &str, identifier: &str) -> Option<String> {
    let (_, diagnostic) = HANDLER_NOT_FOUND_DIAGNOSTICS
        .iter()
        .find(|(name, _)| *name == type_name)?;
    // The sentence names the resource wherever the owning service's wording does, with the
    // identifier the request carried.
    let sentence = diagnostic
        .sentence
        .replace(IDENTIFIER_PLACEHOLDER, identifier);
    Some(match diagnostic.sdk_service {
        // The handler carries its client's rendering of the owning service's own
        // exception, which names the service, the status it answered and the request the
        // handler made of it.
        Some(service) => format!(
            "{sentence} (Service: {service}, Status Code: {HANDLER_NOT_FOUND_STATUS}, \
             Request ID: {})",
            mint_token()
        ),
        None => sentence,
    })
}

/// How a create words "the resource with the name requested already exists", naming the
/// resource by the identifier the request carried.
///
/// The recorded refusals this sentence is held to name the resource that way for every
/// type, so no type renders a schema map here ([`handler_rendered_identifier`] is about the
/// not-found diagnostic the recordings DO cover).
pub fn already_exists_message(type_name: &str, identifier: &str) -> String {
    format!("Resource of type '{type_name}' with identifier '{identifier}' already exists.")
}

/// The diagnostic the handler that owns `type_name` states for a create whose resource is
/// already there.
pub fn handler_already_exists_detail(type_name: &str, identifier: &str) -> String {
    HANDLER_ALREADY_EXISTS_DIAGNOSTICS
        .iter()
        .find(|(name, _)| *name == type_name)
        .map(|(_, sentence)| (*sentence).to_string())
        .unwrap_or_else(|| already_exists_message(type_name, identifier))
}

/// How a SYNCHRONOUS refusal carries a handler verdict: the owning handler's own
/// diagnostic, the `HandlerErrorCode` it failed with and the token of the handler
/// invocation that failed.
///
/// A `ProgressEvent` reports the handler error code in `ErrorCode` and the diagnostic in
/// `StatusMessage`, each in its own member. An exception has only its `Message`, so a
/// refusal a resource-level operation answers inline is where the whole verdict has to
/// go — which is why clients recover the handler error code from this text.
pub fn handler_failure_message(
    type_name: &str,
    detail: &str,
    handler_error_code: &str,
    request_token: &str,
) -> String {
    format!(
        "{type_name} Handler returned status {STATUS_FAILED}: {detail} \
         (HandlerErrorCode: {handler_error_code}, RequestToken: {request_token})"
    )
}

/// The `Message` a resource-level operation refuses an absent resource with.
///
/// Which of the two forms a refusal takes follows from WHOSE diagnostic it carries:
///
/// * a handler that states the not-found in Cloud Control API's own terms — the not-found
///   exception the generic resource-provider base class throws, whose sentence names
///   nothing but the type and the identifier — is reported as THAT sentence, which is the
///   modelled "a resource with the specified identifier can't be found" itself;
/// * a handler that failed with the OWNING SERVICE's own exception reports that exception,
///   and the handler envelope around it is what carries the rest of the verdict
///   ([`handler_failure_message`]): the `HandlerErrorCode` it failed with and the
///   `RequestToken` of the invocation that failed, neither of which an exception's single
///   `Message` member would otherwise leave a client able to recover.
///
/// The token is minted where the envelope is used because the handler invocation the
/// refusal reports IS this call: nothing was accepted, so there is no resource operation
/// request to poll and no record for the token to name.
/// The `Message` a resource-level operation refuses an absent resource with: the
/// handler-wrapped not-found verdict ([`handler_failure_message`]) around the diagnostic
/// the type's own handler states ([`handler_not_found_detail`]).
///
/// The token is minted here because the handler invocation this refusal reports IS this
/// call: nothing was accepted, so there is no resource operation request to poll and no
/// record for the token to name.
pub fn resource_not_found_refusal(type_name: &str, identifier: &str) -> String {
    handler_failure_message(
        type_name,
        &handler_not_found_detail(type_name, identifier),
        HANDLER_ERROR_NOT_FOUND,
        &mint_token(),
    )
}

/// How a mutating operation words a refusal caused by the request itself, with the
/// prefix Cloud Control API states an invalid request behind.
pub fn invalid_request_message(detail: &str) -> String {
    format!("Invalid request provided: {detail}")
}

/// The prefix a resource type's registry schema names one of its properties with: every
/// pointer in its `createOnlyProperties`, `readOnlyProperties` and `primaryIdentifier`
/// lists points into the schema's `properties` object.
const PROPERTY_POINTER_PREFIX: &str = "/properties/";

/// The properties a resource type's registry schema declares `createOnlyProperties`,
/// keyed by `TypeName`.
///
/// Catalog source: the `createOnlyProperties` list of each type's resource provider
/// schema, as CloudFormation publishes it (`DescribeType`), with the
/// `/properties/` prefix of each pointer dropped — it is put back wherever a pointer is
/// rendered ([`not_updatable_message`]).
///
/// A type absent from here still has ONE create-only property: its own primary
/// identifier, which is what every operation addresses the resource by and therefore
/// cannot change while the resource stays the same resource. That is what
/// [`create_only_properties`] adds for every type, catalogued or not.
const CREATE_ONLY_PROPERTIES: &[(&str, &[&str])] = &[
    ("AWS::CloudWatch::Alarm", &["AlarmName"]),
    ("AWS::DynamoDB::Table", &["TableName", "KeySchema"]),
    ("AWS::EC2::VPC", &["CidrBlock", "Ipv4IpamPoolId"]),
    ("AWS::Kinesis::Stream", &["Name"]),
    ("AWS::Logs::LogGroup", &["LogGroupName"]),
    ("AWS::S3::Bucket", &["BucketName", "ObjectLockEnabled"]),
    ("AWS::SNS::Topic", &["TopicName", "FifoTopic"]),
    ("AWS::SQS::Queue", &["QueueName", "FifoQueue"]),
    ("AWS::SSM::Parameter", &["Name"]),
];

/// The properties a patch may not change for `type_name`, whose primary identifier the
/// resource carries under `identifier_property`: the type's declared
/// `createOnlyProperties`, and that identifier property wherever the schema's list does
/// not already name it.
///
/// Kept in the schema's own order, with the identifier first where it is added, so a
/// refusal lists the offending properties the way the schema declares them.
pub fn create_only_properties(type_name: &str, identifier_property: &str) -> Vec<String> {
    let declared = CREATE_ONLY_PROPERTIES
        .iter()
        .find(|(name, _)| *name == type_name)
        .map(|(_, properties)| *properties)
        .unwrap_or(&[]);
    let mut properties: Vec<String> = declared.iter().map(|name| (*name).to_string()).collect();
    if !properties.iter().any(|name| name == identifier_property) {
        properties.insert(0, identifier_property.to_string());
    }
    properties
}

/// How a mutating operation words a patch that would change a create-only property: the
/// patch diagnostic, naming the offending properties as the pointers into the type's
/// registry schema that its `createOnlyProperties` list spells them with.
///
/// Takes the whole set, because one patch document can touch several create-only
/// properties and the diagnostic lists them together.
pub fn not_updatable_message(properties: &[&str]) -> String {
    let pointers: Vec<String> = properties
        .iter()
        .map(|property| format!("{PROPERTY_POINTER_PREFIX}{property}"))
        .collect();
    format!(
        "Invalid patch update: createOnlyProperties [{}] cannot be updated",
        pointers.join(", ")
    )
}

/// How a mutating operation words a `PatchDocument` its reader would not take, with the
/// prefix Cloud Control API states such a refusal behind.
pub fn invalid_patch_document_message(detail: &str) -> String {
    format!("Invalid PatchDocument: {detail}")
}

/// What the document reader states about a `PatchDocument` it could not read as a list
/// of patch operations.
///
/// A patch document is read as a JSON ARRAY first, so a document whose first value
/// character is not `[` is refused by the reader before any operation is looked at, and
/// the complaint names where in the document that character is: the index of the
/// character read, its position on its line and the line it is on, all counted from the
/// document itself. A document that IS an array but carries something that is not a
/// patch operation fails later, on the operation rather than on the text.
pub fn patch_document_read_failure(document: &str) -> String {
    // Where the reader stopped: whitespace before the value is skipped, and the first
    // character of the value is the one it reports.
    let trimmed = document.trim_start();
    let skipped = &document[..document.len() - trimmed.len()];
    let value = trimmed.chars().next();
    if value == Some('[') {
        return "the document carries an operation that is not a JSON Patch operation".to_string();
    }
    // The character of the value itself is read before the reader gives up, so it counts
    // towards the index; a document that holds no value character at all leaves the
    // index at the whitespace it skipped.
    let read = usize::from(value.is_some());
    let index = skipped.chars().count() + read;
    let line = 1 + skipped.matches('\n').count();
    let on_line = match skipped.rfind('\n') {
        Some(newline) => skipped[newline + 1..].chars().count() + read,
        None => index,
    };
    format!(
        "A JSONArray text must start with '[' at {index} [character {} line {line}]",
        on_line + 1
    )
}

/// How a mutating operation words a resource another operation is already acting on: the
/// resource under contention and, when one is still running, the request that holds it.
///
/// The holding request is what lets a caller poll the operation that blocked it to
/// completion; a request that has since terminated leaves the resource named alone.
pub fn concurrent_operation_message(identifier: &str, holding_request: Option<&str>) -> String {
    match holding_request {
        Some(request_token) => format!(
            "Concurrent operation found for resource {identifier} with RequestToken \
             {request_token}"
        ),
        None => format!("Concurrent operation found for resource {identifier}"),
    }
}

/// How a mutating operation words a `ClientToken` that already named another request:
/// the token the caller submitted and the request it was first used for.
pub fn client_token_conflict_message(client_token: &str, request_token: &str) -> String {
    format!(
        "ClientToken {client_token} is already associated with an existing operation with \
         RequestToken {request_token}"
    )
}

/// How `GetResourceRequestStatus` words a token no request of this store answers to,
/// naming the token that was not found.
pub fn request_token_not_found_message(request_token: &str) -> String {
    format!("Request with token {request_token} was not found")
}

/// How a cancel words a request it will not cancel: the token the caller named and the
/// status that request has already reached, because "only resource operations requests
/// with a status of PENDING or IN_PROGRESS can be canceled".
pub fn request_already_in_status_message(request_token: &str, operation_status: &str) -> String {
    format!("Request {request_token} is already in status {operation_status}")
}

/// How an operation that names a `TypeName` words one the registry does not hold.
pub fn type_not_found_message(type_name: &str) -> String {
    format!("The type '{type_name}' cannot be found.")
}

/// How an operation words an action the resource type it names does not support: the type
/// from the request, and the resource action this operation performs.
pub fn unsupported_action_message(type_name: &str, action: &str) -> String {
    format!("Resource type {type_name} does not support {action} action")
}

/// How a mutating operation words a request it has no room to accept.
pub const THROTTLING_MESSAGE: &str = "Rate exceeded";

/// How a create words the service-side resource limit this store enforces.
pub fn resource_limit_message(type_name: &str) -> String {
    format!(
        "The account already holds the maximum of {MAX_RESOURCES} resources, so no resource of \
         type '{type_name}' can be created."
    )
}

// ---------------------------------------------------------------------------
// Wire rendering
// ---------------------------------------------------------------------------

/// The `ProgressEvent` one request record reports as.
///
/// Shared by the three mutating operations and `GetResourceRequestStatus`, so the event
/// a create answers with and the event a later status call reports cannot disagree about
/// one request.
pub fn progress_event(record: &RequestRecord) -> crate::ops::types::ProgressEvent {
    crate::ops::types::ProgressEvent {
        type_name: Some(record.type_name.clone()),
        identifier: record.identifier.clone(),
        request_token: Some(record.request_token.clone()),
        // Hooks are configured on the CloudFormation registry, and this emulator holds
        // none, so no request drives a Hook invocation to report a token for.
        hooks_request_token: None,
        operation: Some(record.operation.clone()),
        operation_status: Some(record.operation_status.clone()),
        event_time: Some(::aws_smithy_types::DateTime::from(record.event_time)),
        resource_model: record.resource_model.clone(),
        status_message: record.status_message.clone(),
        error_code: record.error_code.clone(),
        retry_after: record
            .retry_after
            .map(::aws_smithy_types::DateTime::from),
    }
}

/// The `ResourceDescription` one stored resource reports as.
///
/// Shared by `GetResource` and `ListResources`, so the two cannot report different
/// properties for one resource.
pub fn resource_description(
    resolved: &ResolvedResource,
) -> crate::ops::types::ResourceDescription {
    crate::ops::types::ResourceDescription {
        identifier: Some(resolved.identifier.clone()),
        properties: Some(rendered_properties(&resolved.properties)),
    }
}

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// The byte a page cursor puts between the type a listing is of and the identifier it
/// resumes after. A unit separator cannot occur in a `TypeName`, whose pattern admits
/// only alphanumerics and `::`.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The cursor that resumes a listing of `type_name` after `identifier`.
pub fn page_cursor(type_name: &str, identifier: &str) -> String {
    format!("{type_name}{CURSOR_SEPARATOR}{identifier}")
}

/// Encode `cursor` as an opaque `NextToken` this store can later recognise as its own:
/// the authenticated bytes of the cursor, hex-encoded.
pub fn encode_page_token(key: &PageTokenKey, cursor: &str) -> String {
    ::hex::encode(key.mint(cursor))
}

/// The identifier a `NextToken` resumes a listing of `type_name` after, or `None` for
/// any token THIS store did not mint for THAT type — one that is not hex, one too short
/// to carry an authenticator, one whose authenticator does not match the cursor it
/// travels with, and one minted for another type's listing.
pub fn decode_page_token(key: &PageTokenKey, type_name: &str, token: &str) -> Option<String> {
    let cursor = key.cursor_of(&::hex::decode(token).ok()?)?;
    let (cursor_type, identifier) = cursor.split_once(CURSOR_SEPARATOR)?;
    (cursor_type == type_name).then(|| identifier.to_string())
}

/// The position a request's `NextToken` names, or `None` when it names none.
///
/// The framework binds an absent member to `None` and an empty one to `Some("")`; an
/// empty token names no position, so a read takes it for no token at all and answers the
/// first page rather than refusing it.
pub fn requested_page_token(token: Option<&str>) -> Option<&str> {
    token.filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constants this service writes onto the wire are values the model's own enums
    /// declare, so a typo cannot reach a client.
    #[test]
    fn the_wire_vocabulary_is_the_models_own() {
        for operation in [OPERATION_CREATE, OPERATION_UPDATE, OPERATION_DELETE] {
            assert!(
                crate::ops::types::OPERATION_VALUES.contains(&operation),
                "operation {operation:?}: not a value the model's Operation enum declares"
            );
        }
        for status in [
            STATUS_PENDING,
            STATUS_IN_PROGRESS,
            STATUS_SUCCESS,
            STATUS_FAILED,
            STATUS_CANCEL_IN_PROGRESS,
            STATUS_CANCEL_COMPLETE,
        ] {
            assert!(
                crate::ops::types::OPERATION_STATUS_VALUES.contains(&status),
                "status {status:?}: not a value the model's OperationStatus enum declares"
            );
        }
        for status in CANCELABLE_STATUSES {
            assert!(
                crate::ops::types::OPERATION_STATUS_VALUES.contains(&status),
                "status {status:?}: not a value the model's OperationStatus enum declares"
            );
        }
        for code in [
            EffectFailure::ResourceGone.error_code(),
            EffectFailure::ResourceExists.error_code(),
        ] {
            assert!(
                crate::ops::types::HANDLER_ERROR_CODE_VALUES.contains(&code),
                "error code {code:?}: not a value the model's HandlerErrorCode enum declares"
            );
        }
    }

    /// The registry holds the namespaces CloudFormation publishes types under, and
    /// nothing else — so a type name of the right SHAPE is still not a type the registry
    /// can find.
    #[test]
    fn only_a_namespace_the_registry_holds_names_a_registered_type() {
        for registered in [
            // Not a type any check of this service names: the rule is about the
            // namespace, not about the types one suite happens to use.
            "AWS::Kinesis::Stream",
            "AWS::S3::Bucket",
            "Alexa::ASK::Skill",
        ] {
            assert!(
                type_is_registered(registered),
                "type {registered:?}: expected the registry to hold it"
            );
        }
        for unregistered in [
            "AWS::NoSuchService::FakeResource",
            "AWS::SimParityProbe::NoSuchType",
            "AWS::Custom::Widget",
            "Sqs::Queue",
            "AWS::SQS::Queue::Extra",
        ] {
            assert!(
                !type_is_registered(unregistered),
                "type {unregistered:?}: expected the registry to hold no such type"
            );
        }
    }

    /// A page token round-trips under the key and the type that minted it, and under
    /// nothing else.
    #[test]
    fn a_page_cursor_round_trips_only_for_the_type_it_was_minted_for() {
        let key = PageTokenKey::new();
        let token = encode_page_token(&key, &page_cursor("AWS::S3::Bucket", "demo-bucket"));
        assert_eq!(
            decode_page_token(&key, "AWS::S3::Bucket", &token).as_deref(),
            Some("demo-bucket"),
            "the cursor must round-trip for the type it was minted for"
        );
        assert_eq!(
            decode_page_token(&key, "AWS::Logs::LogGroup", &token),
            None,
            "a token minted for one type must not resume another type's listing"
        );
        assert_eq!(
            decode_page_token(&PageTokenKey::new(), "AWS::S3::Bucket", &token),
            None,
            "a token minted under another key must not decode"
        );
        assert_eq!(
            decode_page_token(&key, "AWS::S3::Bucket", "not-hex"),
            None,
            "a token a caller wrote must not decode"
        );
    }

    /// Absence and the empty string are different member sets, so a retry that drops a
    /// member is not mistaken for the request that sent it empty.
    #[test]
    fn a_fingerprint_distinguishes_absence_from_emptiness() {
        let absent = request_fingerprint(&[Some("CreateResource"), None]);
        let empty = request_fingerprint(&[Some("CreateResource"), Some("")]);
        assert_ne!(
            absent, empty,
            "an absent member must not digest like an empty one"
        );
        assert_eq!(
            absent,
            request_fingerprint(&[Some("CreateResource"), None]),
            "the same member set must digest alike"
        );
    }

    /// One accepted create of a type no provider stands behind, carrying `status`.
    fn record_in(status: &str, now: SystemTime) -> RequestRecord {
        let mut record = RequestRecord::accepted(
            "a-request-token".to_string(),
            "AWS::Kinesis::Stream".to_string(),
            Some("stream".to_string()),
            OPERATION_CREATE,
            now,
            ResourceEffect::Create {
                properties: ::serde_json::json!({ "StreamName": "stream" }),
                identifier_property: "StreamName".to_string(),
                type_version_id: None,
                region: DEFAULT_REGION.to_string(),
            },
        );
        record.operation_status = status.to_string();
        record
    }

    /// A cancel acts exactly on a request in one of the two cancelable statuses, and the
    /// work it abandons is never applied.
    ///
    /// The rows are the model's OWN `OperationStatus` enum
    /// (`smithy_query(query="shape",shape="OperationStatus")`), so every status a request
    /// can carry is a case — the terminal ones and the two a cancellation itself reaches.
    #[test]
    fn a_cancel_acts_only_inside_the_cancelable_window() {
        let native = NativeResources::for_engine(&());
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for status in crate::ops::types::OPERATION_STATUS_VALUES {
            let mut store = ComAmazonawsCloudcontrolCloudapiserviceState::default();
            store
                .record_request(record_in(status, now))
                .expect("an empty store takes one record");
            let verdict = store.cancel_request("a-request-token", now);
            if is_cancelable_status(status) {
                let canceling = match &verdict {
                    CancelVerdict::Canceling(record) => record.operation_status.clone(),
                    other => panic!(
                        "case {status:?}: expected the request to be canceled, got {other:?}"
                    ),
                };
                assert_eq!(
                    canceling, STATUS_CANCEL_IN_PROGRESS,
                    "case {status:?}: expected the canceling status, got {canceling:?}"
                );
                // The cancellation is what the request now owes the clock, and settling it
                // applies none of the work the request was carrying: nothing is provisioned.
                store.materialise_due(now + RESOURCE_OPERATION_SPAN, &native);
                assert_eq!(
                    store
                        .request("a-request-token")
                        .map(|record| record.operation_status.as_str()),
                    Some(STATUS_CANCEL_COMPLETE),
                    "case {status:?}: expected the request to settle as canceled, got {:?}",
                    store.request("a-request-token")
                );
                assert_eq!(
                    store.resource_count(),
                    0,
                    "case {status:?}: a canceled create must provision nothing"
                );
                assert!(
                    store
                        .resolve(&native, DEFAULT_REGION, "AWS::Kinesis::Stream", "stream")
                        .is_none(),
                    "case {status:?}: a canceled create must leave no resource to resolve"
                );
            } else {
                match &verdict {
                    CancelVerdict::NotCancelable(reported) => assert_eq!(
                        reported, status,
                        "case {status:?}: expected the refusal to name that status, got \
                         {reported:?}"
                    ),
                    other => panic!(
                        "case {status:?}: expected the cancel to be refused, got {other:?}"
                    ),
                }
                assert_eq!(
                    store
                        .request("a-request-token")
                        .map(|record| record.operation_status.as_str()),
                    Some(*status),
                    "case {status:?}: a refused cancel must leave the request as it was, got \
                     {:?}",
                    store.request("a-request-token")
                );
            }
        }
        let mut empty = ComAmazonawsCloudcontrolCloudapiserviceState::default();
        assert!(
            matches!(
                empty.cancel_request("a-request-token", now),
                CancelVerdict::NotFound
            ),
            "case no such request: expected the token to name nothing"
        );
    }
}
